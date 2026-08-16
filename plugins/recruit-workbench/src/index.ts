/**
 * dsh-recruit-workbench —— 猎头工作台插件（完整版）
 *
 * 领域模型继承 md-agent headhunter 模板（C:\Users\Administrator\Desktop\md-agent
 * \src\templates\projects\headhunter\，MIT 开源）+ ow-recruit 界面资产：
 *   - 候选人 / 客户公司 / 职位需求 / 推荐 7 态 / 沟通留痕 / 面试 / Offer / 审计
 *   - 本地优先：数据落 $DSH_HOME/recruit-workbench/store.json（JSON 明文、原子写、可审计）
 *   - 核心规则（继承模板 RULES.md）：
 *       1. 职位与候选人事实基于输入，不编造；
 *       2. 客户/候选人严格隔离，绝不串用；
 *       3. 候选人隐私：只记必要事实，内容只存本机；
 *       4. 薪资/Offer/联系方式等敏感信息默认 confidential=true；
 *       5. 重要信息先确认再落盘；删除需 confirm=true。
 *   - 推荐状态机（对齐 md-agent）：recommended→pending_client→interviewing→
 *       offer_sent→hired；rejected/withdrawn 为终态可直达；只允许推进链下一步或直达终态。
 *
 * 双通道：
 *   - 工具面：recruitwb_* 共 18 个工具（模型可见，走对话）
 *   - 浏览器面：GET /api/recruit-workbench/state + POST /api/recruit-workbench/mutate
 *     （工作台 UI 读写同一份业务逻辑；写操作同样落审计）
 *
 * 加载（开发）：
 *   dsh web --patch <本目录>/cordis.yml --host 127.0.0.1 --port <port>
 */

import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import type { IncomingMessage, ServerResponse } from 'node:http'

import type { Context } from '@deepseek-ai/cordis'
import z from '@deepseek-ai/schemastery'
import { defineTool } from '@deepseek-ai/dsh-tools'
import { dshHomePath, expandHomePath } from '@deepseek-ai/dsh-home-paths'

export const name = 'recruit-workbench'
export const inject = ['tools']

/** 插件配置：数据目录可覆盖；默认 `$DSH_HOME/recruit-workbench`。 */
export interface Config {
  /** 数据目录；空字符串 = 默认 `$DSH_HOME/recruit-workbench`。 */
  dataDir: string
}

export const Config = z.object({
  dataDir: z.string().default(''),
})

// ---------- 领域模型 ----------

/** 候选人阶段 */
export const CANDIDATE_STAGES = ['sourcing', 'contacted', 'interviewing', 'offered', 'placed', 'archived'] as const
export type CandidateStage = (typeof CANDIDATE_STAGES)[number]

/** 推荐流水线：推进链 5 态 + 终态 2 态（对齐 md-agent 7 态） */
export const REFERRAL_CHAIN = ['recommended', 'pending_client', 'interviewing', 'offer_sent', 'hired'] as const
export const REFERRAL_TERMINALS = ['rejected', 'withdrawn'] as const
export const REFERRAL_STAGES = [...REFERRAL_CHAIN, ...REFERRAL_TERMINALS] as const
export type ReferralStage = (typeof REFERRAL_STAGES)[number]

/** 推荐推进表：链上下一步（终态不可再推进） */
export const REFERRAL_NEXT: Record<string, string> = {
  recommended: 'pending_client',
  pending_client: 'interviewing',
  interviewing: 'offer_sent',
  offer_sent: 'hired',
}

/** 职位状态 */
export const POSITION_STATUSES = ['open', 'paused', 'closed'] as const
export type PositionStatus = (typeof POSITION_STATUSES)[number]

/** 活动类型与目标类型 */
export const ACTIVITY_KINDS = ['comm', 'interview', 'offer', 'note', 'system'] as const
export type ActivityKind = (typeof ACTIVITY_KINDS)[number]
export const ACTIVITY_TARGET_TYPES = ['candidate', 'position', 'referral', 'company'] as const
export type ActivityTargetType = (typeof ACTIVITY_TARGET_TYPES)[number]

/** Offer 状态 */
export const OFFER_STATUSES = ['draft', 'sent', 'accepted', 'declined'] as const
export type OfferStatus = (typeof OFFER_STATUSES)[number]

export interface Company {
  id: string
  name: string
  industry: string
  size: string
  /** 对接人 */
  contact: string
  /** 对接人联系方式（保密） */
  contactPhone: string
  notes: string
  confidential: boolean
  createdAt: string
  updatedAt: string
}

export interface Candidate {
  id: string
  name: string
  title: string
  /** 现公司 */
  company: string
  city: string
  /** 电话（保密） */
  phone: string
  /** 邮箱（保密） */
  email: string
  /** 简历要点（只记必要事实） */
  resume: string
  /** 薪资期望（保密） */
  salaryExpect: string
  /** 标签（技能/关键词，用于检索） */
  tags: string[]
  stage: CandidateStage
  notes: string
  confidential: boolean
  createdAt: string
  updatedAt: string
}

