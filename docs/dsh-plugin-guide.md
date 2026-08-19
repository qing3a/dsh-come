# dsh 官方插件模式速查（本地化参考）

> 来源：官方文档站 [develop/basic](https://deepseek-harness.github.io/deepseek-harness/develop/basic/)（仓库内 `docs/user/develop/basic/`：`index.md` / `tool.md` / `config.md`，均含 `.zh.md` 中文版）。
> 用途：开发猎头工作台插件前的起点；本文件是浓缩，细节以官方文档为准。
> 本文档按官方现状整理（2026-08-14），upstream 变化时更新本文件。

## 1. 插件是什么

插件是一个 TypeScript 模块，导出 `apply` 函数。框架加载插件时调用 `apply` 并传入 `ctx`（Cordis Context），插件通过 `ctx` 注册能力（事件、工具、定时器等）：

```ts
import type { Context } from '@deepseek-ai/cordis'

export const name = 'my-plugin'

export function apply(ctx: Context) {
  // 在这里注册能力
}
```

## 2. 最小可跑示例（本地 overlay）

1. 建本地项目（官方仓库 checkout 内，或任何可解析目录）：

```sh
mkdir -p scratch-plugin/src
```

2. 写插件 `scratch-plugin/src/my-plugin.ts`：

```ts
import type { Context } from '@deepseek-ai/cordis'

export const name = 'hello-plugin'

export function apply(ctx: Context) {
  console.log('[hello-plugin] plugin loaded!')
}
```

3. 写 `scratch-plugin/cordis.yml`（Web overlay，插入本地插件；**路径必须绝对路径**，patch 不改变 profile 目录的模块解析根）：

```yaml
- insert:
    - id: hello
      name: '/absolute/path/to/scratch-plugin/src/my-plugin.ts'
```

4. 带 overlay 启动：

```sh
pnpm dsh web --patch ./scratch-plugin/cordis.yml
```

打开 `http://127.0.0.1:3080`，启动日志出现 `[hello-plugin] plugin loaded!` 即加载成功。

## 3. 三种插件形态

| 形态 | 写法 | 适用 |
|---|---|---|
| 函数 | `export function apply(ctx)` | 大多数情况 |
| 对象 | `export default { name, inject, apply(ctx) }` | 同函数 |
| 类 | `export default class X extends Service`（构造器里 `super(ctx, 'x')` 同步初始化） | 给其他插件提供服务时 |

## 4. 声明依赖（inject）

插件用到 `tools` / `llm` 等服务时，在 `inject` 里声明，框架等依赖就绪后才加载插件：

```ts
import type { Context } from '@deepseek-ai/cordis'

export const name = 'my-tool-plugin'
export const inject = ['tools']

export function apply(ctx: Context) {
  ctx.tools.register(/* ... */)
}
```

## 5. 注册工具（defineTool）

```ts
import type { Context } from '@deepseek-ai/cordis'
import { defineTool } from '@deepseek-ai/dsh-tools'

export const name = 'greet-tool'
export const inject = ['tools']

export function apply(ctx: Context) {
  ctx.tools.register(defineTool({
    name: 'greet',
    description: 'Greet someone by name.',
    parameters: {
      name: { type: 'string', required: true, description: 'The name to greet' },
    },
    output: {
      schema: { type: 'string' },
      render: (_args, value) => [{ type: 'text', text: value }],
    },
    async execute(args) {
      return `Hello, ${args.name}!`
    },
  }))
}
```

要点：`defineTool` 从 `parameters` 推导并校验 `args`；`execute` 返回 `output.schema` 声明的规范值；`output.render` 把值转成模型可见内容。
进阶参考：官方 cookbook `docs/cookbook/adding-a-tool.md`（嵌套 schema、规范值、后台工作、policy hooks、Code Mode、UI 卡片）。

## 6. 插件配置（Config + Schemastery）

导出 `Config` 类型与同名 Schemastery schema（**不要**导出普通对象，需实现 Standard Schema 接口），默认值直接写在 schema 字段上：

```ts
import type { Context } from '@deepseek-ai/cordis'
import Schema from '@deepseek-ai/schemastery'

export const name = 'my-plugin'

export interface Config {
  greeting: string
  maxRetries: number
  verbose?: boolean
}

export const Config: Schema<Config> = Schema.object({
  greeting: Schema.string().default('Hello'),
  maxRetries: Schema.number().default(3),
  verbose: Schema.boolean().default(false),
})

export function apply(ctx: Context, config: Config) {
  console.log(config.greeting) // 用户值或 schema 默认值
}
```

`cordis.yml` 里给插件传配置：

```yaml
- insert:
    - id: hello
      name: './src/my-plugin.ts'
      config:
        greeting: 'Hi there'
        maxRetries: 5
```

设计原则：
- **任何两个部署想设不同的值都必须做成配置字段**（判据：不改代码、改 cordis.yml 能否改变行为）；
- 自包含约束写进 schema，配置非法就在加载期报错（服务引用类约束走依赖注入）。

## 7. 自动清理与 ctx.effect

通过 `ctx` 注册的一切（事件监听、工具、定时器）在插件卸载时自动清理，无需手动 removeListener/clearInterval。
需要显式释放的资源（如网络连接）用 `ctx.effect()` 提供 disposer：

```ts
export function apply(ctx: Context) {
  ctx.effect(() => {
    const timer = setInterval(() => console.log('heartbeat'), 5000)
    return () => clearInterval(timer) // 插件卸载时执行
  })
}
```

## 8. HMR

配置热改会热替换插件：框架先卸载旧实例再加载新实例。因为注册都是 effect、会自我清理，替换不会残留旧实例的注册。

## 9. 发布与安装

官方流程：`publish.md`（`docs/user/develop/basic/publish.md`）—— 把插件打成可安装包。安装后走 dsh profile 的 pnpm 管理（对应本项目契约 C5：`dsh plugin --profile web <pnpm args>`）。

## 10. 对猎头工作台的落地建议

- **拆分插件**：按能力边界拆成多个插件（如 `recruit-tools` 业务工具、`recruit-config` 配置与 schema、`recruit-services` 公共服务），每个插件职责单一、依赖声明清晰，便于 HMR 迭代与按需装/卸。
- **开发期**：cordis.yml overlay + `dsh web --patch`（决策 1 的壳已用系统 dsh 起 web，加 `--patch` 指向本地插件目录即可）。
- **发布期**：打成安装包，进壳的「插件市场」（内置 ✓已验证 清单）一键装/卸。
- **业务规则**：猎头工作台的数据模型与规则以 `docs/memory.md` 决策 2 为准（继承 md-agent headhunter 模板的隔离/隐私/保密/不编造规则）。

## 11. 项目骨架：plugins/recruit-tools（本机验证通过）

第一个猎头工作台插件已落地在 `plugins/recruit-tools/`（7 个工具：候选人/职位/推荐流水线），
本机（Windows + dsh 0.1.0-rc.6）已验证可加载。

```powershell
# 冒烟验证过的加载命令（隔离 home 起在临时端口；生产用壳的系统 dsh + --patch）
$env:DSH_HOME = '<临时隔离目录>\home'
dsh web --patch C:/Users/Administrator/Desktop/dsh-come/plugins/recruit-tools/cordis.yml --host 127.0.0.1 --port 3199
```

启动日志出现 `[recruit-tools] plugin loaded!` 且 `http://127.0.0.1:<port>/` 返回 200 即成功。

### Windows 踩坑（本机实测记录）

1. **插件路径必须是 file:// URL**：cordis.yml 的 `name` 写 `file:///C:/Users/.../src/index.ts`。
   裸 `C:/...` 被 ESM loader 当成协议 `c:`，报 `ERR_UNSUPPORTED_ESM_URL_SCHEME`（`--dump-config` 不报错，只有真实加载才暴露）。
2. **源码别重复导出**：`export const name` 之后不要再 `export { name, ... }`，报 `Duplicate export of 'name'`。
   官方发布包 JS 里那样写是给 bundler 的元数据，TS 源码不需要。
3. **依赖解析**：插件在仓库内、依赖在 dsh 的 node_modules 时，建 junction 指向同一份 node_modules
   （`New-Item -ItemType Junction`），确保与 harness 共享同一个 Cordis 实例，避免类型/服务双实例。
4. **验证顺序**：先 `dsh web --patch <yml> --dump-config`（静态组合校验，快），再隔离 DSH_HOME + 临时端口真实加载。
5. **输出 schema 限制**：`output.schema` 的 `additionalProperties` 只接受显式布尔（`true`/`false`），
   不接受子 schema 对象（报 `must be explicitly true or false`）；需要键值结构时改用数组项（如 `{stage,count}`）。
7. **插件可挂 HTTP 路由（工作台界面）**：`inject: ['webServer']` 后 `ctx.webServer.register({ kind: 'exact'|'prefix', path, handler(req,res) })`
   可给 dsh web 进程加路由（如 `/recruit` 页面 + `/recruit/api` JSON 接口），页面与 AI 工具共用同一 store；
   这是本地 overlay 插件也能交付完整 UI 的正道（client 插件需发布安装才被发现）。
8. **UI 两层**：工具级富卡片（`presentCall`/`presentResult`，本地 overlay 插件可用，已实现）与
   完整 client 插件（React + package.json `dsh.client.inject` + `exports["./client"]`，**需发布安装进 profile
   才被浏览器端发现**，本地 cordis.yml 插入的 .ts 无 package.json 不生效）——完整工作台视图走后者，待做。

### 骨架结构

```
plugins/recruit-tools/
├── src/index.ts      # 插件：apply(ctx, config) + 7 个 defineTool
├── package.json      # 发布形态（对齐官方 dsh-tool-todo）
├── tsconfig.json     # tsc 构建到 lib/
├── cordis.yml        # 开发 overlay（file:// URL）
└── README.md         # 加载/开发说明
```

## 12. 完整版猎头工作台：plugins/recruit-workbench（client 插件形态，已验证）

`recruit-workbench` 是「client 插件」形态的完整落地（对应 §11 第 8 条的「待做」），2026-08-14 已装入
web profile 并通过临时端口冒烟。结构与本节的坑位直接相关：

```
plugins/recruit-workbench/
├── src/index.ts      # HOST 半：19 个 recruitwb_* 工具 + /api/recruit-workbench/* 路由 + 审计
├── lib/client.js     # CLIENT 半：手写 ModuleLoader bundle（无构建工具）
├── package.json      # 关键：dsh.client {platform:'web', inject:[...]} + exports["./client"]
├── cordis.yml        # 开发 overlay（只加载 host 半；UI 需安装进 profile）
└── README.md
```

### 安装流程（client 半必须走这里）

```powershell
dsh plugin --profile web add C:/Users/Administrator/Desktop/dsh-come/plugins/recruit-workbench
# cordis.patch.yml 加行：name: 'recruit-workbench'（包名，供 clientModules 发现 dsh.client）
```

clientModules 只扫描 **profile node_modules 里可解析**（`require.resolve(<pkg>/package.json)`）的包，
本地 `--patch` overlay 的 file:// 行不算 —— 所以 UI 必须装进 profile。装完重启 GUI 生效。

### 坑位追加（本机实测）

9. **Node 24 原生 TS（host 免编译）**：`main: 'src/index.ts'` 直载，但 type stripping 不支持
   参数属性 `constructor(public x)` / enums / namespaces / 装饰器（`ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`）。
10. **webServer 时序**：apply 时 `ctx.get('webServer')` 可能 undefined（web-app bundle 后注册）——用
    `ctx.inject(['webServer'], cb)` 等就绪再挂路由；headless 下该 fiber 不激活，不影响工具面。
11. **client bundle 格式**：必须是 `window.__ModuleLoader__.load({ id, factory: (require) => module.exports })`，
    factory 里 `require("react")`（种子模块）；`exports.apply` + `exports.inject=['slots']`；视图注册
    `ctx.slots.inject("conversation.view", () => ctx.slots.register({name, id, order, label}, View))`。
12. **--patch 重复 insert 会炸**：overlay 再 insert 相同 row id 报 `duplicate loader entry id`（不是合并）。
13. **HTTP 客户端编码**：PowerShell `Invoke-WebRequest -Body <string>` 发非 UTF-8，中文变 `??`；
    用 `[System.Text.Encoding]::UTF8.GetBytes($json)` 或浏览器 fetch。
