# mcp-apps-host 设计（MCP-Apps 宿主插件）

> 2026-08-27 定稿（用户拍板：现在就做 Phase 0 spike；server 连接层**自管**；代码放本仓库
> `plugins/mcp-apps-host/`）。实现 SEP-1865「MCP Apps: Interactive User Interfaces for MCP」
> 的 **Host 侧**，让 dsh 能渲染 MCP server 分发的交互式应用界面，并最大化 MCP 协议兼容。
>
> 参考：SEP-1865 [draft spec](https://raw.githubusercontent.com/modelcontextprotocol/ext-apps/main/specification/draft/apps.mdx)、
> [SEP 页](https://modelcontextprotocol.io/seps/1865-mcp-apps-interactive-user-interfaces-for-mcp)、
> 官方 ext-apps SDK（app 侧参考实现）。
>
> 背景判断（2026-08-27 分析）：协议为 draft、未定稿；但 dsh 生态的宿主基建
> （会话/授权/沙箱/工具注册/UI 注入/LLM）已齐，缺的只是协议层——所以自研成本可控，
> 风险集中在协议变动。对策：协议层单模块隔离 + 实现时锁定 spec 快照。

## 1. 核心架构：宿主 = 双向 MCP 端点，全走官方 SDK

```
┌─────────────── dsh 进程 ───────────────┐
│  mcp-apps-host 插件                       │
│  ┌──────────────────────────────────┐   │
│  │ 对 app：MCP Server（postMessage 传输）│   │   @modelcontextprotocol/sdk 的 Server 语义
│  │ 对 server：MCP Client（stdio/HTTP）  │   │   自管连接（决策：不用 dsh-mcp-client 的内部）
│  └──────────────────────────────────┘   │
│        ↕ 标准 JSON-RPC（协议层唯一接触 spec）│
└──────────────────────────────────────────┘
```

**兼容性定义**：任何用官方 `@modelcontextprotocol/sdk` / `@modelcontextprotocol/ext-apps`
写的 app，零改动即可连上本宿主。**验收标准 = 官方 SDK 写的 app 能跑通**（CI 用官方示例
TicTacToe 类 app 做回归件）。

## 2. 模块结构

```
plugins/mcp-apps-host/
├── src/
│   ├── index.ts              # Cordis 入口：装配 + 路由/工具注册
│   ├── protocol/             # ★ 唯一接触 spec 的层（升级=换这层，锁 spec 快照）
│   │   ├── transport.ts      # postMessage Transport（实现 SDK Transport 接口）
│   │   ├── handshake.ts      # ui/initialize 能力协商（appCapabilities ↔ hostCapabilities）
│   │   ├── messages.ts       # ui/* 消息类型 + 入站校验
│   │   └── lifecycle.ts      # open/close/dimensions/theme/streaming 通知
│   ├── host/
│   │   ├── server-bridge.ts  # app→server 代理（tools/call、resources/read 双向）
│   │   ├── discovery.ts      # 从自管连接的 server 发现 ui:// 资源 + _meta.ui
│   │   ├── sandbox.ts        # 沙箱 proxy 页（独立 origin：插件自起本地 HTTP 随机端口）
│   │   ├── csp.ts            # 从 ui.csp 元数据构造 CSP（严格默认 + 宿主全局白名单）
│   │   └── approval.ts       # 审批（挂 dsh-authorization/user-approval）
│   ├── dsh/
│   │   ├── tools.ts          # app 提供工具 → ctx.tools（app__<appId>__<tool> 命名空间）
│   │   ├── context.ts        # ui/update-model-context → dsh-session reference
│   │   ├── sampling.ts       # sampling/createMessage → ctx.llm（人工确认）
│   │   └── client-ui.ts      # 应用列表面板（client plugin，可选）
│   └── registry.ts           # server → app 清单（来源/哈希/CSP/生命周期状态）
└── test/                     # spike 与兼容性测试件
```

## 3. 与 dsh 生态的接线

| 能力 | 复用 | 说明 |
|---|---|---|
| server 连接 | **自管**（SDK Client，stdio/streamable-http） | 决策 2026-08-27：dsh-mcp-client 不暴露 client 服务，apps-host 自己管连接（配置独立，与 dsh-mcp-client 平行） |
| 工具注册 | `ctx.tools` | app 工具命名空间化 + 确定性 hash（沿用 dsh-mcp-client 的命名方案） |
| 审批 | `dsh-authorization` / `dsh-user-approval` | spec 的 per-instance/per-server/per-tool 粒度 |
| 会话/状态 | `dsh-session` + `dsh-session-persistence-jsonl` | app 状态、已注册工具的重连恢复 |
| LLM 采样 | `ctx.llm` | sampling/createMessage 的宿主实现 |
| 渲染 | `dsh-host-webserver` 路由（宿主页 3080）+ 插件自起随机端口（沙箱 origin） | 满足 spec「不同 origin」 |
| 主题 | `dsh-client-ui-theme` CSS 变量 | spec theming 节 |
| 审计 | `dsh-session-telemetry` / 结构化日志 | 「可审计通信」条款 |

## 4. 最大化 MCP 兼容的 6 条

1. postMessage transport 实现成 SDK Transport 接口（app 用标准 `Client` + 我们给的 transport 直连）；
2. 宿主两侧都用官方 SDK（对 app 是 Server 语义、对 server 是 Client）；
3. 严格能力协商：如实声明 `hostCapabilities`，app 请求前先查（spec 强制）；
4. 只发标准 MCP 消息 + spec 规定的 `ui/*`，不发明自定义协议；
5. 双向工具流走标准 `tools/call`/`tools/list`（app→server 经宿主代理；agent→app 走 app 注册工具）；
6. 兼容性测试矩阵：官方 ext-apps SDK 写的 app 作为 CI 验收件。

## 5. 分阶段路线

- **Phase 0 spike（2026-08-27 完成，PASS）**：连一个声明 `ui://` 的 server → iframe 渲染 →
  握手 → 一条 `tools/call` 打通。9 步全过：自管连接 / ui:// 发现 / HTML 读取 / 宿主+沙箱页
  （不同 origin）/ SDK Client initialize 握手 / ui/initialize 能力协商 / tools/list / tools/call
  代理（app→host→server→host→app）/ sampling 拒绝。浏览器部分留手动验证（宿主页 URL 由
  spike 打印）。**发现**：① SDK 1.30 响应校验 × zod 4.4.3 兼容问题（zod-compat isZ4Schema +
  zod/v4-mini），Phase 1 需固定 zod 或绕过校验；② apps 层消息走裸 JSON-RPC 更贴近 spec。
- **Phase 1 核心宿主**：发现/注册表/沙箱 proxy/CSP/生命周期/双向工具桥/流式
  tool-input/tool-result 通知。
- **Phase 2 agent 集成**：app 工具进 `ctx.tools` + 审批；`ui/update-model-context` → 会话上下文；
  `sampling/createMessage` → `ctx.llm`。
- **Phase 3 打磨**：主题/尺寸/多 server/审计/断线恢复/客户端面板 + 上架 dsh-market。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 协议 draft 变动 | 协议层单模块隔离；实现锁 spec 快照（2026-01-26 或当前 draft），升级=换适配器 |
| dsh-mcp-client 不暴露 client | 已拍板自管连接（重复连接的代价可接受） |
| 沙箱逃逸/XSS | 严格 CSP（`default-src 'none'` + 元数据白名单 + 宿主全局白名单强制）、sandbox 属性、消息校验、全量审计 |
| 工具命名冲突 | app 工具命名空间化 + 确定性 hash |
| 被官方替代 | 插件保持薄、对齐官方 SDK、文档全；官方出宿主时可平滑停更 |

## 7. 决策记录

- 2026-08-27：现在就做 Phase 0 spike（协议 draft 期间验证假设，知识不浪费）；
- 2026-08-27：server 连接层**自管**（不依赖 dsh-mcp-client 内部）；
- 2026-08-27：代码放本仓库 `plugins/mcp-apps-host/`，发布 npm 包（md-studio 同模式）；
- 待定：插件 npm 包名（`mcp-apps-host` 或 `@dsh-come/mcp-apps-host`，发布时确认占用）。

## 8. 与 dsh 既有插件的冲突规避（2026-08-27 分析）

dsh 官方 bundle 160+ 插件共存靠的是 cordis 的注册机制 + 命名约定；apps-host 需遵守同一套，
并针对三个真实重叠点立规则：

| 重叠点 | 冲突形态 | 规避规则 |
|---|---|---|
| **dsh-mcp-client（最大）** | 同一 MCP server 若同时配进两个插件 → **双连接/双会话**（两个 stdio 子进程、状态各一份） | ① apps-host **只代理不注册** server 工具进 ctx.tools（agent 工具通道仍归 dsh-mcp-client，避免 `mcp__` 命名空间抢占——dsh-mcp-client 对抢占会回滚整个世代，是硬错误）；② 带 apps 的 server 建议**只配 apps-host**，或接受双连接（无状态 server 可接受）；③ 远期：给 dsh-mcp-client 提 PR 暴露 client 服务，消除双连接 |
| **ctx.tools 命名空间** | app 提供的工具与现有工具重名 → 注册冲突/覆盖 | 独立前缀 `app__<appId>__<tool>` + 规范化（64 字符、确定性 hash，沿用 dsh-mcp-client 方案）；**绝不使用 `mcp__` 前缀** |
| **webServer 路由 / 存储域 / 端口** | 路径撞车（md-studio 已占 /workbench、/api/workbench）；状态文件撞车；固定端口被占 | 路由统一 `/mcp-apps/*`、`/api/mcp-apps/*`；沙箱/宿主辅助 HTTP 一律**随机端口**（bind 0）；app 状态存独立数据域（md-studio 的 store.json 模式）；采样/审批走共享服务（ctx.llm / dsh-authorization），只消费不抢占 |

其余无冲突面：client UI 注入走标准注入点（dsh-client-ui-* 几十个插件同机制共存）；
与 dsh-come 壳零交集（进程外）；与 md-studio 无交集（不同路径/前缀）；dsh-market 上架为纯增量。
cordis 插件的注册冲突默认报错/回滚——冲突会显式暴露而非静默破坏，这是生态的安全网。
