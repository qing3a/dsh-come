/**
 * dsh-recruit-tools —— 猎头工作台工具插件（骨架）
 *
 * 领域模型参考 md-agent 的 headhunter 模板（src/templates/projects/headhunter/）：
 *   - 候选人 / 职位 / 推荐 全流程（推荐 → 面试 → Offer → 入职）
 *   - 本地优先：数据落 $DSH_HOME/recruit/（JSON 明文，可审计、可回滚）
 *   - 隐私与保密：仅记录工具收到的必要事实，不编造；敏感字段标注 confidential
 *
 * 开发加载（本机验证过的调用方式）：
 *   dsh web --patch <本目录>/cordis.yml --host 127.0.0.1 --port <port>
 */

import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import type { Context } from '@deepseek-ai/cordis'
import z from '@deepseek-ai/schemastery'
import { defineTool } from '@deepseek-ai/dsh-tools'
import { dshHomePath, expandHomePath } from '@deepseek-ai/dsh-home-paths'
import type { IncomingMessage, ServerResponse } from 'node:http'

export const name = 'recruit-tools'
export const inject = ['tools', 'webServer']

/** 插件配置：数据目录可覆盖；默认 `$DSH_HOME/recruit`（本地优先、随 DSH 数据隔离）。 */
export interface Config {
  /** 数据目录；空字符串 = 默认 `$DSH_HOME/recruit`。任何部署想换目录都应改配置而非改代码。 */
  dataDir: string
}

export const Config = z.object({
  dataDir: z.string().default(''),
})

// ---------- 领域模型（继承 md-agent headhunter 模板的规则） ----------

/** 候选人阶段 */
export const CANDIDATE_STAGES = ['sourcing', 'contacted', 'interviewing', 'offered', 'placed', 'archived'] as const
export type CandidateStage = (typeof CANDIDATE_STAGES)[number]

/** 推荐流水线 7 态 —— 对齐 md-agent headhunter 应用（kb/apps/headhunter/index.html）：
 *  推进链 STAGE_NEXT：已推荐 → 待客户反馈 → 面试中 → 已发Offer → 已入职；
 *  终态（endRec）：拒绝 / 撤回，可从任意状态直接到达。
 */
export const REFERRAL_STAGES = ['已推荐', '待客户反馈', '面试中', '已发Offer', '已入职', '拒绝', '撤回'] as const
export type ReferralStage = (typeof REFERRAL_STAGES)[number]

/** 推进链（md-agent STAGE_NEXT 逐字对齐） */
export const REFERRAL_NEXT: Record<string, ReferralStage | undefined> = {
  '已推荐': '待客户反馈',
  '待客户反馈': '面试中',
  '面试中': '已发Offer',
  '已发Offer': '已入职',
}

/** 终态（md-agent endRec 的两个结束分支 + 入职） */
export const REFERRAL_TERMINAL: ReadonlySet<string> = new Set(['已入职', '拒绝', '撤回'])

export interface Candidate {
  id: string
  name: string
  title: string
  company: string
  stage: CandidateStage
  notes: string
  confidential: boolean
  createdAt: string
  updatedAt: string
}

export interface Position {
  id: string
  client: string
  title: string
  requirements: string
  salaryRange: string
  confidential: boolean
  createdAt: string
  updatedAt: string
}

export interface Referral {
  id: string
  candidateId: string
  positionId: string
  stage: ReferralStage
  note: string
  createdAt: string
  updatedAt: string
}

interface Store {
  candidates: Candidate[]
  positions: Position[]
  referrals: Referral[]
}

// ---------- 本地 JSON 存储（原子写：临时文件 + rename） ----------

function emptyStore(): Store {
  return { candidates: [], positions: [], referrals: [] }
}

