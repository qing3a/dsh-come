# md-agent + dsh-come 瘦身方案

## 一、核心原则

**md-hr 是唯一变厚的地方。md-agent 和 dsh-come 都要越做越薄。**

瘦的标准不是"删几行代码"，而是：**每个模块的存在能否被"别人已经有了"替代**。
- dsh 已有 agent loop → md-agent 的 agent.rs 多余
- dsh 已有 LLM 集成 → md-agent 的 llm.rs 多余
- dsh 已有 MCP client → md-agent 的 mcp_client.rs 多余
- dsh-market 已有插件市场 → dsh-come 的 plugins.rs 多余
- dsh web UI 已有状态展示 → dsh-come 的 status_page.rs 多余

---

## 二、md-agent 瘦身方案

### 现状：25 文件 / 16,298 行 + 前端 16 文件 / 660K

### 目标：9 文件 / ~5,500 行 + 前端 1 文件 / ~10K（66% 减少）

### 2.1 保留（8 文件 / 5,374 行）— 数据层核心

| 文件 | 行数 | 职责 | 为什么留 |
|------|------|------|----------|
| main.rs | 431→250 | 入口 + CLI + 托盘 | 进程入口，不可省 |
| config.rs | 366→250 | 配置加载/保存 | 所有模块依赖 |
| mcp.rs | 393 | MCP stdio server（12 工具） | 对外暴露的唯一通道 |
| kb.rs | 1,567 | 知识库存储 + 布局 + L1/L2 分层 | 核心数据层 |
| memory.rs | 330→450 | 记忆提取/召回（吸收 consolidate） | 核心数据层 |
| graph.rs | 2,238 | 知识图谱 + 双向链接 + SQLite | 核心数据层 |
| search.rs | 618 | 全文检索（ripgrep 内核） | 核心数据层 |
| risk.rs | 240 | 风控检查 | 核心数据层 |

### 2.2 合并（2 文件 → 并入核心）

| 源文件 | 行数 | 目标 | 原因 |
|--------|------|------|------|
| ingest.rs | 133 | → kb.rs | 文件摄取本质是 KB 写入，无独立必要 |
| consolidate.rs | 213 | → memory.rs | 记忆整理本质是 memory 操作，无独立必要 |

### 2.3 精简（server.rs: 4,758 → ~800 行）

**当前 100+ 路由 → 保留 15 条核心路由：**

