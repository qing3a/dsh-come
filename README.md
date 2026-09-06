# 道临天下（dsh-come）｜DSH 桌面壳与 LocalApp 生态

> 🌐 [English README](README.en.md)

**道临天下（dsh-come）—— 开源的 DSH 桌面应用：托盘常驻的进程守护 + 一键打开 DSH，并随附 LocalApp 生态（v0.3+）。**

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 变成**托盘常驻的桌面应用**：系统托盘图标 + 进程守护（崩溃自愈/退避重启）+ 一键打开/重启，不用每次手敲 `dsh web`。v0.3 起内置 **HLP 协议层与 LocalApp 生态**（[Harness-LocalApp](https://github.com/qing3a/harness-localapp)），安装即得 7 个开箱可用的业务应用。

## 内置能力

```
道临天下（dsh-come 桌面应用）
├── 桌面壳      托盘常驻 / 进程守护（崩溃自愈）/ 安装引导 / 自愈诊疗 / 管理页 / 自更新
├── DSH 引擎    系统安装的 dsh（缺失时自动安装；版本跟随系统 npm）
├── HLP 协议层   @hlp/dsh-light-cockpit（协议方法 + App 托管 + @协作好友 + 信任治理）
└── LocalApp 生态 7 个开箱可用的业务应用（见下表）
```

## 内置 LocalApp

| App | 图标 | 用途 |
|---|---|---|
| 数据驾驶舱 | 📊 | 销售 KPI / 月度趋势 / Top 客户 / CSV 导入（DuckDB 持久化） |
| 邮箱协作 | ✉️ | 真实邮箱承载的群组话题协作、@协作好友、好友活跃度、待办/询价提取 |
| 轻量 CRM | 👥 | 客户列表 / 订单明细 / 询价线索池 / 统一联系人视图 |
| 日程管理 | 📅 | 日程 / 待办 / 从邮件自动提取日程导入 |
| 项目看板 | 📋 | 项目 / 三列看板 / 任务拖拽流转 |
| 官网询价组件 | 💬 | 客户侧一键询价表单（可发布公网）+ 分享到 X + 反馈入口 |
| 产品目录 | 📦 | 老板侧产品库（增删改查 / 筛选 / 持久化） |

## 什么是 HLP

**HLP（Harness LocalApp Protocol）** 是本项目的协议层（`io.deepseek.harness.localapps`）：定义业务应用如何被 Agent 宿主发现、渲染、编排与「深入对话」（用户在对话里 `@协作好友` 即联动业务工具）。应用 = 声明式元数据 + 标准 MCP 业务 Server + iframe 纯表现层（iframe 不持 MCP 客户端，即使被 XSS 攻破也无法直接发起工具调用）。完整规范见 [HLP 协议 v1.0 正式规范](https://github.com/qing3a/harness-localapp/blob/master/docs/HLP-Protocol-v1.0_正式规范.md)。

> **面向谁**：已经装了 `dsh`（或 Node.js）的人，想要一个常驻托盘、双击即启动、挂了自动拉起的桌面入口。缺失时管理页/向导会自动安装（node 用 winget、dsh 用 `npm install -g`，不走 npx 临时拉取）。开发者直接用官方 `npx @deepseek-ai/dsh web` 亦可，本项目的价值是把引擎守护和桌面体验包起来。

> 🚀 **当前方向（2026-08-27 更新，v4）**：**越做越薄 + 壳零 UI**——壳只做三件事：守护（profile 组）/ 引导安装 / 环境清单（come.patch.yml），外加自更新；一切用户可见的东西都是 dsh 插件（详见 `docs/direction-v4.md`）。不做插件市场（归 [dsh-market](https://github.com/dsh-market/dsh-market) 插件）、不做版本管理（跟随系统 dsh）、不做状态页（dsh web UI 已有）。

## 架构（四层）

```
┌──────────────────────────────────────────────┐
│  dsh-come 桌面壳（Rust）                      │
│  托盘 / 进程守护（崩溃自愈）/ 安装引导 / 管理页 │
└──────────────────┬───────────────────────────┘
                   │ spawn + --patch come.patch.yml
┌──────────────────▼───────────────────────────┐
│  DSH 引擎（Agent 宿主，系统 dsh）              │
│  对话流（Agent + 工具面板） + Web GUI          │
└──────┬──────────────────────┬────────────────┘
       │ HLP 协议层            │ iframe 托管
┌──────▼──────────────────────▼────────────────┐
│  HLP 协议层（@hlp/dsh-light-cockpit）         │
│  协议方法 / App 注册表 / 代理桥 / 信任治理     │
└──────┬──────────────────────┬────────────────┘
       │ MCP stdio            │ Mail Bus（HLP over Email）
┌──────▼──────────┐   ┌──────▼────────────────┐
│ 业务 MCP Server │   │ 真实邮箱协作            │
│ （驾驶舱/邮箱…） │   │ 群组·待办·询价·自进化    │
└─────────────────┘   └───────────────────────┘
        LocalApp 应用层（7 个内置 + 动态/持久化 App）
```

## 与 WorkBuddy 的差异

WorkBuddy 是中心化的 AI 工作台产品。dsh-come 的差异化定位：**开源（MIT）**、**本地优先**（数据全部在本机，业务 Server 是本地 MCP 进程）、**协议驱动**（HLP 开放协议，第三方可按规范接入应用与传输绑定）、**去中心化协作**（应用数据经真实邮箱在对端之间直接流转，无中心服务器）。两者面向的场景不同，可并存使用。

## 它做什么

```
dsh-come.exe（Rust 单 exe，进程外 supervisor）
├── 进程守护   spawn `dsh web`（PATH 直启系统 dsh；崩溃自动重启（指数退避+健康期清零）；滚动日志
├── 自愈诊疗   doctor：扫描取证→分级→按模式处置→兜底升级（崩溃时逐级升级到急救）
├── 安装引导   缺失即正常安装（不走 npx 临时拉取）：node 缺失→winget 装 LTS；dsh 缺失→npm install -g
├── 托盘       打开界面（置顶）/ 状态行 / 重启引擎 / 关闭引擎 / 打开日志目录 / 退出
├── 管理页      http://127.0.0.1:3081：状态展示 + 安装 Node/dsh + 启动/关闭
└── patch      come.patch.yml 经 `dsh --patch` 挂载，禁用 dsh-market 的 detached 重启
```

系统托盘菜单（每 3 秒刷新状态）：**打开界面**（置顶）、状态行（`运行中 ✓ http://127.0.0.1:3080` 或
阶段提示）、**重启引擎**、**关闭引擎**（不区分是否本壳启动，真正关闭、省内存）、打开日志目录、
退出。引擎就绪后自动打开浏览器；未检测到 dsh/Node 时自动安装（或到管理页
`http://127.0.0.1:3081` 手动安装与启停）。

## 自愈诊疗（doctor）

证据驱动的自愈系统，**不写死检查**——所有「发现」来自对环境的实际扫描（孤儿 file:// 插件入口 /
损坏的 cordis.patch.yml / 残缺下载 / 端口被占 / 孤儿进程），将来是别的原因拖垮 dsh 也能识别。

- **模式阶梯**（失败逐级升级）：巡检 Inspect（只报不改）→ 处置 Treat（自动 🟢绿，🟡黄/🔴红只推荐）→
  主治 Attend（自动 🟢绿+🟡黄，🔴红只推荐）→ 急救 Emergency（全量，🔴红先备份再动）
- **接入**：首次启动跑「处置」；引擎反复崩溃时每次重启前逐级升级（处置→主治→急救），上限耗尽跑一次
  急救兜底再放弃
- **手动**：`dsh-come doctor`（默认巡检，只打印报告）/ `dsh-come doctor --mode attend`（执行修复）
- **安全边界**：所有修改先备份 `.bak`；进程处置排除当前运行的引擎；无端口证据的疑似 dsh 进程仅急救
  自动（防误杀你另开的实例）

## 快速开始

**Windows**：从 [GitHub Releases](https://github.com/qing3a/dsh-come/releases) 下载
`dsh-come.exe` 直接运行。源码构建亦可：

```bash
git clone https://github.com/qing3a/dsh-come
cd dsh-come
cargo run --release
```

**macOS / Linux**（一键安装，自动注册看门狗 launchd/systemd）：

```bash
curl -fsSL https://github.com/qing3a/dsh-come/releases/latest/download/install.sh | sh
# 或 wget -qO- … | sh；仓库内直接 sh scripts/install.sh
```

**前置**：Windows 10/11、macOS 或主流 Linux（Node.js 缺失时自动用 winget 安装 LTS；dsh 缺失时
自动 `npm install -g @deepseek-ai/dsh`——均可在管理页 `http://127.0.0.1:3081` 手动操作与查看进度）。
Linux 看门狗依赖 systemd 用户会话（无则自动降级为托盘/`--no-tray` 常驻，崩溃不自动复活）；
macOS 看门狗为 launchd LaunchAgent（登录自启 + KeepAlive）。

## 发布指南（含 HLP 插件打包）

发版前**必须**同步 HLP 插件源（随附的 LocalApp 生态来自这一步）：

```bash
# 从 HLP 开发仓库（默认 ../Harness-LocalApp，可用 --hlp-path 指定）同步最新插件
node scripts/sync-hlp-plugin.mjs
```

脚本会：校验源完整性 → 复制到 `target/release/hlp-plugin/`（排除 .git/data/测试文件——真实数据不进安装包）→ 验证 version 与 `business/*/node_modules` → 生成 `hlp-plugin-version.json`（版本标记，release notes 引用）。CI 的 `hlp-plugin-check` job 每次推送自动做同样校验（需仓库 Secret `DEPLOY_SSH_KEY`：可读私有仓 qing3a/harness-localapp 的 SSH 私钥）。

## 关键设计

| 决策 | 理由 |
|---|---|
| **跟随系统 dsh，不管理版本** | 安装/升级交给系统 npm（`npm install -g @deepseek-ai/dsh`）；壳不做版本锁定/回滚/冒烟验证 |
| **缺失即正常安装，不走 npx 临时拉取**（2026-08-19） | 临时拉取无法保证可用性与一致性；node 缺失→winget 装 LTS（弹一次 UAC），dsh 缺失→npm install -g（用户级）；wizard 自动触发 + 管理页手动兜底 |
| **不隔离数据** | 不设 `DSH_HOME`，dsh 用其系统默认目录（`%USERPROFILE%\.dsh`），与终端用法一致 |
| **进程外 supervisor** | 崩溃自愈 / 托盘 / 日志全在壳里，DSH 更新不影响壳 |
| **壳只碰「门把手」** | 只依赖启动命令/端口探测/进程管理（`docs/cli-contract.md`），不解析 CLI 输出、不读内部文件、不碰插件 API |

## 契约面（docs/cli-contract.md）

- C1 `dsh web --host <host> --port <port>`（PATH 直启系统 dsh；缺失走安装流程，无 npx 回退）
- C2 `GET http://127.0.0.1:<port>/` → HTTP 200（就绪探测）
- C3 `dsh --patch <path>`（come.patch.yml overlay）
- C4/C5 预留（v2 冒烟验证 / 插件管理）

## 与 dsh-tray 的关系

[`dsh-tray`](https://github.com/qing3a/dsh-tray) 是 DSH **进程内**插件（托盘/气泡通知，随 DSH 生灭）；本项目的 **进程外** 壳（守护 DSH 进程）。两者互补不冗余：同一用户装了两边时，dsh-tray 检测到 dsh-come 会自动降级。

## 贡献

欢迎社区贡献！见 [CONTRIBUTING.md](CONTRIBUTING.md)（Rust 开发 / 关键设计约束 / 发布流程）。

- 🐛 [报告 Bug](https://github.com/qing3a/dsh-come/issues/new?labels=bug&template=bug_report.md)
- 💡 [请求功能](https://github.com/qing3a/dsh-come/issues/new?labels=enhancement&template=feature_request.md)
- 🤝 [提交 PR](https://github.com/qing3a/dsh-come/pulls)

## 许可

MIT。托盘图标为代码生成的 32x32 圆角图标（`src/tray.rs`），与 DeepSeek AI 商标无关联。

## Roadmap

- ✅ v1（当前）：进程守护 / 托盘 / 自动开浏览器 / come.patch.yml / 崩溃退避重启 / 自愈诊疗（doctor，证据驱动分级处置）
- ✅ P0（2026-08-27）：自动更新（GitHub Releases + GitHub Actions 自动发布 + SHA256 校验 + 询问制）/ i18n（zh/en：托盘/通知/CLI/管理页，README 双语）
- ✅ 跨平台（2026-08-29）：三平台矩阵发布（win / macOS universal / linux）+ `install.sh` 一键安装 + launchd/systemd 看门狗 + 自更新按平台取清单（`update-{win,macos,linux}.json`）
- 🔜 P1：多实例（单机多 profile 组）；P2：统一为「写插件」（come-manager / md-studio 模板 / 上架 dsh-market）——详见 `docs/direction-v4.md`
