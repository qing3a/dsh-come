# dsh-come（原 dsh-desktop）｜项目记忆（方向与决策记录）

> 本文档是本项目长期记忆：记录方向决策、参考项目、技术选型与踩坑。
> 改动方向前先读这里；新增决策按时间追加，保留历史不删改。

状态：✅ v1 定稿（启动器/伴侣）· 🚀 方向 v2 定案（2026-08-14）：**基座切官方仓库 + 猎头工作台（dsh 插件模式）+ 托盘参考 md-agent** · 🚀 方向 v3 定案（2026-08-16）：**只做 dsh-desktop，md-agent 功能插件化移植进 dsh 生态**（见第 0 节）· 🔄 **定名 dsh-come（2026-08-16）**：dsh-desktop → dsh-companion → dsh-come 两次更名，均在公开引用前完成；运行时数据路径不变（见 §0.6）。

## 0.7 工作区文件夹改名 dsh-come（2026-08-17，改名前操作）

**操作**：关闭本会话后，把项目文件夹 `C:\Users\Administrator\Desktop\dsh-desktop` 重命名为 `C:\Users\Administrator\Desktop\dsh-come`，然后重新用本软件打开新文件夹（新工作区）继续。

**改名前已完成（快照）**：
- 代码/清单/文档已全部定名 dsh-come（仓库 `qing3a/dsh-come`、二进制 `dsh-come.exe`、自启项 "DSH Come"、远程清单 `raw.githubusercontent.com/qing3a/dsh-come/master/verified-plugins.json`）；git 历史在（`.git` 随文件夹移动不丢）。
- 文档/插件里硬编码的旧文件夹路径已全部替换为 `Desktop/dsh-come`（`plugins/*/cordis.yml` 的 `file://` 引用、README/dsh-plugin-guide 命令示例等）——改后无需再改。
- npm 占位 `dsh-come@0.0.1` 已发布（2026-08-17，见下方发布踩坑）。

**改后接续要点（新会话先读）**：
- 本文件（`docs/memory.md`）是项目记忆中枢：方向 v3（§0）、定名链（§0.6/§0.5）、本改名前置（§0.7）都在此。
- 项目现在在 `C:\Users\Administrator\Desktop\dsh-come`；git remote 仍是 `qing3a/dsh-come`（已同步）。用旧路径打开会失败，用新路径即可。
- **npm 发布踩坑（2026-08-17 实测）**：npm CLI 不读 `NODE_AUTH_TOKEN` 环境变量（那是 CI 约定）——用环境变量发布实际走 `~/.npmrc` 旧 token，永远 403。必须**命令行内联**：`npm --//registry.npmjs.org/:_authToken=<granular token> publish`；token 需在 npmjs.com 生成 granular access token（Packages 读写，scope 选 All packages）。

## 0.6 定名 dsh-come（2026-08-16，当日第二次更名）

- **链路**：`dsh-desktop`（初始）→ 同日发现 GitHub 三方撞车（dataelement / SnowCrescenter-tech 同名，npm 名亦被占）→ 更名 `dsh-companion`（与中文「伴侣」对应）→ 用户拍板定名 `dsh-come`。
- **时点**：全部发生在 Show&Tell 帖发布前、npm 注册前、任何公开引用前——GitHub 改名后旧名 301 转发 + 保留 60 天，无迁移代价。
- **当前定名**：仓库 `qing3a/dsh-come` / 二进制 `dsh-come.exe` / 自启项 "DSH Come" / 远程清单 `raw.githubusercontent.com/qing3a/dsh-come/master/verified-plugins.json`。产品显示名 **DSH 伴侣** 不变。
- **数据路径不变**：`%LOCALAPPDATA%\dsh-desktop` 与 `DSH_DESKTOP_*` 环境变量保持（已有用户数据不迁移）。
- **自启迁移**：`set_autostart(true)` 删除 "DSH Desktop" 与 "DSH Companion" 两个旧注册表项。
- 本文档历史条目中的旧名保留（历史记录不删改），当前状态一律用 dsh-come。

## 0.5 更名 dsh-companion（2026-08-16，当日第一次更名，已被 0.6 取代）

