/**
 * workbench —— 可自定义工作台插件（MVP 骨架）
 *
 * 一个通用化的工作台：items（待办/笔记/任意条目）为示例领域，证明
 * 「工具面（AI 对话）与页面 API 双通道同源」跑通。业务逻辑封装在
 * Workbench 类，工具 execute 与页面 mutate 都调 wb.run → 一份逻辑、三处调用。
 *
 * 三件套：
 *   ① Workbench 类——业务核心 + store.json 原子写 + 审计
 *   ② TOOLS 数组——声明式工具定义，循环注册进 ctx.tools
 *   ③ webServer 路由——/workbench 页面 + /api/workbench state/mutate
 *
 * 远程 MCP：不写代码。在 cordis.patch.yml 声明 @deepseek-ai/dsh-mcp-client
 * 实例后，远程工具自动注册进 ctx.tools，AI 对话与工作台均可调用。
 *
 * 加载（开发）：
 *   dsh web --patch <本目录>/cordis.yml --host 127.0.0.1 --port <port>
 *   访问 http://127.0.0.1:<port>/workbench 即工作台页面
 *
 * 框架边界（MVP 最小必要，勿过度开发）：
 *   做：run/snapshot/原子写/审计、2-3 示例工具、页面+API、Config(dataDir)
 *   不做：多用户/权限、复杂 UI 框架、内置 agent loop、自建 MCP client
 */

import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import type { Context } from '@deepseek-ai/cordis'
import z from '@deepseek-ai/schemastery'
import { defineTool } from '@deepseek-ai/dsh-tools'
import { dshHomePath, expandHomePath } from '@deepseek-ai/dsh-home-paths'
import type { WbHttpRequest, WbHttpResponse } from './dsh-host.js'

export const name = 'md-studio'
export const inject = ['tools']

// ---------- 插件配置 ----------

/** 插件配置：数据目录可覆盖；默认 `$DSH_HOME/workbench`。 */
export interface Config {
  /** 数据目录；空字符串 = 默认 `$DSH_HOME/workbench`。 */
  dataDir: string
}

export const Config = z.object({
  dataDir: z.string().default(''),
})

// ---------- 领域模型 ----------

/** 工作台条目（通用化示例领域） */
export interface Item {
  id: string
  title: string
  content: string
  tags: string[]
  done: boolean
  createdAt: string
  updatedAt: string
}

/** 审计条目 */
export interface AuditEntry {
  id: string
  ts: string
  action: string
  targetId: string
  actor: string
  detail: string
}

/** 持久化存储 */
export interface Store {
  items: Item[]
  audit: AuditEntry[]
}

const AUDIT_LIMIT = 500

function emptyStore(): Store {
  return { items: [], audit: [] }
}

// ---------- 辅助 ----------

const now = (): string => new Date().toISOString()
const str = (v: unknown): string => (v == null ? '' : String(v).trim())
const text = (t: string): { type: 'text'; text: string }[] => [{ type: 'text', text: t }]

async function loadStore(dataDir: string): Promise<Store> {
  await mkdir(dataDir, { recursive: true })
  const path = join(dataDir, 'store.json')
  try {
    const raw = await readFile(path, 'utf8')
    const parsed = JSON.parse(raw) as Partial<Store>
    return {
      items: Array.isArray(parsed.items) ? parsed.items : [],
      audit: Array.isArray(parsed.audit) ? parsed.audit : [],
    }
  } catch {
    return emptyStore()
  }
}

/** 原子写：先写 .tmp 再 rename，防半写损坏 */
async function saveStore(dataDir: string, store: Store): Promise<void> {
  const path = join(dataDir, 'store.json')
  const tmp = join(dataDir, 'store.json.tmp')
  await writeFile(tmp, JSON.stringify(store, null, 2), 'utf8')
  await rename(tmp, path)
}

// ---------- 工作台业务核心 ----------

export type OpArgs = Record<string, unknown>

export class Workbench {
  readonly dataDir: string

  constructor(dataDir: string) {
    this.dataDir = dataDir
  }

  /** 写操作统一入口：改 store + 落审计 + 原子写 */
  private async mutate(
    store: Store,
    action: string,
    targetId: string,
    actor: string,
    detail: string,
  ): Promise<void> {
    store.audit.push({ id: randomUUID(), ts: now(), action, targetId, actor, detail })
    if (store.audit.length > AUDIT_LIMIT) store.audit = store.audit.slice(-AUDIT_LIMIT)
    await saveStore(this.dataDir, store)
  }

