# 三层集成落地方案

> 2026-08-17 定稿。基于源码审计结果，规划 dsh-come（壳）+ dsh（平台）+ md-agent（领域 MCP）三层协作。
> 核心原则：**dsh-come 管壳、dsh 管平台、md-agent 管数据**，各层独立进程、独立部署、MCP 连接。

> ⚠️ **2026-08-18 状态标注**：本方案与同日 `docs/slimming-plan.md`（已执行）存在冲突，部分条目
> **已不可落地**，按现状修订如下：
> - ❌ **status_page 增强（§4.4 / Phase 2）**：status_page.rs 已被瘦身删除（dsh web UI 已展示状态），本方案不再适用
> - ❌ **托盘 md-agent 子菜单（§4.3）**：托盘已精简为 5 项菜单（状态行/打开/重启/日志/退出），不再新增子菜单
> - ⚠️ **come.patch.yml 三条目（§2）**：当前实现只写 dsh-market 一条（`src/runtime.rs::ensure_come_patch`）；
>   recruit-workbench / kb-mcp 条目未写入，且 kb-mcp 依赖的 md-agent release 构建是否就绪需先确认
> - ✅ **仍成立**：dsh 侧插件（recruit-workbench、kb-mcp 桥接）、「壳不代启动外部服务」原则、数据流设计

---

## 1. 三层职责边界

### Layer 1 — dsh-come（壳 · Rust）

**只管三件事：守护进程、托盘交互、插件市场引导。**

| 职责 | 现状 | 目标 |
|------|------|------|
| dsh 引擎守护 | ✅ supervisor 崩溃自愈 + 退避 | 保持 |
| 托盘菜单 | ✅ 打开/重启/日志/市场/更新 | 增加 md-agent 开关 |
| come.patch.yml | ✅ 禁用 dsh-market 重启 | 增加 recruit-workbench + kb-mcp 条目 |
| 工作台分组 | ✅ builtin_marketplace | md-agent 工作台卡片入口 |
| md-agent 守护 | ❌ 无 | **新增**：可选守护 md-agent 进程 |
| 状态页 | ✅ dsh 状态 | 显示双服务状态（dsh + md-agent） |

**不做的事**：不内置业务逻辑、不写 UI、不管知识库。

### Layer 2 — dsh 平台层（Cordis + TS）

**管所有用户可见的东西：Web UI、会话、插件、工具。**

已有插件：
- `recruit-workbench`（18 tools）：招聘业务 CRUD + 状态机 + 审计
- `dsh-market`：800+ 社区插件市场

需要新增的插件：
- `kb-mcp`（桥接插件）：把 md-agent 的 MCP 工具注册为 dsh 工具
- `recruit-ui`（工作台 UI）：md-hr 前端资产以 client plugin 形态注入 dsh Web UI

**不做的事**：不复制 md-agent 的知识库引擎、不自己跑 ripgrep/SQLite。

### Layer 3 — md-agent（领域 MCP server · Rust）

**只管数据层：检索、记忆、图谱、风控、任务。**

保持独立进程，以 `--mcp` 模式运行，通过 stdio JSON-RPC 暴露 12 个工具：

| MCP 工具 | 作用 | dsh 侧消费者 |
|----------|------|-------------|
| `search` | ripgrep 全文检索 + 激活扩散联想 | kb-mcp 桥接 |
| `read_l1` | 读 KB 规范层（FRAMEWORK/RULES/MEMORY） | kb-mcp 桥接 |
| `memory_search` | 跨层记忆检索 | kb-mcp 桥接 |
| `memory.recall` | 跨会话记忆召回（grep + 语义） | kb-mcp 桥接 |
| `graph.linked` | 文档出链 | kb-mcp 桥接 |
| `graph.backlinks` | 文档入链 | kb-mcp 桥接 |
| `graph.paths` | 文档间最短路径 | kb-mcp 桥接 |
| `risk.check` | 风控预警（规则零 LLM） | kb-mcp 桥接 |
| `tasks` | 任务引擎状态 | kb-mcp 桥接 |
| `file_read` | 读 KB 文件全文 | kb-mcp 桥接 |
| `pending.list` | 待审提案列表 | kb-mcp 桥接 |
| `agent.spawn` | 派生受限子 agent | kb-mcp 桥接 |