export interface Position {
  id: string
  /** 客户公司 */
  client: string
  title: string
  city: string
  quantity: number
  /** 硬性要求 */
  requirements: string
  /** 软性要求/加分项 */
  niceToHave: string
  /** 薪资区间（保密） */
  salaryRange: string
  status: PositionStatus
  notes: string
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

export interface Activity {
  id: string
  kind: ActivityKind
  targetType: ActivityTargetType
  targetId: string
  text: string
  confidential: boolean
  createdAt: string
}

export interface Interview {
  id: string
  referralId: string
  candidateId: string
  positionId: string
  /** 轮次：一面/二面/终面… */
  round: string
  /** 时间：ISO 或自由文本 */
  when: string
  /** 方式：onsite/video/phone */
  mode: string
  note: string
  createdAt: string
}

export interface Offer {
  id: string
  referralId: string
  candidateId: string
  positionId: string
  /** 薪酬包（保密） */
  package: string
  status: OfferStatus
  note: string
  createdAt: string
  updatedAt: string
}

export interface AuditEntry {
  id: string
  ts: string
  action: string
  targetType: string
  targetId: string
  actor: string
  detail: string
}

export interface Store {
  companies: Company[]
  candidates: Candidate[]
  positions: Position[]
  referrals: Referral[]
  activities: Activity[]
  interviews: Interview[]
  offers: Offer[]
  audit: AuditEntry[]
}

// ---------- 本地 JSON 存储（原子写：临时文件 + rename） ----------

function emptyStore(): Store {
  return { companies: [], candidates: [], positions: [], referrals: [], activities: [], interviews: [], offers: [], audit: [] }
}

async function loadStore(dataDir: string): Promise<Store> {
  await mkdir(dataDir, { recursive: true })
  const path = join(dataDir, 'store.json')
  try {
    const raw = await readFile(path, 'utf8')
    const parsed = JSON.parse(raw) as Partial<Store>
    const base = emptyStore()
    return {
      companies: Array.isArray(parsed.companies) ? parsed.companies : base.companies,
      candidates: Array.isArray(parsed.candidates) ? parsed.candidates : base.candidates,
      positions: Array.isArray(parsed.positions) ? parsed.positions : base.positions,
      referrals: Array.isArray(parsed.referrals) ? parsed.referrals : base.referrals,
      activities: Array.isArray(parsed.activities) ? parsed.activities : base.activities,
      interviews: Array.isArray(parsed.interviews) ? parsed.interviews : base.interviews,
      offers: Array.isArray(parsed.offers) ? parsed.offers : base.offers,
      audit: Array.isArray(parsed.audit) ? parsed.audit : base.audit,
    }
  } catch {
    return emptyStore()
  }
}

async function saveStore(dataDir: string, store: Store): Promise<void> {
  const path = join(dataDir, 'store.json')
  const tmp = join(dataDir, 'store.json.tmp')
  await writeFile(tmp, JSON.stringify(store, null, 2), 'utf8')
  await rename(tmp, path)
}

// ---------- 工作台业务核心 ----------

export type OpArgs = Record<string, unknown>
export type Op = (wb: Workbench, args: OpArgs) => Promise<unknown>

const AUDIT_LIMIT = 500
const now = (): string => new Date().toISOString()
const str = (v: unknown): string => (v == null ? '' : String(v).trim())
const num = (v: unknown): number => (v == null ? 0 : Number(v))

export class Workbench {
  readonly dataDir: string

  constructor(dataDir: string) {
    this.dataDir = dataDir
  }

  private async mutate(store: Store, action: string, targetType: string, targetId: string, actor: string, detail: string): Promise<void> {
    store.audit.push({ id: randomUUID(), ts: now(), action, targetType, targetId, actor, detail })
    if (store.audit.length > AUDIT_LIMIT) store.audit = store.audit.slice(-AUDIT_LIMIT)
    await saveStore(this.dataDir, store)
  }

  /** 深度拷贝快照（审计不随快照暴露给 UI，避免无限膨胀） */
  async snapshot(): Promise<Store> {
    const store = await loadStore(this.dataDir)
    return {
      companies: store.companies,
      candidates: store.candidates,
      positions: store.positions,
      referrals: store.referrals,
      activities: store.activities.slice(-100),
      interviews: store.interviews,
      offers: store.offers,
      audit: [],
    }
  }

  async auditTail(limit = 30): Promise<AuditEntry[]> {
    const store = await loadStore(this.dataDir)
    return store.audit.slice(-limit)
  }

  // ---- 客户公司 ----

  async registerCompany(args: OpArgs, actor: string): Promise<Company> {
    const store = await loadStore(this.dataDir)
    const ts = now()
    const record: Company = {
      id: args.id && str(args.id) ? str(args.id) : randomUUID(),
      name: str(args.name),
      industry: str(args.industry),
      size: str(args.size),
      contact: str(args.contact),
      contactPhone: str(args.contactPhone),
      notes: str(args.notes),
      confidential: args.confidential === undefined ? true : Boolean(args.confidential),
      createdAt: ts,
      updatedAt: ts,
    }
    if (!record.name) throw new Error('register_company: name 必填且不能为空')
    const i = store.companies.findIndex((c) => c.id === record.id)
    if (i >= 0) {
      record.createdAt = store.companies[i].createdAt
      store.companies[i] = record
    } else {
      store.companies.push(record)
    }
    await this.mutate(store, i >= 0 ? 'company.update' : 'company.create', 'company', record.id, actor, `公司：${record.name}`)
    return record
  }

  async listCompanies(args: OpArgs): Promise<{ count: number; companies: Company[] }> {
    const store = await loadStore(this.dataDir)
    const q = str(args.query).toLowerCase()
    const list = store.companies.filter((c) => !q || c.name.toLowerCase().includes(q) || c.industry.toLowerCase().includes(q) || c.contact.toLowerCase().includes(q))
    return { count: list.length, companies: list }
  }

  // ---- 候选人 ----