- **触发**：调研 dsh-market 生态时发现 GitHub 上另有 dataelement/dsh-desktop（Electron 跨平台）与 SnowCrescenter-tech/dsh-desktop（Electron Windows，定位文案与我们几乎雷同，npm 名 dsh-desktop 0.2.0 亦为其发布）——「dsh-desktop」三方撞车。
- **决策**：仓库名/二进制名（dsh-companion.exe）/自启项（"DSH Companion"）/远程清单 URL 全部改为 dsh-companion（npm + GitHub 均无占用）；产品显示名 **DSH 伴侣** 不变；**运行时数据路径 `%LOCALAPPDATA%\dsh-desktop` 与环境变量 DSH_DESKTOP_* 保持不变**（已有用户数据不迁移）。
- **迁移兼容**：`set_autostart(true)` 时删除旧 "DSH Desktop" 注册表自启项；GitHub 仓库改名后旧链接 301 跳转；`verified-plugins.json` 远程 URL 指向新仓库（改名前旧 URL 保留 raw 可达，改名后由 GitHub 自动转发）。
- **本文档历史条目中「dsh-desktop」保留原名（历史记录不删改），当前状态一律用 dsh-companion。

## 0. 方向 v3 定案（2026-08-16）

用户拍板：**未来只做 dsh-desktop 一个项目**，不再迭代 md-agent；md-agent 的猎头工作台/记忆/图谱/MCP 工具等能力，以 **dsh 官方插件形态**移植进 dsh-desktop 生态（md-agent 仅作移植参考）。

- 背景：md-agent 的 Rust 底座（agent 循环 / LLM 代理 / MCP server）与 dsh-desktop 的壳职责重叠，双项目维护成本高；dsh 官方插件模式已成熟，业务放插件层贴合基座策略。
- 落地规则：新业务一律做成 dsh 插件（参考 `plugins/recruit-workbench`、`plugins/recruit-tools`）；Rust 壳专注托盘/守护/更新/插件管理，不内置业务逻辑；md-agent 源码只读作参考，不回写。
- 可借鉴资产：md-agent `kb/apps/ow-recruit`（21 屏三端）、`kb/apps/headhunter`（四 Tab）、`src/templates/projects/headhunter`（业务规则）、L1/L2 明文双层记忆 + git 回滚 + 合规模式人审、`engine.rs` 的 MCP 家底对接模式（`@deepseek-ai/dsh-mcp-client` 暴露记忆/图谱/任务/风控工具）。

---

## 1. 方向 v2 定案（2026-08-14）

用户拍板的三件事，作为后续开发的总纲：

### 决策 1｜基座：基于官方 deepseek-harness 仓库