  /** 深度拷贝快照（审计不随快照暴露给 UI，避免无限膨胀） */
  async snapshot(): Promise<Store> {
    const store = await loadStore(this.dataDir)
    return { items: store.items, audit: [] }
  }

  async auditTail(limit = 30): Promise<AuditEntry[]> {
    const store = await loadStore(this.dataDir)
    return store.audit.slice(-limit)
  }

  // ---- items ----

  async createItem(args: OpArgs, actor: string): Promise<Item> {
    const store = await loadStore(this.dataDir)
    const ts = now()
    const tags = Array.isArray(args.tags) ? args.tags.map((t) => str(t)).filter(Boolean) : []
    const record: Item = {
      id: args.id && str(args.id) ? str(args.id) : randomUUID(),
      title: str(args.title),
      content: str(args.content),
      tags,
      done: args.done === true,
      createdAt: ts,
      updatedAt: ts,
    }
    if (!record.title) throw new Error('create_item: title 必填且不能为空')
    store.items.push(record)
    await this.mutate(store, 'item.create', record.id, actor, `创建：${record.title}`)
    return record
  }

  async listItems(args: OpArgs): Promise<{ count: number; items: Item[] }> {
    const store = await loadStore(this.dataDir)
    const q = str(args.query).toLowerCase()
    const tag = str(args.tag)
    let list = store.items
    if (q) list = list.filter((i) => i.title.toLowerCase().includes(q) || i.content.toLowerCase().includes(q))
    if (tag) list = list.filter((i) => i.tags.includes(tag))
    if (args.done !== undefined) list = list.filter((i) => i.done === (args.done === true))
    return { count: list.length, items: list }
  }

  async getItem(args: OpArgs): Promise<Item | { notFound: string }> {
    const store = await loadStore(this.dataDir)
    const id = str(args.id)
    const item = store.items.find((i) => i.id === id)
    return item ?? { notFound: id }
  }

  async updateItem(args: OpArgs, actor: string): Promise<Item> {
    const store = await loadStore(this.dataDir)
    const id = str(args.id)
    const i = store.items.findIndex((it) => it.id === id)
    if (i < 0) throw new Error(`update_item: 未找到 ${id}`)
    const item = store.items[i]
    if (args.title !== undefined) item.title = str(args.title)
    if (args.content !== undefined) item.content = str(args.content)
    if (Array.isArray(args.tags)) item.tags = args.tags.map((t) => str(t)).filter(Boolean)
    if (args.done !== undefined) item.done = args.done === true
    item.updatedAt = now()
    store.items[i] = item
    await this.mutate(store, 'item.update', item.id, actor, `更新：${item.title}`)
    return item
  }

  /** 统一操作分发器：工具面与页面 API 都调它，保证双通道同源 */
  async run(op: string, args: OpArgs, actor: string): Promise<unknown> {
    const ops: Record<string, (a: OpArgs) => Promise<unknown>> = {
      create_item: (a) => this.createItem(a, actor),
      list_items: (a) => this.listItems(a),
      get_item: (a) => this.getItem(a),
      update_item: (a) => this.updateItem(a, actor),
    }
    const fn = ops[op]
    if (!fn) throw new Error(`未知操作：${op}（可用：${Object.keys(ops).join(', ')}）`)
    return fn(args)
  }
}

// ---------- 声明式工具定义 ----------

interface ToolSpec {
  name: string
  description: string
  parameters: Record<string, unknown>
  op: string
  render: (value: unknown) => { type: 'text'; text: string }[]
}