async function loadStore(dataDir: string): Promise<Store> {
  await mkdir(dataDir, { recursive: true })
  const path = join(dataDir, 'store.json')
  try {
    const raw = await readFile(path, 'utf8')
    const parsed = JSON.parse(raw) as Partial<Store>
    const base = emptyStore()
    return {
      candidates: Array.isArray(parsed.candidates) ? parsed.candidates : base.candidates,
      positions: Array.isArray(parsed.positions) ? parsed.positions : base.positions,
      referrals: Array.isArray(parsed.referrals) ? parsed.referrals : base.referrals,
    }
  } catch {
    return emptyStore() // 首次运行 / 文件损坏：从空库开始（不静默丢数据——先留空，后续版本加备份）
  }
}

async function saveStore(dataDir: string, store: Store): Promise<void> {
  const path = join(dataDir, 'store.json')
  const tmp = join(dataDir, 'store.json.tmp')
  await writeFile(tmp, JSON.stringify(store, null, 2), 'utf8')
  await rename(tmp, path) // 原子替换：崩溃不产生半截文件
}

// ---------- 领域操作（AI 工具与工作台 HTTP API 共用同一套校验/写入） ----------

function upsertCandidate(store: Store, args: { id?: string; name: string; title?: string; company?: string; stage?: CandidateStage; notes?: string; confidential?: boolean; now: string }): Candidate {
  const record: Candidate = {
    id: args.id && args.id.trim() ? args.id : randomUUID(),
    name: String(args.name ?? '').trim(),
    title: args.title ? String(args.title).trim() : '',
    company: args.company ? String(args.company).trim() : '',
    stage: args.stage ?? 'sourcing',
    notes: args.notes ? String(args.notes).trim() : '',
    confidential: args.confidential ?? true,
    createdAt: args.now,
    updatedAt: args.now,
  }
  if (!record.name) throw new Error('recruit_register_candidate: name 必填且不能为空')
  const i = store.candidates.findIndex((c) => c.id === record.id)
  if (i >= 0) {
    record.createdAt = store.candidates[i].createdAt
    store.candidates[i] = record
  } else {
    store.candidates.push(record)
  }
  return record
}

function upsertPosition(store: Store, args: { id?: string; client: string; title: string; requirements?: string; salaryRange?: string; confidential?: boolean; now: string }): Position {
  const record: Position = {
    id: args.id && args.id.trim() ? args.id : randomUUID(),
    client: String(args.client ?? '').trim(),
    title: String(args.title ?? '').trim(),
    requirements: args.requirements ? String(args.requirements).trim() : '',
    salaryRange: args.salaryRange ? String(args.salaryRange).trim() : '',
    confidential: args.confidential ?? true,
    createdAt: args.now,
    updatedAt: args.now,
  }
  if (!record.client || !record.title) throw new Error('recruit_register_position: client 与 title 必填且不能为空')
  const i = store.positions.findIndex((p) => p.id === record.id)
  if (i >= 0) {
    record.createdAt = store.positions[i].createdAt
    store.positions[i] = record
  } else {
    store.positions.push(record)
  }
  return record
}

function createReferral(store: Store, args: { candidateId: string; positionId: string; stage?: ReferralStage; note?: string; now: string }): Referral {
  const candidate = store.candidates.find((c) => c.id === args.candidateId)
  if (!candidate) throw new Error(`recruit_create_referral: 候选人 ${args.candidateId} 不存在（先 recruit_register_candidate）`)
  const position = store.positions.find((p) => p.id === args.positionId)
  if (!position) throw new Error(`recruit_create_referral: 职位 ${args.positionId} 不存在（先 recruit_register_position）`)
  const referral: Referral = {
    id: randomUUID(),
    candidateId: args.candidateId,
    positionId: args.positionId,
    stage: args.stage ?? '已推荐',
    note: args.note ? String(args.note).trim() : '',
    createdAt: args.now,
    updatedAt: args.now,
  }
  store.referrals.push(referral)
  return referral
}