**不做的事**：不提供 Web UI、不管理会话、不直接面对用户。

---

## 2. 集成接口：come.patch.yml

dsh-come 的 `come.patch.yml` 是三层串联的枢纽。当前内容只有 dsh-market 禁重启一条，目标形态：

```yaml
# dsh-come 壳维护的 patch overlay
# 1. dsh-market 禁用 detached 重启（已有）
- id: dsh-market
  config:
    allowRestart: false

# 2. recruit-workbench 插件（已有，从 ~/.dsh/profiles/web/cordis.patch.yml 迁移过来）
- insert:
    - id: recruit-workbench
      name: 'file:///C:/Users/Administrator/Desktop/dsh-come/plugins/recruit-workbench/src/index.ts'

# 3. kb-mcp 桥接插件（新增）：连接 md-agent MCP server
- insert:
    - id: kb-mcp
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: mdagent
        transport: stdio
        command: 'C:/Users/Administrator/Desktop/md-agent/target/release/md-agent.exe'
        args: ['--mcp']
```

**关键点**：
- 条目 2 从 `~/.dsh/profiles/web/cordis.patch.yml` 迁移到 `come.patch.yml`，统一由壳管理
- 条目 3 是之前注释掉的 `mdagent-mcp`，但改用 release 构建 + 由壳统一注入（不再散落在 profile 里）
- dsh-come spawn dsh 时 `--patch come.patch.yml` 自动挂载全部三条
- 用户不再需要手动改 `~/.dsh/profiles/web/cordis.patch.yml`

---

## 3. 新增插件：kb-mcp 桥接

**目标**：把 md-agent 的 12 个 MCP 工具以 `mcp__mdagent.*` 命名空间注册进 dsh 工具表。

**实现方式**：直接用 dsh 官方的 `@deepseek-ai/dsh-mcp-client` 插件（就是之前 cordis.patch.yml 里注释掉的那个），不需要自己写代码。

**与 recruit-workbench 的职责划分**：

| 维度 | recruit-workbench | kb-mcp (md-agent) |
|------|-------------------|-------------------|
| 数据 | 招聘业务数据（候选人/岗位/推荐/面试/Offer） | 知识库内容（md 文档/记忆/图谱） |
| 存储 | `$DSH_HOME/recruit-workbench/store.json` | md-agent 自己的 SQLite + 文件系统 |
| 工具数 | 18 个（recruitwb_*） | 12 个（mcp__mdagent.*） |
| 写操作 | 直接写 JSON store | 只读为主，写走 pending 人审 |
| UI | recruit-ui 工作台 | 无独立 UI，通过对话调用 |
| 依赖 | 无外部进程 | 依赖 md-agent 进程存活 |

**一个对话中的协作示例**：
```
用户：帮我看看张三的面试安排，再查一下知识库里有没有他的背景资料

dsh 调用链：
  1. recruitwb_list_interviews({candidate: "张三"})     → recruit-workbench 返回面试记录
  2. mcp__mdagent.search({q: "张三 背景 简历"})          → md-agent 检索 KB 返回命中片段
  3. dsh LLM 综合两者回答用户
```

---

## 4. dsh-come 壳层增强清单

### 4.1 come.patch.yml 升级（runtime.rs）

```rust
// 当前 ensure_come_patch() 只写 dsh-market 一条
// 改为写三条：dsh-market + recruit-workbench + kb-mcp
// 已存在则跳过（幂等）
```

### 4.2 md-agent 守护（supervisor.rs）

```rust
// 新增可选守护：配置项 manage_md_agent = true 时
// spawn md-agent.exe（非 --mcp 模式，独立 HTTP 服务）
// 崩溃自愈 + 退避（复用现有 supervisor 逻辑）
// 端口 8756 健康检查（GET /api/health）
```

### 4.3 托盘菜单增强（tray.rs）