  async registerCandidate(args: OpArgs, actor: string): Promise<Candidate> {
    const store = await loadStore(this.dataDir)
    const ts = now()
    const tags = Array.isArray(args.tags) ? args.tags.map((t) => str(t)).filter(Boolean) : []
    const record: Candidate = {
      id: args.id && str(args.id) ? str(args.id) : randomUUID(),
      name: str(args.name),
      title: str(args.title),
      company: str(args.company),
      city: str(args.city),
      phone: str(args.phone),
      email: str(args.email),
      resume: str(args.resume),
      salaryExpect: str(args.salaryExpect),
      tags,
      stage: (args.stage as CandidateStage) ?? 'sourcing',
      notes: str(args.notes),
      confidential: args.confidential === undefined ? true : Boolean(args.confidential),
      createdAt: ts,
      updatedAt: ts,
    }
    if (!record.name) throw new Error('register_candidate: name 必填且不能为空')
    if (!CANDIDATE_STAGES.includes(record.stage)) throw new Error(`register_candidate: 非法阶段 ${record.stage}`)
    const i = store.candidates.findIndex((c) => c.id === record.id)
    if (i >= 0) {
      record.createdAt = store.candidates[i].createdAt
      store.candidates[i] = record
    } else {
      store.candidates.push(record)
    }
    await this.mutate(store, i >= 0 ? 'candidate.update' : 'candidate.create', 'candidate', record.id, actor, `候选人：${record.name}`)
    return record
  }

  async listCandidates(args: OpArgs): Promise<{ count: number; candidates: Candidate[] }> {
    const store = await loadStore(this.dataDir)
    const q = str(args.query).toLowerCase()
    const stage = str(args.stage)
    const list = store.candidates.filter((c) =>
      (!stage || c.stage === stage) &&
      (!q || c.name.toLowerCase().includes(q) || c.title.toLowerCase().includes(q) || c.company.toLowerCase().includes(q) || c.tags.some((t) => t.toLowerCase().includes(q))),
    )
    return { count: list.length, candidates: list }
  }

  async getCandidate(id: string): Promise<Candidate | null> {
    const store = await loadStore(this.dataDir)
    return store.candidates.find((c) => c.id === id) ?? null
  }

  // ---- 职位 ----

  async registerPosition(args: OpArgs, actor: string): Promise<Position> {
    const store = await loadStore(this.dataDir)
    const ts = now()
    const record: Position = {
      id: args.id && str(args.id) ? str(args.id) : randomUUID(),
      client: str(args.client),
      title: str(args.title),
      city: str(args.city),
      quantity: num(args.quantity) || 1,
      requirements: str(args.requirements),
      niceToHave: str(args.niceToHave),
      salaryRange: str(args.salaryRange),
      status: (args.status as PositionStatus) ?? 'open',
      notes: str(args.notes),
      createdAt: ts,
      updatedAt: ts,
    }
    if (!record.client || !record.title) throw new Error('register_position: client 与 title 必填且不能为空')
    if (!POSITION_STATUSES.includes(record.status)) throw new Error(`register_position: 非法状态 ${record.status}`)
    const i = store.positions.findIndex((p) => p.id === record.id)
    if (i >= 0) {
      record.createdAt = store.positions[i].createdAt
      store.positions[i] = record
    } else {
      store.positions.push(record)
    }
    await this.mutate(store, i >= 0 ? 'position.update' : 'position.create', 'position', record.id, actor, `职位：${record.client}｜${record.title}`)
    return record
  }

  async listPositions(args: OpArgs): Promise<{ count: number; positions: Position[] }> {
    const store = await loadStore(this.dataDir)
    const q = str(args.query).toLowerCase()
    const client = str(args.client)
    const status = str(args.status)
    const list = store.positions.filter((p) =>
      (!client || p.client === client) &&
      (!status || p.status === status) &&
      (!q || p.client.toLowerCase().includes(q) || p.title.toLowerCase().includes(q) || p.requirements.toLowerCase().includes(q)),
    )
    return { count: list.length, positions: list }
  }

  async getPosition(id: string): Promise<Position | null> {
    const store = await loadStore(this.dataDir)
    return store.positions.find((p) => p.id === id) ?? null
  }

  // ---- 推荐（7 态状态机） ----

  async createReferral(args: OpArgs, actor: string): Promise<Referral> {
    const store = await loadStore(this.dataDir)
    const candidate = store.candidates.find((c) => c.id === str(args.candidateId))
    if (!candidate) throw new Error(`create_referral: 候选人 ${str(args.candidateId)} 不存在（先 register_candidate）`)
    const position = store.positions.find((p) => p.id === str(args.positionId))
    if (!position) throw new Error(`create_referral: 职位 ${str(args.positionId)} 不存在（先 register_position）`)
    const stage = (args.stage as ReferralStage) ?? 'recommended'
    if (!REFERRAL_STAGES.includes(stage)) throw new Error(`create_referral: 非法阶段 ${stage}`)
    const ts = now()
    const referral: Referral = {
      id: randomUUID(),
      candidateId: candidate.id,
      positionId: position.id,
      stage,
      note: str(args.note),
      createdAt: ts,
      updatedAt: ts,
    }
    store.referrals.push(referral)
    await this.mutate(store, 'referral.create', 'referral', referral.id, actor, `${candidate.name} → ${position.client}｜${position.title}`)
    return referral
  }

  async updateReferralStage(args: OpArgs, actor: string): Promise<Referral> {
    const store = await loadStore(this.dataDir)
    const r = store.referrals.find((x) => x.id === str(args.referralId))
    if (!r) throw new Error(`update_referral_stage: 推荐 ${str(args.referralId)} 不存在`)
    const next = str(args.stage) as ReferralStage
    if (!REFERRAL_STAGES.includes(next)) throw new Error(`update_referral_stage: 非法阶段 ${next}`)
    const cur = r.stage as string
    if ((REFERRAL_TERMINALS as readonly string[]).includes(cur)) throw new Error(`update_referral_stage: 推荐已处于终态 ${cur}，不可再变更`)
    if (!(REFERRAL_TERMINALS as readonly string[]).includes(next)) {
      const expected = REFERRAL_NEXT[cur]
      if (!expected || expected !== next) {
        throw new Error(`update_referral_stage: 不可跳级——${cur} 只能推进到 ${expected ?? '终态'}，或直达终态（rejected/withdrawn）`)
      }
    }
    r.stage = next
    r.updatedAt = now()
    if (args.note !== undefined) r.note = str(args.note)
    await this.mutate(store, 'referral.stage', 'referral', r.id, actor, `阶段：${cur} → ${next}`)
    return r
  }