function advanceReferral(store: Store, id: string, stage: ReferralStage, note: string | undefined, ts: string): Referral {
  const r = store.referrals.find((x) => x.id === id)
  if (!r) throw new Error(`recruit_update_referral_stage: 推荐 ${id} 不存在（先 recruit_create_referral）`)
  const current = r.stage
  // 迁移规则（md-agent）：只能推进链下一步，或直达终态（拒绝/撤回/已入职）
  const nextAllowed = REFERRAL_NEXT[current]
  const allowed = nextAllowed === stage || REFERRAL_TERMINAL.has(stage)
  if (!allowed) {
    throw new Error(
      `recruit_update_referral_stage: 非法阶段迁移 ${current} → ${stage}；` +
      `只允许推进到 ${nextAllowed ?? '（无）'} 或直达终态 已入职/拒绝/撤回`,
    )
  }
  r.stage = stage
  if (note) r.note = String(note).trim()
  r.updatedAt = ts
  return r
}

function joinReferral(store: Store, r: Referral) {
  const c = store.candidates.find((x) => x.id === r.candidateId)
  const p = store.positions.find((x) => x.id === r.positionId)
  return {
    id: r.id,
    candidateId: r.candidateId,
    candidateName: c?.name ?? r.candidateId,
    positionId: r.positionId,
    positionTitle: p?.title ?? r.positionId,
    client: p?.client ?? '',
    stage: r.stage,
    note: r.note,
    createdAt: r.createdAt,
    updatedAt: r.updatedAt,
  }
}

// ---------- 工作台 HTTP API（node:http 路由，webServer 注册） ----------

async function readBodyJson(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = []
  let size = 0
  for await (const chunk of req) {
    size += (chunk as Buffer).length
    if (size > 1024 * 1024) throw new Error('请求体过大（>1MB）')
    chunks.push(chunk as Buffer)
  }
  if (chunks.length === 0) return {}
  return JSON.parse(Buffer.concat(chunks).toString('utf8'))
}

function sendJson(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' })
  res.end(JSON.stringify(body))
}

function sendText(res: ServerResponse, status: number, contentType: string, body: string): void {
  res.writeHead(status, { 'content-type': contentType })
  res.end(body)
}

async function handleApi(dataDir: string, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? '/', 'http://localhost')
  const path = url.pathname.replace(/^\/recruit\/api/, '') || '/'
  const method = (req.method ?? 'GET').toUpperCase()
  const store = await loadStore(dataDir)

  if (method === 'GET' && path === '/') {
    const referralByStage = REFERRAL_STAGES
      .map((s) => ({ stage: s, count: store.referrals.filter((x) => x.stage === s).length }))
      .filter((x) => x.count > 0)
    sendJson(res, 200, {
      dataDir,
      candidateCount: store.candidates.length,
      positionCount: store.positions.length,
      referralCount: store.referrals.length,
      referralByStage,
    })
    return
  }

  if (method === 'GET' && path === '/candidates') {
    const stage = url.searchParams.get('stage') ?? undefined
    const q = (url.searchParams.get('query') ?? '').toLowerCase()
    const list = store.candidates.filter((c) =>
      (!stage || c.stage === stage) &&
      (!q || c.name.toLowerCase().includes(q) || c.title.toLowerCase().includes(q) || c.company.toLowerCase().includes(q)))
    sendJson(res, 200, { count: list.length, candidates: list })
    return
  }
  if (method === 'POST' && path === '/candidates') {
    const body = (await readBodyJson(req)) as Record<string, unknown>
    const record = upsertCandidate(store, { ...body, now: new Date().toISOString() } as never)
    await saveStore(dataDir, store)
    sendJson(res, 200, record)
    return
  }

  if (method === 'GET' && path === '/positions') {
    const client = url.searchParams.get('client') ?? undefined
    const q = (url.searchParams.get('query') ?? '').toLowerCase()
    const list = store.positions.filter((p) =>
      (!client || p.client === client) &&
      (!q || p.client.toLowerCase().includes(q) || p.title.toLowerCase().includes(q) || p.requirements.toLowerCase().includes(q)))
    sendJson(res, 200, { count: list.length, positions: list })
    return
  }
  if (method === 'POST' && path === '/positions') {
    const body = (await readBodyJson(req)) as Record<string, unknown>
    const record = upsertPosition(store, { ...body, now: new Date().toISOString() } as never)
    await saveStore(dataDir, store)
    sendJson(res, 200, record)
    return
  }

  if (method === 'GET' && path === '/referrals') {
    const stage = url.searchParams.get('stage') ?? undefined
    const list = store.referrals.filter((r) => !stage || r.stage === stage)
    sendJson(res, 200, { count: list.length, referrals: list.map((r) => joinReferral(store, r)) })
    return
  }
  if (method === 'POST' && path === '/referrals') {
    const body = (await readBodyJson(req)) as Record<string, unknown>
    const referral = createReferral(store, { ...body, now: new Date().toISOString() } as never)
    await saveStore(dataDir, store)
    sendJson(res, 200, referral)
    return
  }
  const stageMatch = path.match(/^\/referrals\/([^/]+)\/stage$/)
  if (method === 'POST' && stageMatch) {
    const body = (await readBodyJson(req)) as { stage?: ReferralStage; note?: string }
    const r = advanceReferral(store, decodeURIComponent(stageMatch[1]), body.stage!, body.note, new Date().toISOString())
    await saveStore(dataDir, store)
    sendJson(res, 200, { id: r.id, stage: r.stage, updatedAt: r.updatedAt })
    return
  }

  sendJson(res, 404, { error: `未找到接口: ${method} ${path}` })
}