const TOOLS: ToolSpec[] = [
  {
    name: 'wb_create_item',
    description: '在工作台创建一个条目（待办/笔记/任意条目）。',
    parameters: {
      title: { type: 'string', required: true, description: '标题' },
      content: { type: 'string', description: '正文内容' },
      tags: { type: 'array', items: { type: 'string' }, description: '标签列表' },
      done: { type: 'boolean', description: '是否完成' },
    },
    op: 'create_item',
    render: (v) => {
      const i = v as Item
      return text(`已创建条目 ${i.id}：${i.title}`)
    },
  },
  {
    name: 'wb_list_items',
    description: '列出工作台条目（可按关键词/标签/完成态过滤）。',
    parameters: {
      query: { type: 'string', description: '标题或正文关键词' },
      tag: { type: 'string', description: '标签精确匹配' },
      done: { type: 'boolean', description: '按完成态过滤' },
    },
    op: 'list_items',
    render: (v) => {
      const r = v as { count: number; items: Item[] }
      if (!r.items.length) return text('无匹配条目')
      const lines = [`共 ${r.count} 条：`]
      for (const i of r.items) {
        const mark = i.done ? '[x]' : '[ ]'
        const tags = i.tags.length ? ` #${i.tags.join(' #')}` : ''
        lines.push(`${mark} ${i.title}${tags}（${i.id}）`)
      }
      return text(lines.join('\n'))
    },
  },
  {
    name: 'wb_get_item',
    description: '获取单个工作台条目的详情。',
    parameters: {
      id: { type: 'string', required: true, description: '条目 id' },
    },
    op: 'get_item',
    render: (v) => {
      const i = v as Item | { notFound: string }
      if ('notFound' in i) return text(`未找到条目：${i.notFound}`)
      return text(
        `${i.title}${i.done ? ' [已完成]' : ''}\n${i.content}\n标签：${i.tags.join(', ') || '无'}\n创建：${i.createdAt}\n更新：${i.updatedAt}`,
      )
    },
  },
]

// ---------- 插件入口 ----------

export function apply(ctx: Context, config: Config) {
  const dataDir = config.dataDir ? expandHomePath(config.dataDir) : dshHomePath('md-studio')
  const wb = new Workbench(dataDir)

  // ① 注册工具（AI 对话可见；execute 调 wb.run → 与页面同源）
  for (const spec of TOOLS) {
    ctx.tools.register(
      defineTool({
        name: spec.name,
        description: spec.description,
        parameters: spec.parameters as never,
        output: {
          schema: { type: 'string' },
          render: (_args, value) => spec.render(value),
        },
        execute: async (args) =>
          (await wb.run(spec.op, args as Record<string, unknown>, 'tool')) as string,
      }),
    )
  }

  // ② 浏览器工作台（webServer 就绪后注册；headless 下永不就绪，工具面不受影响）
  ctx.inject(['webServer'], (ctx2: Context) => {
    const sendJson = (res: WbHttpResponse, status: number, body: unknown): void => {
      res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' })
      res.end(JSON.stringify(body))
    }
    const readBody = (req: WbHttpRequest): Promise<string> =>
      new Promise((resolve, reject) => {
        const chunks: Buffer[] = []
        req.on('data', (c: Uint8Array) => chunks.push(Buffer.from(c)))
        req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
        req.on('error', reject)
      })

    // 工作台页面
    ctx2.effect(() =>
      ctx2.webServer.register({
        kind: 'exact',
        path: '/workbench',
        handler: async (_req, res) => {
          try {
            const html = await readFile(
              fileURLToPath(new URL('./workbench.html', import.meta.url)),
              'utf8',
            )
            res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
            res.end(html)
          } catch (e) {
            res.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' })
            res.end(`工作台页面读取失败: ${e instanceof Error ? e.message : e}`)
          }
        },
      }),
    )

    // 工作台 API
    ctx2.effect(() =>
      ctx2.webServer.register({
        kind: 'prefix',
        path: '/api/workbench',
        handler: async (req, res) => {
          try {
            const url = new URL(req.url ?? '/', 'http://localhost')
            // GET /api/workbench/state → 快照
            if (req.method === 'GET' && url.pathname === '/api/workbench/state') {
              sendJson(res, 200, { ok: true, data: await wb.snapshot() })
              return
            }
            // GET /api/workbench/audit → 审计尾部
            if (req.method === 'GET' && url.pathname === '/api/workbench/audit') {
              sendJson(res, 200, { ok: true, data: await wb.auditTail(50) })
              return
            }
            // POST /api/workbench/mutate { op, args } → wb.run（与工具同源）
            if (req.method === 'POST' && url.pathname === '/api/workbench/mutate') {
              const raw = await readBody(req)
              let body: { op?: string; args?: OpArgs }
              try {
                body = JSON.parse(raw) as { op?: string; args?: OpArgs }
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
            sendJson(res, 500, {
              ok: false,
              error: error instanceof Error ? error.message : String(error),
            })
          }
        },
      }),
    )
  })

  console.log(`[md-studio] plugin loaded! dataDir=${dataDir} tools=${TOOLS.length}`)
}