  async listReferrals(args: OpArgs): Promise<{ count: number; referrals: Referral[] }> {
    const store = await loadStore(this.dataDir)
    const stage = str(args.stage)
    const list = store.referrals.filter((r) => !stage || r.stage === stage)
    return { count: list.length, referrals: list }
  }

  // ---- 沟通留痕 ----

  async addActivity(args: OpArgs, actor: string): Promise<Activity> {
    const store = await loadStore(this.dataDir)
    const kind = (args.kind as ActivityKind) ?? 'comm'
    const targetType = str(args.targetType) as ActivityTargetType
    if (!ACTIVITY_KINDS.includes(kind)) throw new Error(`add_activity: 非法类型 ${kind}`)
    if (!ACTIVITY_TARGET_TYPES.includes(targetType)) throw new Error(`add_activity: 非法目标类型 ${targetType}`)
    const targetId = str(args.targetId)
    const text = str(args.text)
    if (!targetId || !text) throw new Error('add_activity: targetId 与 text 必填')
    const record: Activity = {
      id: randomUUID(),
      kind,
      targetType,
      targetId,
      text,
      confidential: args.confidential === undefined ? true : Boolean(args.confidential),
      createdAt: now(),
    }
    store.activities.push(record)
    await this.mutate(store, 'activity.create', targetType, targetId, actor, `${kind}：${text.slice(0, 60)}`)
    return record
  }

  async listActivities(args: OpArgs): Promise<{ count: number; activities: Activity[] }> {
    const store = await loadStore(this.dataDir)
    const targetType = str(args.targetType)
    const targetId = str(args.targetId)
    const list = store.activities.filter((a) => (!targetType || a.targetType === targetType) && (!targetId || a.targetId === targetId))
    return { count: list.length, activities: list.slice(-200) }
  }

  // ---- 面试 ----

  async scheduleInterview(args: OpArgs, actor: string): Promise<Interview> {
    const store = await loadStore(this.dataDir)
    const referralId = str(args.referralId)
    const referral = store.referrals.find((r) => r.id === referralId)
    if (!referral) throw new Error(`schedule_interview: 推荐 ${referralId} 不存在`)
    const record: Interview = {
      id: randomUUID(),
      referralId,
      candidateId: referral.candidateId,
      positionId: referral.positionId,
      round: str(args.round) || '面试',
      when: str(args.when),
      mode: str(args.mode) || 'video',
      note: str(args.note),
      createdAt: now(),
    }
    if (!record.when) throw new Error('schedule_interview: when（时间）必填')
    store.interviews.push(record)
    const candidate = store.candidates.find((c) => c.id === referral.candidateId)
    await this.mutate(store, 'interview.create', 'interview', record.id, actor, `${candidate?.name ?? referral.candidateId}｜${record.round}｜${record.when}`)
    return record
  }

  async listInterviews(args: OpArgs): Promise<{ count: number; interviews: Interview[] }> {
    const store = await loadStore(this.dataDir)
    const candidateId = str(args.candidateId)
    const positionId = str(args.positionId)
    const list = store.interviews.filter((i) => (!candidateId || i.candidateId === candidateId) && (!positionId || i.positionId === positionId))
    return { count: list.length, interviews: list }
  }

  // ---- Offer ----

  async createOffer(args: OpArgs, actor: string): Promise<Offer> {
    const store = await loadStore(this.dataDir)
    const referralId = str(args.referralId)
    const referral = store.referrals.find((r) => r.id === referralId)
    if (!referral) throw new Error(`create_offer: 推荐 ${referralId} 不存在`)
    const ts = now()
    const record: Offer = {
      id: randomUUID(),
      referralId,
      candidateId: referral.candidateId,
      positionId: referral.positionId,
      package: str(args.package),
      status: (args.status as OfferStatus) ?? 'sent',
      note: str(args.note),
      createdAt: ts,
      updatedAt: ts,
    }
    if (!OFFER_STATUSES.includes(record.status)) throw new Error(`create_offer: 非法状态 ${record.status}`)
    store.offers.push(record)
    const candidate = store.candidates.find((c) => c.id === referral.candidateId)
    await this.mutate(store, 'offer.create', 'offer', record.id, actor, `${candidate?.name ?? referral.candidateId}｜${record.status}`)
    return record
  }

  async listOffers(args: OpArgs): Promise<{ count: number; offers: Offer[] }> {
    const store = await loadStore(this.dataDir)
    const status = str(args.status)
    const list = store.offers.filter((o) => !status || o.status === status)
    return { count: list.length, offers: list }
  }

  // ---- 删除（需确认） ----

  async deleteEntity(args: OpArgs, actor: string): Promise<{ deleted: boolean; type: string; id: string }> {
    if (args.confirm !== true) throw new Error('delete_entity: 删除需确认——请传入 confirm=true')
    const store = await loadStore(this.dataDir)
    const type = str(args.type) as 'company' | 'candidate' | 'position' | 'referral'
    const id = str(args.id)
    if (!id) throw new Error('delete_entity: id 必填')
    const bucket = store[`${type}s` as 'companies' | 'candidates' | 'positions' | 'referrals']
    const before = bucket.length
    const next = bucket.filter((x) => x.id !== id)
    if (next.length === before) throw new Error(`delete_entity: ${type} ${id} 不存在`)
    store[`${type}s` as 'companies' | 'candidates' | 'positions' | 'referrals'] = next as never
    await this.mutate(store, `${type}.delete`, type, id, actor, `删除 ${type}：${id}`)
    return { deleted: true, type, id }
  }

  // ---- 仪表盘 ----

