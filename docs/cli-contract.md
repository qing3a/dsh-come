# dsh-come ↔ dsh CLI 契约（v2）

启动器只依赖以下稳定表面。任何一项被 upstream 破坏 → 显式升级本文件并 bump 启动器版本；
启动器**不**解析 CLI 输出、**不**读 dsh 内部文件、**不**碰插件 API（鸭子类型原则）。

| # | 契约 | 依赖方式 | 依据 |
|---|---|---|---|
| C1 | `dsh web --host <host> --port <port>` | spawn 子进程参数（web app flag 透传）；**PATH 直启系统 dsh**（2026-08-19 起**无 npx 回退**——dsh 缺失走安装流程：wizard 自动 `npm install -g @deepseek-ai/dsh` 或管理页手动，见 `src/installer.rs`） | `apps/cli/tests/args.spec.ts`（官方测试锚定 `--host/--port`） |
| C2 | `GET http://127.0.0.1:<port>/` → HTTP 200 | 健康/就绪探测（v1 只看状态码；后续加版本指纹防 SW 陈旧 UI，见 DESIGN §7.4） | README「the command prints its URL」+ web profile 首屏 |
| C3 | `dsh --patch <path>` | 壳 patch overlay（come.patch.yml）经 CLI 顶层 `--patch` 传入，dsh-market 安装后禁止其 detached 一键重启 | `bin.js` 支持 `--patch <path>` repeatable |
| C4 | `dsh --profile headless "job"` | v2 冒烟验证扩展（mock-llm waterfall） | `apps/cli/README.md` Entry modes |
| C5 | `dsh plugin --profile web <pnpm args>`（add/remove） | 插件市场安装/卸载；直接走系统 `dsh`（PATH 直启），不设 DSH_HOME，pnpm 解析交给 dsh 自身 | `apps/cli/README.md`「Manage a profile's plugins by forwarding to pnpm」 |

## 设计原则（v2）

- **不隔离**：不设置 `DSH_HOME`，dsh 用其系统默认目录（`%USERPROFILE%\.dsh`），与终端里正常用法完全一致
- **不管理版本**：不锁定/回滚/冒烟验证 dsh 版本，安装升级全交给系统 npm（`npm install -g @deepseek-ai/dsh`）
- **壳只做守护**：进程 spawn → 健康探测 → 崩溃退避重启 → 滚动日志；不碰 dsh 数据、不代管 pnpm

## 自愈诊疗例外（doctor.rs）

「壳不碰 dsh 数据」的**唯一例外**：崩溃自愈诊疗（`dsh-come doctor`）会读并修复用户 profile 的
`cordis.patch.yml`（孤儿 file:// 入口 / 结构损坏）与清理残缺下载/孤儿进程。边界：

- 只做**文件系统级自愈**（patch 结构、下载缓存、端口/进程），不解析 dsh 业务数据、不依赖插件 API
- 所有修改先备份 `.bak`；影响半径分级：🟢绿（壳自有 come.patch.yml）自动 / 🟡黄（端口占用、孤儿
  配置条目、残缺下载）主治及以上自动 / 🔴红（重置损坏 profile patch）仅急救且先备份
- 进程处置**排除活引擎树**（supervisor 当前管理的进程）；无端口证据的疑似 dsh 进程仅急救自动（防误杀
  用户另开的实例）

## 认领语义（adopt，2026-08-18 定案，2026-08-19 修订「关闭引擎不区分内外」）

端口已被**健康** dsh 占用（HTTP 200）时，壳**接管**而非重复启动（`owned=false`）：

- **探活与接管**：monitor 对 adopted 每 5s 探活（HTTP + 端口 PID），连续 3 次失败判定外部 dsh 已死 →
  **自动接管**：清残留进程 + spawn owned 实例（保证 dsh 一直运行）；端口换主人自动更新认领目标
- **关闭不再区分内外（2026-08-19 用户拍板）**：`kill_child` 已移除 `owned` 判断——**stop / 重启 /
  退出统一真正关闭 dsh（含认领的外部实例）**，不再"只解除认领"。托盘「关闭引擎」运行中即可点
- **doctor 协调**：健康占用者 → 接管提示（🟢，不杀）；不健康僵尸占用（HTTP 不 200）→ 按分级处置杀
  掉腾出端口
- **重启后接管**：spawn 分支始终 `owned=true`——手动重启后新实例由壳完整管理

## 升级契约的显式步骤

1. 契约变化（flag 改名 / 端口默认值变 / --patch 语义变 / headless 退出码变）→ 先改本文件
2. bump 启动器版本（启动器与 DSH 版本解耦，见 DESIGN §6）

## 已知稳定面（勿依赖）

- dsh 的 Web UI 内部路由 / API 结构（可能变，不解析）
- `dist/` 前端构建产物路径（SW 出现后可能有 `service-worker.js`，勿假设）
- CLI 的 stdout 文案（调试用，不解析）

## 变更记录

- 2026-08-19：C1 **移除 npx 回退**——用户拍板「不走临时拉取，缺失就正常安装」：node 用 winget 装 LTS、
  dsh 用 `npm install -g @deepseek-ai/dsh`（wizard 自动触发 + 管理页手动）；`config.pin_dsh_version`
  字段随 npx 通道一并移除；PATH 探测合并注册表与 `npm prefix -g`（装完不重启进程即可用）
- 2026-08-17（PTY 回归）：C1 的 npx 通道按 `config.pin_dsh_version` 锁版本（默认 `0.1.0-rc.6`，官方 rc.7 Windows PTY 回归；配置项 `#[serde(default)]` 缺省即钉 rc.6，置 null 手动关闭）。系统 dsh 直启路径不锁版本。设计原则「不管理版本」的例外：npx 通道跟随 latest 会吃到回归，pin 是配置开关（可置 null 关闭）。（**2026-08-19 随 npx 回退移除**）
- 2026-08-17：v2 — 移除隔离（C3 由 DSH_HOME 隔离改为 `--patch` overlay），移除版本管理，C5 由捆绑 pnpm 改为系统 dsh 直启
- 2026-08-14：v1 定稿（C1-C3 实现，C4 预留）。