```
托盘菜单目标结构：
├─ 打开 dsh 界面          (open → http://127.0.0.1:3080)
├─ 打开系统浏览器          (open_sys)
├─ ─────────
├─ md-agent
│  ├─ 启动                (spawn md-agent.exe)
│  ├─ 停止                (kill)
│  └─ 状态: 运行中/已停止   (GET /api/health)
├─ ─────────
├─ 插件市场               (market → dsh-market)
├─ 工作台
│  └─ 猎头协作            (md-hr → file:///.../index.html)
├─ ─────────
├─ 检查更新               (update)
├─ 开机自启               (autostart toggle)
├─ ─────────
├─ 日志目录               (logs)
├─ 数据目录               (data)
├─ 状态页                 (status_page)
├─ 重启引擎               (restart)
├─ ─────────
└─ 退出                   (quit)
```

### 4.4 状态页增强（status_page.rs）

```
当前：只显示 dsh 引擎状态
目标：
  dsh 引擎：运行中 (PID xxx, port 3080) ✅
  md-agent：运行中 (PID yyy, port 8756) ✅
  插件：recruit-workbench, kb-mcp, dsh-market
  KB 路径：C:\Users\Administrator\kb
```

---

## 5. 落地步骤（按优先级）

### Phase 1：打通基础链路（立即可做）

1. **修改 come.patch.yml**：在 `ensure_come_patch()` 里加入 recruit-workbench + kb-mcp 条目
2. **清理 ~/.dsh/profiles/web/cordis.patch.yml**：注释掉 recruit-workbench 条目（改由壳统一管理）
3. **编译 md-agent release**：`cargo build --release`（当前只有 debug 构建）
4. **测试**：启动 dsh-come → 验证 dsh 自动加载两个插件 + md-agent 被 MCP client 拉起

### Phase 2：壳层守护增强（1-2 天）

5. **supervisor.rs 增加 md-agent 守护**：可选开关，spawn + 健康检查 + 崩溃自愈
6. **tray.rs 增加 md-agent 子菜单**：启动/停止/状态
7. **status_page.rs 显示双服务状态**

### Phase 3：工作台 UI 整合（3-5 天）

8. **recruit-ui 插件**：把 md-hr 前端打包成 dsh client plugin（clientModules）
9. **工作台卡片**：dsh-come 托盘 → 工作台 → 猎头协作 → 打开 recruit-ui
10. **数据互通**：recruit-ui 调用 recruit-workbench 的 `/api/recruit-workbench/state` 读写数据

### Phase 4：高级能力（按需）

11. **scheduler 插件**：把 md-agent 的定时任务能力做成 dsh 插件
12. **ingest 插件**：文档摄入（PDF/DOCX → MD）做成 dsh 工具
13. **activity 审计面板**：md-agent 的活动日志在 dsh Web UI 展示

---

## 6. 数据流全景

```
用户在 dsh Web UI 对话
        │
        ▼
   dsh LLM 回路
    ├── recruitwb_*  → recruit-workbench → store.json (招聘业务数据)
    ├── mcp__mdagent.search → md-agent (stdio) → ripgrep + KB
    ├── mcp__mdagent.graph  → md-agent (stdio) → SQLite graph.db
    └── mcp__mdagent.memory → md-agent (stdio) → MEMORY.md + recall
        
用户点击工作台 UI
        │
        ▼
   recruit-ui (dsh client plugin)
    └── GET/POST /api/recruit-workbench/state → recruit-workbench → store.json

dsh-come 托盘
    ├── spawn dsh web --patch come.patch.yml → dsh 加载全部插件
    ├── (可选) spawn md-agent.exe → 8756 HTTP 独立服务
    └── 健康检查：GET 3080/api/health + GET 8756/api/health
```

---

## 7. 注意事项

1. **md-agent 构建模式**：kb-mcp 用 `--mcp` (stdio)，壳守护用普通模式 (HTTP 8756)。同一台机器上两种模式不要同时跑（端口冲突）。如果 kb-mcp 已经通过 stdio 拉起了 md-agent，壳守护就不需要再单独 spawn。
2. **KB 路径**：md-agent 的 KB 默认在 `~/.dsh/kb` 或 `~/kb`。确保 recruit-workbench 的 RULES 与 md-agent headhunter 模板的 RULES 保持一致（目前已经对齐）。
3. **版本兼容**：come.patch.yml 里 md-agent.exe 路径硬编码了 debug 构建。Phase 1 完成后改用 release 路径。
4. **回退方案**：如果 kb-mcp 导致 dsh 启动失败，删掉 come.patch.yml 里 kb-mcp 条目即可回退（recruit-workbench 不受影响）。