  async dashboard(): Promise<unknown> {
    const store = await loadStore(this.dataDir)
    const byStage = <T extends { stage: string }>(rows: T[]): { stage: string; count: number }[] => {
      const map = new Map<string, number>()
      for (const r of rows) map.set(r.stage, (map.get(r.stage) ?? 0) + 1)
      return [...map.entries()].map(([stage, count]) => ({ stage, count }))
    }
    return {
      counts: {
        companies: store.companies.length,
        candidates: store.candidates.length,
        positions: store.positions.length,
        openPositions: store.positions.filter((p) => p.status === 'open').length,
        referrals: store.referrals.length,
        interviews: store.interviews.length,
        offers: store.offers.length,
        hired: store.referrals.filter((r) => r.stage === 'hired').length,
      },
      candidatesByStage: byStage(store.candidates),
      referralsByStage: byStage(store.referrals),
      recentActivities: store.activities.slice(-10).reverse(),
      recentAudit: (await this.auditTail(10)).map((a) => ({ ts: a.ts, action: a.action, detail: a.detail })),
    }
  }

  // ---- 跨实体检索 ----

  async search(query: string): Promise<unknown> {
    const q = str(query).toLowerCase()
    if (!q) throw new Error('search: query 必填')
    const store = await loadStore(this.dataDir)
    const hit = (fields: string[]): boolean => fields.some((f) => f.toLowerCase().includes(q))
    const candidates = store.candidates.filter((c) => hit([c.name, c.title, c.company, c.city, ...c.tags])).map((c) => ({ id: c.id, name: c.name, title: c.title, company: c.company, stage: c.stage }))
    const positions = store.positions.filter((p) => hit([p.title, p.client, p.city, p.requirements, p.niceToHave])).map((p) => ({ id: p.id, title: p.title, client: p.client, status: p.status }))
    const companies = store.companies.filter((c) => hit([c.name, c.industry, c.contact])).map((c) => ({ id: c.id, name: c.name, industry: c.industry }))
    const referrals = store.referrals.filter((r) => {
      const c = store.candidates.find((x) => x.id === r.candidateId)
      const p = store.positions.find((x) => x.id === r.positionId)
      return hit([c?.name ?? '', p?.title ?? '', p?.client ?? '', r.note])
    }).map((r) => ({ id: r.id, candidateId: r.candidateId, positionId: r.positionId, stage: r.stage }))
    return { query, counts: { candidates: candidates.length, positions: positions.length, companies: companies.length, referrals: referrals.length }, candidates, positions, companies, referrals }
  }

  // ---- 统一分派（工具与 HTTP mutate 共用） ----