| 保留路由 | 对应模块 | 用途 |
|----------|----------|------|
| GET /api/health | - | 健康检查（dsh-come 探测） |
| GET /api/search | search.rs | 全文检索 |
| GET /api/tools | mcp.rs | 工具清单（MCP 复用） |
| POST /api/kb/sync | kb.rs | 索引重建 |
| GET /api/kb/pending* | kb.rs | 待审列表/预览/批准/拒绝（4 条） |
| POST /api/graph/sync | graph.rs | 图谱重建 |
| GET /api/graph/* | graph.rs | 图谱查询（6 条） |
| GET /api/risk | risk.rs | 风控检查 |
| POST /api/memory/* | memory.rs | 记忆操作（3 条） |
| GET/POST /api/config | config.rs | 配置读写 |
| GET/POST/DELETE /api/file | kb.rs | 文件读写 |

**砍掉的 85+ 路由及原因：**

| 路由组 | 行数(估) | 砍掉原因 |
|--------|----------|----------|
| /api/agent* | ~400 | dsh 已有 agent loop |
| /api/llm* | ~200 | dsh 直连 LLM |
| /api/mcp/servers* | ~250 | dsh 通过 dsh-mcp-client 管理 |
| /api/mcp/call, /api/mcp/usage | ~100 | 同上 |
| /api/mdapi/* | ~150 | 云同步独立服务或废弃 |
| /api/headhunter/* | ~200 | 业务逻辑归 md-hr |
| /api/hubs/* | ~300 | 插件市场归 dsh-market |
| /api/market/* | ~200 | 同上 |
| /api/apps/* | ~200 | 同上 |
| /api/tasks* | ~250 | 非核心 |
| /api/projects* | ~200 | 非核心 |
| /api/page, /api/fetch | ~150 | 网页抓取非核心 |
| /api/notifications* | ~150 | 非核心 |
| /api/scheduler* | ~150 | 非核心 |
| /api/activity* | ~100 | 非核心 |
| /api/dev/* | ~200 | 开发工具非核心 |
| /api/link*, /api/sessions | ~150 | 非核心 |
| /api/context/*, /api/experience/* | ~150 | 非核心 |
| /api/decide, /api/l1* | ~100 | 非核心 |
| /api/ingest | ~50 | 并入 kb |

### 2.4 移除（15 文件 / 5,820 行）

| 文件 | 行数 | 移除原因 |
|------|------|----------|
| agent.rs | 683 | dsh 已有 agent loop，md-agent 不需要自己的 |
| llm.rs | 295 | dsh 直连 LLM，md-agent 不做代理 |
| mcp_client.rs | 477 | dsh 通过 dsh-mcp-client 连接其他 MCP |
| hub.rs | 1,140 | 插件市场归 dsh-market / dsh-come |
| market.rs | 283 | 同上，重复 |
| mdapi.rs | 590 | 云同步是独立服务，不属于本地数据层 |
| heartbeat.rs | 138 | 心跳同步非核心 |
| page.rs | 231 | CDP 网页读取非核心（且依赖 chromiumoxide 重库） |
| fetch.rs | 95 | 静态网页抓取非核心 |
| task.rs | 324 | 任务管理非核心 |
| projects.rs | 323 | 项目管理非核心 |
| notifications.rs | 189 | 通知非核心 |
| scheduler.rs | 132 | 调度器非核心 |
| activity.rs | 111 | 活动流非核心 |
| consolidate.rs | 213 | 并入 memory.rs |

### 2.5 依赖瘦身

**移除：**
- `chromiumoxide` — CDP 浏览器自动化，重依赖（page.rs 专用）
- `hmac`, `sha2`, `hex` — md-api 云同步签名（mdapi.rs 专用）
- `anydoc` — 文档解析（ingest.rs 专用）
- `base64` — 编码工具（page.rs 专用）

**保留：**
- `axum` + `tower` + `tower-http` — HTTP 服务
- `tokio` — 异步运行时
- `grep` + `regex` + `ignore` — 检索引擎
- `rusqlite` — 图谱元数据
- `serde` + `serde_json` — 序列化
- `reqwest` — MCP stdio 不需要，但 KB 同步可能需要
- `chrono` — 时间戳
- `tray-icon` + `winit` — 系统托盘

### 2.6 前端瘦身

**当前 web/ 目录：16 文件 / 660K**
- index.html, app.js, core.js, stream.js, theme.css
- views/: automation, board, graph, home, market, mcp, notifications, sessions, skills
- config.html, onboarding.html

**目标：1 文件 / ~10K**
- 只留 `health.html` — 极简健康检查页（JSON 状态展示）
- 所有交互 UI 归 dsh web 或 md-hr

---

## 三、dsh-come 瘦身方案

### 现状：9 文件 / 2,837 行

### 目标：5 文件 / ~1,100 行（61% 减少）

### 3.1 保留（4 文件 / 939 行 → 850 行）

| 文件 | 行数 | 职责 | 精简 |
|------|------|------|------|
| main.rs | 105 | 入口 | 不变 |
| config.rs | 74 | 配置 | 不变 |
| runtime.rs | 221 | 路径/端口解析 | 不变 |
| supervisor.rs | 539→400 | 进程守护 | 移除 version pin 调用 |

### 3.2 精简（2 文件）

**tray.rs: 852 → ~300 行**

当前菜单项：
- 状态行 / 打开界面 / 系统浏览器打开 / 重启 / 自动启动 / 日志目录 / 退出
- 插件市场 / 清空数据 / 数据目录
- 工作台子菜单 / 面板导航子菜单 / 已装应用子菜单

精简后菜单项：
- 状态行（running / ready / error）
- 打开 dsh 界面
- 重启 dsh
- 退出

移除的菜单项及原因：
- 插件市场 → dsh-market 插件提供（Settings → Plugin Market）
- 清空数据 → 用户手动删目录即可（低频操作不值得维护）
- 数据目录 → 用户手动打开资源管理器
- 工作台/面板/已装应用子菜单 → dsh web UI 内已有导航

**wizard.rs: 317 → ~100 行**

当前：本地 HTTP 服务（3177 端口）+ 内嵌 HTML 页面 + 轮询状态 + 重试逻辑

精简后：
1. 检测 dsh 是否已安装（`dsh --version`）
2. 未安装 → 弹消息框"请先运行 npm install -g @deepseek-ai/dsh"
3. 已安装 → supervisor.start() → 轮询健康 → 打开浏览器

移除：
- tiny_http 本地服务（不需要独立向导页）
- 内嵌 wizard.html（不需要进度展示页）
- 端口探测逻辑（3177-3181 重试）

### 3.3 移除（3 文件 / 729 行）

| 文件 | 行数 | 移除原因 |
|------|------|----------|
| plugins.rs | 362 | dsh-market 插件提供完整市场功能（浏览/搜索/安装/卸载 800+ 插件） |
| version.rs | 243 | rc.7 PTY 回归是临时问题，官方修复后即无用 |
| status_page.rs | 124 | dsh web UI 已展示运行状态，壳不需要独立状态页 |

### 3.4 依赖瘦身

**移除：**
- `resvg` — SVG 光栅化（托盘图标改用静态 .ico 文件）
- `winreg` — 注册表读取（系统主题检测，改用固定图标）
- `tiny_http` — 向导 HTTP 服务（wizard 简化后不需要）

**保留：**
- `tray-icon` + `winit` — 托盘（核心）
- `serde` + `serde_json` — 配置
- `chrono` — 日志时间戳
- `reqwest` — HTTP 健康探测
- `windows-sys` — 窗口管理（open_browser 用）

---

## 四、执行步骤

### Phase 1：md-agent 瘦身（2 天）

1. **合并 ingest.rs → kb.rs**（0.5 天）
   - 把 ingest 的文件摄取函数移入 kb.rs
   - 删除 ingest.rs，更新 main.rs 的 mod 声明
   - 更新 server.rs 的 /api/ingest 路由指向 kb

2. **合并 consolidate.rs → memory.rs**（0.5 天）
   - 把记忆整理函数移入 memory.rs
   - 删除 consolidate.rs，更新 mod 声明

3. **删除 15 个模块**（0.5 天）
   - 逐个删除文件 + main.rs 的 mod 声明
   - 清理 server.rs 中对应的路由和 handler
   - 清理 config.rs 中对应的配置段

4. **精简 server.rs**（0.5 天）
   - 保留 15 条核心路由
   - 删除 85+ 非核心路由和对应 handler
   - 清理 AppState 中不需要的字段（mcp, mdapi_status 等）

5. **清理 Cargo.toml**（0.5 天）
   - 移除 6 个不再需要的依赖
   - 验证编译通过

### Phase 2：dsh-come 瘦身（1 天）

1. **删除 3 个模块**（0.5 天）
   - 删除 plugins.rs, version.rs, status_page.rs
   - 清理 main.rs, tray.rs 中的引用

2. **精简 tray.rs**（0.5 天）
   - 移除市场/插件/状态页相关菜单项和事件处理
   - 移除工作台/面板/已装应用子菜单构建逻辑
   - 只保留：状态行 + 打开 + 重启 + 退出

3. **精简 wizard.rs**（0.5 天）
   - 移除 tiny_http 服务和内嵌 HTML
   - 简化为：检测 dsh → 启动 → 打开浏览器

4. **清理 Cargo.toml**（0.5 天）
   - 移除 resvg, winreg, tiny_http
   - 准备静态 .ico 图标文件
   - 验证编译通过

### Phase 3：前端瘦身（0.5 天）

1. 删除 md-agent/web/ 下 15 个文件
2. 创建极简 health.html（~50 行）
3. 验证 dsh 能正常通过 MCP 调用 md-agent

---

## 五、"越做越薄"的长期策略

### 5.1 每次新增功能前的三问

1. **dsh 已经有了吗？** → 如果 dsh 或其插件生态已有，不自己实现
2. **md-hr 能做吗？** → 如果是业务逻辑，放 md-hr/core/
3. **真的属于这一层吗？** → 数据层只做存储/检索，壳层只做启停

### 5.2 md-agent 的"薄"定义

```
md-agent = KB存储 + 检索 + 图谱 + 记忆 + 风控 + MCP暴露
```

任何超出这个定义的功能都不应该进入 md-agent。具体来说：
- 不做 UI（前端归 dsh web 或 md-hr）
- 不做 Agent 回路（归 dsh）
- 不做 LLM 调用（归 dsh）
- 不做插件市场（归 dsh-market）
- 不做云同步（独立服务）
- 不做网页抓取（非核心）
- 不做任务/项目管理（非核心）

### 5.3 dsh-come 的"薄"定义

```
dsh-come = 托盘图标 + 进程守护 + 极简启停
```

任何超出这个定义的功能都不应该进入 dsh-come：
- 不做插件市场（归 dsh-market）
- 不做版本管理（跟随系统 dsh）
- 不做状态页（dsh web UI 已有）
- 不做向导页（简化到极致）
- 不做数据清理（用户手动）

### 5.4 定期审计

每季度做一次模块审计：
- 每个 .rs 文件是否仍然有存在的必要？
- 是否有功能被 dsh 或其他工具替代了？
- 依赖列表是否可以进一步精简？
- 行数是否在持续增长？（如果是，说明在做厚，需要反思）
