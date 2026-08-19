# dsh-come｜DSH 伴侣

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 变成**托盘常驻的 Windows 桌面壳**：系统托盘图标 + 进程守护（崩溃自愈/退避重启）+ 一键打开/重启，不用每次手敲 `dsh web`。

> **面向谁**：已经装了 `dsh`（或 Node.js）的人，想要一个常驻托盘、双击即启动、挂了自动拉起的桌面入口。缺失时管理页/向导会自动安装（node 用 winget、dsh 用 `npm install -g`，不走 npx 临时拉取）。开发者直接用官方 `npx @deepseek-ai/dsh web` 亦可，本项目的价值是把引擎守护和桌面体验包起来。

> 🚀 **当前方向（2026-08-17 定案）**：**越做越薄**——壳只做托盘 + 进程守护 + 极简启停（详见 `docs/slimming-plan.md`）。不做插件市场（归 [dsh-market](https://github.com/dsh-market/dsh-market) 插件）、不做版本管理（跟随系统 dsh）、不做状态页/向导页（dsh web UI 已有）。三层集成方案（md-agent 协作）见 `docs/integration-plan.md`。

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

```bash
git clone https://github.com/qing3a/dsh-come
cd dsh-come
cargo run --release
```

**前置**：Windows 10/11（Node.js 缺失时自动用 winget 安装 LTS；dsh 缺失时自动 `npm install -g
@deepseek-ai/dsh`——均可在管理页 `http://127.0.0.1:3081` 手动操作与查看进度）。

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

## 许可

MIT。托盘图标为代码生成的 32x32 圆角图标（`src/tray.rs`），与 DeepSeek AI 商标无关联。

## Roadmap

- ✅ v1（当前）：进程守护 / 托盘（5 项菜单）/ 自动开浏览器 / come.patch.yml / 崩溃退避重启 / 自愈诊疗（doctor，证据驱动分级处置）
- 🔜 可选：md-agent 守护（`docs/integration-plan.md` Phase 2）、三层集成（Phase 3）