// ---------- 工具注册 ----------

export function apply(ctx: Context, config: Config) {
  const dataDir = config.dataDir ? expandHomePath(config.dataDir) : dshHomePath('recruit')
  const now = () => new Date().toISOString()
  const text = (t: string): { type: 'text'; text: string }[] => [{ type: 'text', text: t }]

  ctx.tools.register(defineTool({
    name: 'recruit_register_candidate',
    description:
      '登记/更新一名候选人。候选人资料涉及隐私：只记录必要事实，内容只存本机；' +
      '薪资、Offer 等敏感信息请放入 notes 并保留 confidential=true（默认开启）。事实必须来自输入，不得编造。',
    parameters: {
      id: { type: 'string', description: '候选人 id；不传则新建' },
      name: { type: 'string', required: true, description: '候选人姓名' },
      title: { type: 'string', description: '当前职位头衔' },
      company: { type: 'string', description: '当前公司' },
      stage: { type: 'string', enum: [...CANDIDATE_STAGES], description: '阶段：sourcing/contacted/interviewing/offered/placed/archived' },
      notes: { type: 'string', description: '必要事实备注' },
      confidential: { type: 'boolean', description: '是否含保密信息（默认 true）' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          id: { type: 'string', required: true },
          name: { type: 'string', required: true },
          stage: { type: 'string', required: true },
          updatedAt: { type: 'string', required: true },
        },
      },
      render: (_args, value) => text(`候选人 [${value.id}] ${value.name}（阶段：${value.stage}）已登记。`),
      presentCall: (args) => ({
        card: 'generic',
        title: '登记候选人',
        kind: 'other',
        rawInput: args.name ?? '',
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `候选人已登记：${value.name}`,
        content: text(`阶段：${value.stage}｜id：${value.id}`),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const record = upsertCandidate(store, { ...args, now: now() })
      await saveStore(dataDir, store)
      return { id: record.id, name: record.name, stage: record.stage, updatedAt: record.updatedAt }
    },
  }))

  ctx.tools.register(defineTool({
    name: 'recruit_list_candidates',
    description: '列出候选人台账。可按阶段过滤或按姓名/职位关键词检索；只返回存储中的事实。',
    parameters: {
      stage: { type: 'string', enum: [...CANDIDATE_STAGES], description: '按阶段过滤' },
      query: { type: 'string', description: '匹配姓名/职位/公司' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          count: { type: 'integer', required: true },
          candidates: {
            type: 'array', required: true,
            items: {
              type: 'object', additionalProperties: false,
              properties: {
                id: { type: 'string', required: true },
                name: { type: 'string', required: true },
                title: { type: 'string', required: true },
                company: { type: 'string', required: true },
                stage: { type: 'string', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => {
        if (value.count === 0) return text('暂无候选人。')
        const rows = value.candidates.map((c) => `- ${c.id} ${c.name}｜${c.title || '无头衔'}@${c.company || '无公司'}｜${c.stage}`).join('\n')
        return text(`共 ${value.count} 名候选人：\n${rows}`)
      },
      presentCall: (args) => ({
        card: 'generic',
        title: '查看候选人',
        kind: 'other',
        rawInput: args.stage ? `阶段：${args.stage}` : (args.query ? `检索：${args.query}` : '全部'),
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `${value.count} 名候选人`,
        content: value.count === 0
          ? text('暂无候选人。')
          : text(value.candidates.map((c) => `- ${c.name}｜${c.title || '无头衔'}@${c.company || '无公司'}｜${c.stage}`).join('\n')),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const q = (args.query ?? '').toLowerCase()
      const list = store.candidates.filter((c) =>
        (!args.stage || c.stage === args.stage) &&
        (!q || c.name.toLowerCase().includes(q) || c.title.toLowerCase().includes(q) || c.company.toLowerCase().includes(q)),
      )
      return {
        count: list.length,
        candidates: list.map((c) => ({ id: c.id, name: c.name, title: c.title, company: c.company, stage: c.stage })),
      }
    },
  }))

  ctx.tools.register(defineTool({
    name: 'recruit_register_position',
    description:
      '登记/更新一个职位需求。职位事实（客户、要求、薪资）必须来自输入，不得编造；' +
      '薪资区间属保密信息，默认 confidential=true。',
    parameters: {
      id: { type: 'string', description: '职位 id；不传则新建' },
      client: { type: 'string', required: true, description: '客户公司' },
      title: { type: 'string', required: true, description: '职位名称' },
      requirements: { type: 'string', description: '职位要求' },
      salaryRange: { type: 'string', description: '薪资区间（保密）' },
      confidential: { type: 'boolean', description: '是否含保密信息（默认 true）' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          id: { type: 'string', required: true },
          client: { type: 'string', required: true },
          title: { type: 'string', required: true },
          updatedAt: { type: 'string', required: true },
        },
      },
      render: (_args, value) => text(`职位 [${value.id}] ${value.client}｜${value.title} 已登记。`),
      presentCall: (args) => ({
        card: 'generic',
        title: '登记职位',
        kind: 'other',
        rawInput: `${args.client}｜${args.title}`,
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `职位已登记：${value.client}｜${value.title}`,
        content: text(`id：${value.id}`),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const record = upsertPosition(store, { ...args, now: now() })
      await saveStore(dataDir, store)
      return { id: record.id, client: record.client, title: record.title, updatedAt: record.updatedAt }
    },
  }))

  ctx.tools.register(defineTool({
    name: 'recruit_list_positions',
    description: '列出职位需求。可按客户过滤或按职位名/要求关键词检索。',
    parameters: {
      client: { type: 'string', description: '按客户公司过滤' },
      query: { type: 'string', description: '匹配职位名/要求' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          count: { type: 'integer', required: true },
          positions: {
            type: 'array', required: true,
            items: {
              type: 'object', additionalProperties: false,
              properties: {
                id: { type: 'string', required: true },
                client: { type: 'string', required: true },
                title: { type: 'string', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => {
        if (value.count === 0) return text('暂无职位需求。')
        const rows = value.positions.map((p) => `- ${p.id} ${p.client}｜${p.title}`).join('\n')
        return text(`共 ${value.count} 个职位：\n${rows}`)
      },
      presentCall: (args) => ({
        card: 'generic',
        title: '查看职位',
        kind: 'other',
        rawInput: args.client ? `客户：${args.client}` : (args.query ? `检索：${args.query}` : '全部'),
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `${value.count} 个职位`,
        content: value.count === 0
          ? text('暂无职位需求。')
          : text(value.positions.map((p) => `- ${p.client}｜${p.title}`).join('\n')),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const q = (args.query ?? '').toLowerCase()
      const list = store.positions.filter((p) =>
        (!args.client || p.client === args.client) &&
        (!q || p.client.toLowerCase().includes(q) || p.title.toLowerCase().includes(q) || p.requirements.toLowerCase().includes(q)),
      )
      return { count: list.length, positions: list.map((p) => ({ id: p.id, client: p.client, title: p.title })) }
    },
  }))

  ctx.tools.register(defineTool({
    name: 'recruit_create_referral',
    description:
      '创建一条推荐：把候选人推荐到职位，进入推荐流水线（初始阶段 已推荐）。candidateId 与 positionId 必须已存在；' +
      '推荐事实（谁推给谁、当前阶段）基于存储，不编造。',
    parameters: {
      candidateId: { type: 'string', required: true, description: '候选人 id' },
      positionId: { type: 'string', required: true, description: '职位 id' },
      stage: { type: 'string', enum: [...REFERRAL_STAGES], description: '初始阶段（默认 已推荐）' },
      note: { type: 'string', description: '推荐说明' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          id: { type: 'string', required: true },
          candidateId: { type: 'string', required: true },
          positionId: { type: 'string', required: true },
          stage: { type: 'string', required: true },
          createdAt: { type: 'string', required: true },
        },
      },
      render: (_args, value) => text(`推荐 [${value.id}]：候选人 ${value.candidateId} → 职位 ${value.positionId}（阶段：${value.stage}）。`),
      presentCall: (args) => ({
        card: 'generic',
        title: '创建推荐',
        kind: 'other',
        rawInput: `候选人 ${args.candidateId} → 职位 ${args.positionId}`,
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `推荐已创建：${value.candidateId} → ${value.positionId}`,
        content: text(`阶段：${value.stage}｜id：${value.id}`),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const referral = createReferral(store, { ...args, now: now() })
      await saveStore(dataDir, store)
      return { id: referral.id, candidateId: referral.candidateId, positionId: referral.positionId, stage: referral.stage, createdAt: referral.createdAt }
    },
  }))

  ctx.tools.register(defineTool({
    name: 'recruit_update_referral_stage',
    description:
      '推进推荐流水线阶段（已推荐 → 待客户反馈 → 面试中 → 已发Offer → 已入职；终态 拒绝/撤回 可直达）。' +
      '只能推进链的下一步或直达终态，不得跳级或回退；阶段变更必须基于真实沟通/面试进展，不得编造。',
    parameters: {
      referralId: { type: 'string', required: true, description: '推荐 id' },
      stage: { type: 'string', required: true, enum: [...REFERRAL_STAGES], description: '新阶段' },
      note: { type: 'string', description: '进展说明' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          id: { type: 'string', required: true },
          stage: { type: 'string', required: true },
          updatedAt: { type: 'string', required: true },
        },
      },
      render: (_args, value) => text(`推荐 [${value.id}] 阶段已更新为 ${value.stage}。`),
      presentCall: (args) => ({
        card: 'generic',
        title: '推进推荐阶段',
        kind: 'other',
        rawInput: `推荐 ${args.referralId} → ${args.stage}`,
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `阶段已更新：${value.stage}`,
        content: text(`推荐 id：${value.id}`),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const r = advanceReferral(store, args.referralId, args.stage, args.note, now())
      await saveStore(dataDir, store)
      return { id: r.id, stage: r.stage, updatedAt: r.updatedAt }
    },
  }))

  ctx.tools.register(defineTool({
    name: 'recruit_list_referrals',
    description: '列出推荐流水线。可按阶段过滤；输出附带候选人姓名与职位信息（来自存储，便于查看）。',
    parameters: {
      stage: { type: 'string', enum: [...REFERRAL_STAGES], description: '按阶段过滤' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          count: { type: 'integer', required: true },
          referrals: {
            type: 'array', required: true,
            items: {
              type: 'object', additionalProperties: false,
              properties: {
                id: { type: 'string', required: true },
                candidateName: { type: 'string', required: true },
                positionTitle: { type: 'string', required: true },
                client: { type: 'string', required: true },
                stage: { type: 'string', required: true },
                updatedAt: { type: 'string', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => {
        if (value.count === 0) return text('暂无推荐。')
        const rows = value.referrals.map((r) => `- ${r.id} ${r.candidateName} → ${r.client}｜${r.positionTitle}｜${r.stage}`).join('\n')
        return text(`共 ${value.count} 条推荐：\n${rows}`)
      },
      presentCall: (args) => ({
        card: 'generic',
        title: '查看推荐',
        kind: 'other',
        rawInput: args.stage ? `阶段：${args.stage}` : '全部',
      }),
      presentResult: (_args, value) => ({
        card: 'generic',
        title: `${value.count} 条推荐`,
        content: value.count === 0
          ? text('暂无推荐。')
          : text(value.referrals.map((r) => `- ${r.candidateName} → ${r.client}｜${r.positionTitle}｜${r.stage}`).join('\n')),
      }),
    },
    execute: async (args) => {
      const store = await loadStore(dataDir)
      const list = store.referrals.filter((r) => !args.stage || r.stage === args.stage)
      return {
        count: list.length,
        referrals: list.map((r) => {
          const c = store.candidates.find((x) => x.id === r.candidateId)
          const p = store.positions.find((x) => x.id === r.positionId)
          return {
            id: r.id,
            candidateName: c?.name ?? r.candidateId,
            positionTitle: p?.title ?? r.positionId,
            client: p?.client ?? '',
            stage: r.stage,
            updatedAt: r.updatedAt,
          }
        }),
      }
    },
  }))


  ctx.tools.register(defineTool({
    name: 'recruit_status',
    description: '猎头工作台总览：候选人/职位/推荐数量与推荐流水线各阶段分布（工作台仪表卡）。',
    parameters: {},
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          candidateCount: { type: 'integer', required: true },
          positionCount: { type: 'integer', required: true },
          referralCount: { type: 'integer', required: true },
          referralByStage: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                stage: { type: 'string', required: true },
                count: { type: 'integer', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => {
        const dist = value.referralByStage
          .map((x) => `${x.stage}：${x.count}`)
          .join('、')
        return text(`候选人 ${value.candidateCount}｜职位 ${value.positionCount}｜推荐 ${value.referralCount}；${dist || '暂无推荐'}`)
      },
      presentCall: () => ({
        card: 'generic',
        title: '工作台总览',
        kind: 'other',
        rawInput: '',
      }),
      presentResult: (_args, value) => {
        const lines = [
          `候选人：${value.candidateCount}`,
          `职位：${value.positionCount}`,
          `推荐：${value.referralCount}`,
          ...value.referralByStage.map((x) => `推荐·${x.stage}：${x.count}`),
        ]
        return {
          card: 'generic',
          title: '猎头工作台',
          content: text(lines.join('\n')),
        }
      },
    },
    execute: async () => {
      const store = await loadStore(dataDir)
      const referralByStage = REFERRAL_STAGES
        .map((s) => ({ stage: s, count: store.referrals.filter((x) => x.stage === s).length }))
        .filter((x) => x.count > 0)
      return {
        candidateCount: store.candidates.length,
        positionCount: store.positions.length,
        referralCount: store.referrals.length,
        referralByStage,
      }
    },
  }))

  // ---------- 猎头工作台 Web 界面（webServer 路由；与 AI 工具共用同一份 store） ----------
  ctx.webServer.register({
    kind: 'exact',
    path: '/recruit',
    handler: async (_req, res) => {
      try {
        const html = await readFile(fileURLToPath(new URL('./workbench.html', import.meta.url)), 'utf8')
        sendText(res, 200, 'text/html; charset=utf-8', html)
      } catch (e) {
        sendText(res, 500, 'text/plain; charset=utf-8', `工作台页面读取失败: ${e instanceof Error ? e.message : e}`)
      }
    },
  })
  ctx.webServer.register({
    kind: 'prefix',
    path: '/recruit/api',
    handler: (req, res) => {
      handleApi(dataDir, req, res).catch((e) => {
        sendJson(res, 500, { error: e instanceof Error ? e.message : String(e) })
      })
    },
  })

  const selfPath = fileURLToPath(import.meta.url)
  console.log(`[recruit-tools] plugin loaded! dataDir=${dataDir} src=${selfPath}`)
}