- **上游**：[https://github.com/deepseek-ai/deepseek-harness/](https://github.com/deepseek-ai/deepseek-harness/)（官方仓库，持续跟随 upstream 迭代）
- **官方文档站**：[https://deepseek-harness.github.io/deepseek-harness/](https://deepseek-harness.github.io/deepseek-harness/)（插件入门在 `develop/basic/`）
- **含义**：引擎、Web UI、CLI 全部消费官方；本项目不再自研引擎层。所有业务能力以 **dsh 官方插件** 形态叠加（见决策 2）。
- **与 v1 的关系**：dsh-desktop 的「进程外 supervisor/启动器」定位保留（装好即用、托盘常驻、崩溃自愈、验证式更新），它服务的是官方 dsh；业务插件是叠加在官方 dsh 之上的新一层。壳与插件职责分离：
  - **壳（Rust，进程外）**：托盘/开机自启/守护/更新 —— 需要进程外常驻，放壳里（插件随 dsh 进程生灭，做不了这些）。
  - **业务（TS 插件，进程内）**：猎头工作台一切业务能力 —— 用官方插件模式开发。

### 决策 2｜产品方向：猎头工作台（参考 md-agent，用 dsh 插件模式实现）

- **参考项目**：`C:\Users\Administrator\Desktop\md-agent` —— 本地优先的猎头招聘私有化工作台（Rust + Web，MIT）。
- **参考什么**（不是照搬代码，是借鉴资产与模式）：
  - **领域资产**：`md-agent\kb\apps\ow-recruit`（完整版：21 屏三端 = PM 招聘需求建模 / 猎头招聘执行 / 候选人门户，含招聘漏斗、候选人评估、通知与审计日志）+ `md-agent\kb\apps\headhunter`（最小版：候选人 / 岗位 / 推荐 7 态 / 站内信四 Tab）。
  - **项目模板（业务规则沉淀）**：`md-agent\src\templates\projects\headhunter`（`FRAMEWORK.md`/`KB.md`/`MEMORY.md`/`RULES.md` + `notes` 候选人/客户公司/沟通记录/职位需求）。核心规则必须继承：
    1. 职位与候选人事实基于本项目 `notes`，不编造；
    2. 项目间严格隔离（客户/候选人不串用）；
    3. 候选人隐私：只记必要事实，内容只存本机；
    4. 薪资/Offer 等敏感信息标注「保密」；
    5. 重要信息先确认再落盘，删除需确认。
  - **数据与流程主线**：候选人 → 岗位 → 推荐 → 面试 → Offer → 入职；推荐多状态机（7 态）。
  - **本地优先 + 人审**：数据全部本机；AI 写入默认自动落地、可一键切「合规模式」人工审核（md-agent 的 L1/L2 明文双层记忆 + git 回滚通道，移植时保留「可审计」这条主线）。
- **落地方式**：全部用 **dsh 官方插件模式** 实现（见 `docs/dsh-plugin-guide.md`），不引入 md-agent 的 Rust 底座。插件按能力拆分（工具插件 / 配置插件 / 服务插件等），走 cordis.yml overlay 开发，成熟后发布安装。

### 决策 3｜技术形态：使用 dsh 支持的插件模式

- 插件 = TypeScript 模块，导出 `apply(ctx)`，由 Cordis 框架加载；通过 `cordis.yml` overlay（`- insert:`）插入本地插件，`pnpm dsh web --patch <yml>` 加载。
- 能力注册：`inject` 声明依赖 → `ctx.tools.register(defineTool({...}))` 注册工具；`Config` schema（Schemastery）接收用户配置；`ctx.effect()` 注册清理；配置热改触发 HMR 热替换。
- 详细速查 + 最小可跑示例：**`docs/dsh-plugin-guide.md`**（官方文档本地化浓缩，不用重新联网）。

### 决策 4｜驻留图标（托盘）：参考 md-agent

- **参考模式**：`md-agent\src\main.rs` —— tray-icon + winit 事件循环（主线程）+ Axum HTTP 服务（后台线程）；`--no-tray`/`MD_AGENT_NO_TRAY=1` 开发降级；`kb\notes\架构\托盘应用.md` 记录了架构与踩坑。
- **dsh-desktop 现状**：`src/tray.rs` 已复用该模式（tray-icon `0.24` + winit `0.30`，与 md-agent 相同版本组合，Windows 已验证可编译），菜单按 DSH 伴侣语义精简（状态行 / 打开界面 / 插件市场 / 检查更新 / 日志 / 退出），并做了两处 md-agent 之外的改进：
  - 托盘图标**颜色跟随系统主题**（浅色任务栏黑 logo / 深色白 logo，读 `HKCU\...\Themes\Personalize\AppsUseLightTheme`），避免浅色主题下白 logo 隐形；
  - 菜单重建从 2s 改为 **15s 兜底 + 事件驱动**（2s 定时重建导致鼠标悬停时菜单闪烁/消失，用户实测反馈）。
- **踩坑备忘（沿用）**：muda `CheckMenuItem::with_id` 参数顺序是 `(id, text, enabled, checked)`，勾选态误传 `enabled` 会导致整项置灰不可点（md-agent 2026-08-03 定位修复）。
- **方向 v2 的托盘**：入口/守护仍由壳负责；工作台入口（打开猎头工作台）与状态提示追加进托盘菜单。

---

## 2. 参考索引（路径速查）

| 参考 | 位置 | 用途 |
|---|---|---|
| 官方仓库 | `https://github.com/deepseek-ai/deepseek-harness/` | 基座上游 |
| 官方插件文档（入门） | `https://deepseek-harness.github.io/deepseek-harness/develop/basic/`（仓库内 `docs/user/develop/basic/`，含 `index.md`/`tool.md`/`config.md`，各有 `.zh.md` 中文版） | 插件模式标准 |
| md-agent（整体） | `C:\Users\Administrator\Desktop\md-agent` | 猎头工作台参考 + 托盘参考 |
| md-agent 托盘实现 | `md-agent\src\main.rs` | tray-icon + winit 模式 |
| md-agent 托盘笔记 | `md-agent\kb\notes\架构\托盘应用.md` | 架构与踩坑 |
| md-agent 猎头模板 | `md-agent\src\templates\projects\headhunter` | 业务规则/领域模型 |
| md-agent 工作台资产 | `md-agent\kb\apps\ow-recruit`（完整版 21 屏三端）、`md-agent\kb\apps\headhunter`（最小版四 Tab） | 界面与数据模型借鉴 |
| 猎头工作台插件骨架 | `plugins/recruit-tools/` | dsh 插件：8 个 recruit 工具 + 富卡片 + **Web 工作台界面**（`/recruit`，webServer 路由，与 AI 工具共用 store.json），真实对话 E2E 已验证 |
| 猎头工作台插件（完整版，client 插件） | `plugins/recruit-workbench/` | dsh 插件：19 个 recruitwb_* 工具 + **会话「工作台」视图标签**（React client 插件，conversation.view 槽）+ `/api/recruit-workbench/*` Web API + 审计；host 走 Node 24 原生 TS，client 为手写 ModuleLoader bundle，已装入 web profile（2026-08-14 晚，第四轮） |
| 本项目契约 | `docs/cli-contract.md` | 壳 ↔ dsh CLI 稳定面（C1–C5） |
| 本项目设计 | `DESIGN.md`（v1 启动器设计） | v1 架构 |

## 3. 关键文档地图

- `README.md` —— 对外介绍（v1 卖点 + v2 方向入口）
- `DESIGN.md` —— v1 启动器/伴侣设计（不随 v2 推翻，作为壳的架构底稿）
- `docs/memory.md` —— 本文档（方向与决策记忆，长期维护）
- `docs/dsh-plugin-guide.md` —— dsh 官方插件模式速查（开发业务插件的起点）
- `docs/cli-contract.md` —— 壳与 dsh CLI 的契约（C1–C5，upstream 变化时显式升级）

## 4. 变更记录

- 2026-08-16：**方向 v3 定案（只做 dsh-desktop）** + **插件市场远程清单落地** —— `plugins.rs` 的 verified.json 远程清单从「结构预留」变可用：启动后台拉取 `verified-plugins.json`（GitHub raw）→ 缓存 + 与内置清单按 id 合并（覆盖/追加，内置为基底兜底）→ 托盘菜单 `MarketDone` 事件驱动重建；离线/拉取失败静默回退内置清单（不阻塞、不打扰）。命名区分：`verified-plugins.json`（插件清单）与 DESIGN §5 的 `verified.json`（dsh 版本已验证清单，channel A）不同语义，避免共用一个 raw 文件。种子清单 `verified-plugins.json` 已建在仓库根（内容与内置一致，GitHub 尚未提交时拉取 404 走回退）。合并逻辑带 3 个单元测试（覆盖/追加/空远程）；**验证方式**：改动前后 `cargo check` 各通过一次（编译期验证）；合并算法另用独立 rustc 冒烟验证 4 条规则（含顺序）通过。⚠️ 本机环境坑：Git Bash coreutils `link` 遮蔽 MSVC link.exe + VS 18 目录已空 + Git 自带 mingw gcc 缺 cc1 —— 无法链接含 C 依赖（ring/zstd）的测试二进制；且一次错误 linker 尝试污染了 target 的 build script 缓存，后续 `cargo check` 会报 build script 链接失败，**`cargo clean` 后在完整 MSVC 环境重建即可**。

- 2026-08-14（晚，第四轮）：**完整版猎头工作台插件 `recruit-workbench`** —— 基于 GitHub 开源参考（md-agent headhunter 模板 + ow-recruit 21 屏）建成：19 个 `recruitwb_*` 工具（公司/候选人/职位/推荐 7 态/沟通/面试/Offer/删除确认/仪表盘/检索）+ 浏览器「工作台」会话视图标签（React client 插件注册进 `conversation.view`，手写 `window.__ModuleLoader__.load` bundle，无构建工具）+ `/api/recruit-workbench/{state,mutate,audit}` Web API（`ctx.webServer` 就绪后经 `ctx.inject` 注册，与工具共用业务逻辑与审计）。已装入 web profile（pnpm link + `cordis.patch.yml` 行），临时端口冒烟全通过：HTTP 200 / client bundle 200 / 状态机跳级拦截 / 删除确认 / 中文 UTF-8 落盘 / 审计。
  - **踩坑 1（Node 24 原生 TS）**：host 用 `src/index.ts` 直载（type stripping），**不支持参数属性** `constructor(public x)`（ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX），改用普通字段赋值；enums/namespaces/装饰器同样不可用。
  - **踩坑 2（webServer 时序）**：插件 apply 时 `ctx.get('webServer')` 可能为 undefined（webServer 由 web-app bundle 后注册），用 `ctx.inject(['webServer'], cb)` 等就绪再挂路由；headless 下不激活、不影响工具面。
  - **踩坑 3（patch 行不能重复 insert）**：--patch overlay 再 insert 相同 row id 报 `duplicate loader entry id`；冒烟想换数据目录时改为测后清理 store.json。
  - **踩坑 4（client bundle 手写）**：clientModules 只扫描 profile node_modules 里可解析包的 `dsh.client` 声明；bundle 必须是 `window.__ModuleLoader__.load({id, factory})` 格式，factory 里 `require("react")` 取种子模块。
  - **修复线上 profile**：`C:\Users\Administrator\.dsh\profiles\web\cordis.patch.yml` 曾被写成 `[]` 后直接追加 `- insert:`（YAML 非法，重启必挂）→ 已改为纯 insert 列表；`mdagent-mcp` 行引用但未安装 `@deepseek-ai/dsh-mcp-client` → 已 `dsh plugin --profile web add` 装上（md-agent.exe --mcp 正常拉起，8756 起服务）。
- 2026-08-14：方向 v2 定案 —— 基座切官方 deepseek-harness 仓库；新增猎头工作台方向（参考 md-agent，dsh 插件模式）；驻留图标参考 md-agent（托盘模式已复用，记录差异与踩坑）。
- 2026-08-14（晚，第三轮）：**Web 工作台界面** —— 插件经 `ctx.webServer.register` 挂 HTTP 路由：`/recruit` 服务自包含工作台页面（候选人/职位/推荐 7 态看板，可推进/拒绝/撤回），`/recruit/api/*` 提供 JSON 接口；领域操作抽成共用函数（`upsertCandidate`/`upsertPosition`/`createReferral`/`advanceReferral`），AI 工具与 HTTP API 共用同一套校验（含状态机迁移规则）。对话写入立即可见，双向同源。演示实例（`%LOCALAPPDATA%\dsh-desktop\demo-recruit`、端口 3198）已验证：页面 200 + API 正常 + 对话数据出现在工作台。
- 2026-08-14（晚，第二轮）：推荐状态机对齐 md-agent（已推荐→待客户反馈→面试中→已发Offer→已入职，终态 拒绝/撤回 可直达；迁移只允许推进链下一步或直达终态）。真实对话 E2E（headless + 真实 DeepSeek 模型）通过：模型全流程调用 recruit_* 工具、正确理解不可跳级、store.json 落盘核验。UI 层：8 个工具全部加 presentCall/presentResult 富卡片 + 新增 recruit_status 工作台状态工具。完整 client 插件（React + dsh.client.inject + exports["./client"]）需发布安装进 profile 才被发现，本地 overlay 不可行、且不能动 GUI —— 列为下一步。
- 2026-08-14（晚）：创建 `plugins/recruit-tools` 骨架 —— 7 个 recruit 工具（候选人/职位/推荐流水线），数据落 `$DSH_HOME/recruit/store.json`（本地优先、原子写、可审计）。验证：`dsh web --patch <cordis.yml> --dump-config` 组合正确；隔离 DSH_HOME + 临时端口冒烟加载通过（日志出现 `[recruit-tools] plugin loaded!` + HTTP 200）。
  - **踩坑 1（Windows 插件路径）**：cordis.yml 的 `name` 必须用 `file:///C:/...` URL；裸 `C:/...` 会被 ESM loader 当成协议 `c:`，报 `ERR_UNSUPPORTED_ESM_URL_SCHEME`。
  - **踩坑 2（ESM 重复导出）**：TS 源码里 `export const name` 后不要再写 `export { name, ... }`，报 `Duplicate export of 'name'`（官方 JS 产物那样写是为了 bundler 元数据，源码不需要）。
  - **踩坑 3（开发解析）**：插件在仓库内而 `@deepseek-ai/*` 依赖在 dsh 的 node_modules，需在 `plugins/recruit-tools` 建 junction 指向 npx 缓存 node_modules（已 gitignore），保证与 harness 共享同一份 Cordis 实例。