  async run(op: string, args: OpArgs, actor: string): Promise<unknown> {
    const ops: Record<string, (a: OpArgs) => Promise<unknown>> = {
      register_company: (a) => this.registerCompany(a, actor),
      list_companies: (a) => this.listCompanies(a),
      register_candidate: (a) => this.registerCandidate(a, actor),
      list_candidates: (a) => this.listCandidates(a),
      get_candidate: (a) => this.getCandidate(str(a.id)).then((c) => c ?? { notFound: str(a.id) }),
      register_position: (a) => this.registerPosition(a, actor),
      list_positions: (a) => this.listPositions(a),
      get_position: (a) => this.getPosition(str(a.id)).then((p) => p ?? { notFound: str(a.id) }),
      create_referral: (a) => this.createReferral(a, actor),
      update_referral_stage: (a) => this.updateReferralStage(a, actor),
      list_referrals: (a) => this.listReferrals(a),
      add_activity: (a) => this.addActivity(a, actor),
      list_activities: (a) => this.listActivities(a),
      schedule_interview: (a) => this.scheduleInterview(a, actor),
      list_interviews: (a) => this.listInterviews(a),
      create_offer: (a) => this.createOffer(a, actor),
      list_offers: (a) => this.listOffers(a),
      delete_entity: (a) => this.deleteEntity(a, actor),
      dashboard: () => this.dashboard(),
      search: (a) => this.search(str(a.query)),
    }
    const fn = ops[op]
    if (!fn) throw new Error(`未知操作：${op}`)
    return fn(args)
  }
}

// ---------- 工具注册 ----------

const text = (t: string): { type: 'text'; text: string }[] => [{ type: 'text', text: t }]

interface ToolSpec {
  name: string
  description: string
  parameters: Record<string, unknown>
  op: string
  render: (value: unknown) => { type: 'text'; text: string }[]
}

/** 通用对象渲染（登记/详情类返回值） */
function renderRecord(label: string, keys: { key: string; label: string }[]): (v: unknown) => { type: 'text'; text: string }[] {
  return (value) => {
    const v = value as Record<string, unknown>
    if (v.notFound) return text(`${label} ${str(v.notFound)} 不存在`)
    const parts = keys.map((k) => `${k.label}：${v[k.key] === undefined || v[k.key] === null || v[k.key] === '' ? '—' : String(v[k.key])}`)
    return text(`${label} ${str(v.id)}：\n${parts.join('\n')}`)
  }
}

function renderList(label: string, itemKey: string): (v: unknown) => { type: 'text'; text: string }[] {
  return (value) => {
    const v = value as { count: number; [k: string]: unknown }
    if (v.count === 0) return text(`暂无${label}。`)
    const items = (v[itemKey] as Record<string, unknown>[]).map((x) => JSON.stringify(x))
    return text(`共 ${v.count} 条${label}：\n${items.join('\n')}`)
  }
}

const TOOLS: ToolSpec[] = [
  {
    name: 'recruitwb_register_company',
    description: '登记/更新一家客户公司。公司事实（名称/行业/对接人）必须来自输入，不编造；对接人联系方式属敏感信息，默认 confidential=true。',
    parameters: {
      id: { type: 'string', description: '公司 id；不传则新建' },
      name: { type: 'string', required: true, description: '公司名称' },
      industry: { type: 'string', description: '行业' },
      size: { type: 'string', description: '规模' },
      contact: { type: 'string', description: '对接人' },
      contactPhone: { type: 'string', description: '对接人联系方式（保密）' },
      notes: { type: 'string', description: '备注（客户偏好/注意事项）' },
      confidential: { type: 'boolean', description: '是否含保密信息（默认 true）' },
    },
    op: 'register_company',
    render: renderRecord('客户公司', [
      { key: 'name', label: '名称' },
      { key: 'industry', label: '行业' },
      { key: 'contact', label: '对接人' },
      { key: 'updatedAt', label: '更新时间' },
    ]),
  },
  {
    name: 'recruitwb_list_companies',
    description: '列出客户公司台账。可按名称/行业/对接人关键词检索；只返回存储中的事实。',
    parameters: {
      query: { type: 'string', description: '匹配名称/行业/对接人' },
    },
    op: 'list_companies',
    render: renderList('客户公司', 'companies'),
  },
  {
    name: 'recruitwb_register_candidate',
    description:
      '登记/更新一名候选人。候选人资料涉及隐私：只记录必要事实，内容只存本机；' +
      '电话/邮箱/薪资期望等敏感信息请放入对应字段（默认保密），不得编造。' +
      'tags 为技能/关键词标签，便于检索与匹配。',
    parameters: {
      id: { type: 'string', description: '候选人 id；不传则新建' },
      name: { type: 'string', required: true, description: '候选人姓名' },
      title: { type: 'string', description: '当前职位头衔' },
      company: { type: 'string', description: '现公司' },
      city: { type: 'string', description: '城市' },
      phone: { type: 'string', description: '电话（保密）' },
      email: { type: 'string', description: '邮箱（保密）' },
      resume: { type: 'string', description: '简历要点（必要事实，不编造）' },
      salaryExpect: { type: 'string', description: '薪资期望（保密）' },
      tags: { type: 'array', items: { type: 'string' }, description: '技能/关键词标签' },
      stage: { type: 'string', enum: [...CANDIDATE_STAGES], description: '阶段：sourcing/contacted/interviewing/offered/placed/archived' },
      notes: { type: 'string', description: '必要事实备注' },
      confidential: { type: 'boolean', description: '是否含保密信息（默认 true）' },
    },
    op: 'register_candidate',
    render: renderRecord('候选人', [
      { key: 'name', label: '姓名' },
      { key: 'title', label: '职位' },
      { key: 'company', label: '现公司' },
      { key: 'stage', label: '阶段' },
      { key: 'updatedAt', label: '更新时间' },
    ]),
  },
  {
    name: 'recruitwb_list_candidates',
    description: '列出候选人台账。可按阶段过滤或按姓名/职位/公司/标签关键词检索；只返回存储中的事实。',
    parameters: {
      stage: { type: 'string', enum: [...CANDIDATE_STAGES], description: '按阶段过滤' },
      query: { type: 'string', description: '匹配姓名/职位/公司/标签' },
    },
    op: 'list_candidates',
    render: renderList('候选人', 'candidates'),
  },
  {
    name: 'recruitwb_get_candidate',
    description: '读取一名候选人的完整档案（含简历要点/标签/薪资期望等保密字段）。基于存储，不编造。',
    parameters: {
      id: { type: 'string', required: true, description: '候选人 id' },
    },
    op: 'get_candidate',
    render: (value) => {
      const v = value as Record<string, unknown>
      if (v.notFound) return text(`候选人 ${str(v.notFound)} 不存在`)
      return text(`候选人 ${str(v.name)}：\n职位：${str(v.title)}｜现公司：${str(v.company)}｜城市：${str(v.city)}\n电话：${str(v.phone)}｜邮箱：${str(v.email)}\n薪资期望（保密）：${str(v.salaryExpect)}\n标签：${Array.isArray(v.tags) ? (v.tags as string[]).join(', ') : '—'}\n阶段：${str(v.stage)}\n简历要点：${str(v.resume)}\n备注：${str(v.notes)}`)
    },
  },
  {
    name: 'recruitwb_register_position',
    description:
      '登记/更新一个职位需求。职位事实（客户/要求/薪资）必须来自输入，不编造；' +
      '薪资区间属保密信息，默认 confidential 字段不落盘（仅存 salaryRange，UI 与工具按保密处理）。',
    parameters: {
      id: { type: 'string', description: '职位 id；不传则新建' },
      client: { type: 'string', required: true, description: '客户公司' },
      title: { type: 'string', required: true, description: '职位名称' },
      city: { type: 'string', description: '工作地点' },
      quantity: { type: 'integer', description: '招聘人数（默认 1）' },
      requirements: { type: 'string', description: '硬性要求' },
      niceToHave: { type: 'string', description: '软性要求/加分项' },
      salaryRange: { type: 'string', description: '薪资区间（保密）' },
      status: { type: 'string', enum: [...POSITION_STATUSES], description: '状态：open/paused/closed' },
      notes: { type: 'string', description: '备注' },
    },
    op: 'register_position',
    render: renderRecord('职位', [
      { key: 'client', label: '客户' },
      { key: 'title', label: '职位' },
      { key: 'status', label: '状态' },
      { key: 'updatedAt', label: '更新时间' },
    ]),
  },
  {
    name: 'recruitwb_list_positions',
    description: '列出职位需求。可按客户/状态过滤或按职位名/要求关键词检索。',
    parameters: {
      client: { type: 'string', description: '按客户公司过滤' },
      status: { type: 'string', enum: [...POSITION_STATUSES], description: '按状态过滤' },
      query: { type: 'string', description: '匹配职位名/客户/要求' },
    },
    op: 'list_positions',
    render: renderList('职位', 'positions'),
  },
  {
    name: 'recruitwb_get_position',
    description: '读取一个职位的完整需求（含硬性/软性要求、薪资区间等）。基于存储，不编造。',
    parameters: {
      id: { type: 'string', required: true, description: '职位 id' },
    },
    op: 'get_position',
    render: (value) => {
      const v = value as Record<string, unknown>
      if (v.notFound) return text(`职位 ${str(v.notFound)} 不存在`)
      return text(`职位 ${str(v.title)}｜${str(v.client)}：\n地点：${str(v.city)}｜人数：${str(v.quantity)}｜状态：${str(v.status)}\n薪资区间（保密）：${str(v.salaryRange)}\n硬性要求：${str(v.requirements)}\n软性/加分：${str(v.niceToHave)}\n备注：${str(v.notes)}`)
    },
  },
  {
    name: 'recruitwb_create_referral',
    description:
      '创建一条推荐：把候选人推荐到职位，进入推荐流水线（初始阶段 recommended 已推荐）。' +
      'candidateId 与 positionId 必须已存在；推荐事实基于存储，不编造。',
    parameters: {
      candidateId: { type: 'string', required: true, description: '候选人 id' },
      positionId: { type: 'string', required: true, description: '职位 id' },
      note: { type: 'string', description: '推荐说明' },
    },
    op: 'create_referral',
    render: renderRecord('推荐', [
      { key: 'candidateId', label: '候选人' },
      { key: 'positionId', label: '职位' },
      { key: 'stage', label: '阶段' },
      { key: 'createdAt', label: '创建时间' },
    ]),
  },
  {
    name: 'recruitwb_update_referral_stage',
    description:
      '推进推荐流水线阶段。状态机（对齐 md-agent）：recommended 已推荐 → pending_client 待客户反馈 → interviewing 面试中 → offer_sent 已发Offer → hired 已入职；' +
      'rejected 已拒绝 / withdrawn 已撤回 为终态可直达。只允许推进链下一步或直达终态，不可跳级；' +
      '阶段变更必须基于真实沟通/面试进展，不得编造。',
    parameters: {
      referralId: { type: 'string', required: true, description: '推荐 id' },
      stage: { type: 'string', required: true, enum: [...REFERRAL_STAGES], description: '新阶段' },
      note: { type: 'string', description: '进展说明' },
    },
    op: 'update_referral_stage',
    render: renderRecord('推荐', [
      { key: 'stage', label: '新阶段' },
      { key: 'updatedAt', label: '更新时间' },
    ]),
  },
  {
    name: 'recruitwb_add_activity',
    description:
      '记录一条沟通/活动留痕（与客户或候选人的重要沟通、面试反馈、Offer 进展等）。' +
      '涉敏信息请保留 confidential=true（默认）；事实基于真实发生，不编造。',
    parameters: {
      kind: { type: 'string', enum: [...ACTIVITY_KINDS], description: '类型：comm 沟通/interview 面试/offer/note 备注/system 系统' },
      targetType: { type: 'string', required: true, enum: [...ACTIVITY_TARGET_TYPES], description: '关联对象类型' },
      targetId: { type: 'string', required: true, description: '关联对象 id' },
      text: { type: 'string', required: true, description: '留痕内容' },
      confidential: { type: 'boolean', description: '是否含保密信息（默认 true）' },
    },
    op: 'add_activity',
    render: renderRecord('活动', [
      { key: 'kind', label: '类型' },
      { key: 'targetType', label: '对象' },
      { key: 'targetId', label: '对象 id' },
      { key: 'createdAt', label: '时间' },
    ]),
  },
  {
    name: 'recruitwb_list_activities',
    description: '列出活动/沟通留痕。可按目标对象过滤；倒序返回最近 200 条。',
    parameters: {
      targetType: { type: 'string', enum: [...ACTIVITY_TARGET_TYPES], description: '按对象类型过滤' },
      targetId: { type: 'string', description: '按对象 id 过滤' },
    },
    op: 'list_activities',
    render: renderList('活动', 'activities'),
  },
  {
    name: 'recruitwb_schedule_interview',
    description: '为一条推荐安排一场面试（一面/二面/终面等）。referralId 必须已存在；时间 when 必填。',
    parameters: {
      referralId: { type: 'string', required: true, description: '推荐 id' },
      round: { type: 'string', description: '轮次（默认「面试」）' },
      when: { type: 'string', required: true, description: '时间：ISO 或自由文本' },
      mode: { type: 'string', description: '方式：onsite/video/phone（默认 video）' },
      note: { type: 'string', description: '备注' },
    },
    op: 'schedule_interview',
    render: renderRecord('面试', [
      { key: 'referralId', label: '推荐' },
      { key: 'round', label: '轮次' },
      { key: 'when', label: '时间' },
      { key: 'mode', label: '方式' },
    ]),
  },
  {
    name: 'recruitwb_list_interviews',
    description: '列出面试安排。可按候选人/职位过滤。',
    parameters: {
      candidateId: { type: 'string', description: '按候选人过滤' },
      positionId: { type: 'string', description: '按职位过滤' },
    },
    op: 'list_interviews',
    render: renderList('面试', 'interviews'),
  },
  {
    name: 'recruitwb_create_offer',
    description: '为一条推荐创建 Offer。薪酬包属保密信息（package 字段）；基于真实沟通，不编造。',
    parameters: {
      referralId: { type: 'string', required: true, description: '推荐 id' },
      package: { type: 'string', description: '薪酬包（保密）' },
      status: { type: 'string', enum: [...OFFER_STATUSES], description: '状态：draft/sent/accepted/declined（默认 sent）' },
      note: { type: 'string', description: '备注' },
    },
    op: 'create_offer',
    render: renderRecord('Offer', [
      { key: 'referralId', label: '推荐' },
      { key: 'status', label: '状态' },
      { key: 'createdAt', label: '创建时间' },
    ]),
  },
  {
    name: 'recruitwb_list_offers',
    description: '列出 Offer。可按状态过滤。',
    parameters: {
      status: { type: 'string', enum: [...OFFER_STATUSES], description: '按状态过滤' },
    },
    op: 'list_offers',
    render: renderList('Offer', 'offers'),
  },
  {
    name: 'recruitwb_delete',
    description: '删除一条记录（客户公司/候选人/职位/推荐）。删除是破坏性操作，必须显式传入 confirm=true；删除会写入审计。',
    parameters: {
      type: { type: 'string', required: true, enum: ['company', 'candidate', 'position', 'referral'], description: '记录类型' },
      id: { type: 'string', required: true, description: '记录 id' },
      confirm: { type: 'boolean', required: true, description: '确认删除，必须为 true' },
    },
    op: 'delete_entity',
    render: renderRecord('删除', [
      { key: 'type', label: '类型' },
      { key: 'id', label: 'id' },
    ]),
  },
  {
    name: 'recruitwb_dashboard',
    description: '工作台统计：各实体数量、候选人/推荐阶段分布漏斗、最近活动与最近审计。基于存储，不编造。',
    parameters: {},
    op: 'dashboard',
    render: (value) => {
      const v = value as Record<string, unknown>
      const counts = v.counts as Record<string, number>
      const lines = [
        `候选人 ${counts.candidates}｜职位 ${counts.positions}（open ${counts.openPositions}）｜推荐 ${counts.referrals}｜已入职 ${counts.hired}｜面试 ${counts.interviews}｜Offer ${counts.offers}`,
      ]
      const cand = v.candidatesByStage as { stage: string; count: number }[]
      const ref = v.referralsByStage as { stage: string; count: number }[]
      if (cand.length) lines.push(`候选人漏斗：${cand.map((x) => `${x.stage} ${x.count}`).join(' / ')}`)
      if (ref.length) lines.push(`推荐漏斗：${ref.map((x) => `${x.stage} ${x.count}`).join(' / ')}`)
      const recent = v.recentActivities as { kind: string; text: string; createdAt: string }[]
      if (recent.length) lines.push(`最近活动：\n${recent.map((a) => `- [${a.kind}] ${a.text}（${a.createdAt}）`).join('\n')}`)
      return text(lines.join('\n'))
    },
  },
  {
    name: 'recruitwb_search',
    description: '跨实体检索：一次查询同时匹配候选人（姓名/职位/公司/标签）、职位（名称/客户/要求）、客户公司与推荐。基于存储，不编造。',
    parameters: {
      query: { type: 'string', required: true, description: '检索关键词' },
    },
    op: 'search',
    render: (value) => {
      const v = value as Record<string, unknown>
      const counts = v.counts as Record<string, number>
      const parts = [`命中：候选人 ${counts.candidates}｜职位 ${counts.positions}｜公司 ${counts.companies}｜推荐 ${counts.referrals}`]
      for (const key of ['candidates', 'positions', 'companies', 'referrals']) {
        const rows = v[key] as Record<string, unknown>[]
        if (rows.length) parts.push(`${key}：\n${rows.map((r) => JSON.stringify(r)).join('\n')}`)
      }
      return text(parts.join('\n'))
    },
  },
]

// ---------- 插件入口 ----------

export function apply(ctx: Context, config: Config) {
  const dataDir = config.dataDir ? expandHomePath(config.dataDir) : dshHomePath('recruit-workbench')
  const wb = new Workbench(dataDir)

  for (const spec of TOOLS) {
    ctx.tools.register(defineTool({
      name: spec.name,
      description: spec.description,
      parameters: spec.parameters as never,
      output: {
        schema: { type: 'json' },
        render: (_args, value) => spec.render(value),
      },
      execute: async (args) => wb.run(spec.op, args as Record<string, unknown>, 'tool'),
    }))
  }

  // 浏览器工作台 API（webServer 就绪后注册；headless 下永不就绪，工具面不受影响）
  ctx.inject(['webServer'], (ctx2: Context) => {
    const sendJson = (res: ServerResponse, status: number, body: unknown): void => {
      res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' })
      res.end(JSON.stringify(body))
    }
    const readBody = (req: IncomingMessage): Promise<string> => new Promise((resolve, reject) => {
      const chunks: Buffer[] = []
      req.on('data', (c: Buffer) => chunks.push(c))
      req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
      req.on('error', reject)
    })

    ctx2.effect(() => ctx2.webServer.register({
      kind: 'prefix',
      path: '/api/recruit-workbench',
      handler: async (req, res) => {
        try {
          const url = new URL(req.url ?? '/', 'http://localhost')
          if (req.method === 'GET' && url.pathname === '/api/recruit-workbench/state') {
            sendJson(res, 200, { ok: true, data: await wb.snapshot() })
            return
          }
          if (req.method === 'GET' && url.pathname === '/api/recruit-workbench/audit') {
            sendJson(res, 200, { ok: true, data: await wb.auditTail(50) })
            return
          }
          if (req.method === 'POST' && url.pathname === '/api/recruit-workbench/mutate') {
            const raw = await readBody(req)
            let body: { op?: string; args?: Record<string, unknown> }
            try {
              body = JSON.parse(raw) as { op?: string; args?: Record<string, unknown> }
            } catch {
              sendJson(res, 400, { ok: false, error: '请求体不是合法 JSON' })
              return
            }
            if (!body.op) {
              sendJson(res, 400, { ok: false, error: '缺少 op 字段' })
              return
            }
            const result = await wb.run(body.op, body.args ?? {}, 'ui')
            sendJson(res, 200, { ok: true, result, state: await wb.snapshot() })
            return
          }
          sendJson(res, 404, { ok: false, error: `未找到路由：${url.pathname}` })
        } catch (error) {
          sendJson(res, 500, { ok: false, error: error instanceof Error ? error.message : String(error) })
        }
      },
    }), 'recruit-workbench: web api')
    console.log('[recruit-workbench] web api on')
  })

  console.log(`[recruit-workbench] plugin loaded! dataDir=${dataDir} tools=${TOOLS.length}`)
}
