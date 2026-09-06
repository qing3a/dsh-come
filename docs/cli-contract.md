# dsh-come ↔ dsh CLI 契约（v2）

启动器只依赖以下稳定表面。任何一项被 upstream 破坏 → 显式升级本文件并 bump 启动器版本；
启动器**不**解析 CLI 输出、**不**读 dsh 内部文件、**不**碰插件 API（鸭子类型原则）。

| # | 契约 | 依赖方式 | 依据 |
|---|---|---|---|
| C1 | `dsh web --host <host> --port <port>` | spawn 子进程参数（web app flag 透传）；**PATH 直启系统 dsh**（2026-08-19 起**无 npx 回退**——dsh 缺失走安装流程：wizard 自动 `npm install -g @deepseek-ai/dsh` 或管理页手动，见 `src/installer.rs`） | `apps/cli/tests/args.spec.ts`（官方测试锚定 `--host/--port`） |
| C2 | `GET http://127.0.0.1:<port>/` → HTTP 存活应答：2xx，或 **401/403**（dsh 0.1.2-rc.1 起 web 默认本地鉴权，裸请求返回 401 鉴权挑战——挑战本身就是服务存活的证明） | 健康/就绪探测（`supervisor::http_ok_path`） | README「the command prints its URL」+ web profile 首屏 |
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

端口已被**健康** dsh 占用（HTTP 存活应答，见 C2）时，壳**接管**而非重复启动（`owned=false`）：

- **探活与接管**：monitor 对 adopted 每 5s 探活（HTTP + 端口 PID），连续 3 次失败判定外部 dsh 已死 →
  **自动接管**：清残留进程 + spawn owned 实例（保证 dsh 一直运行）；端口换主人自动更新认领目标
- **关闭不再区分内外（2026-08-19 用户拍板）**：`kill_child` 已移除 `owned` 判断——**stop / 重启 /
  退出统一真正关闭 dsh（含认领的外部实例）**，不再"只解除认领"。托盘「关闭引擎」运行中即可点
- **doctor 协调**：健康占用者 → 接管提示（🟢，不杀）；不健康僵尸占用（无存活应答）→ 按分级处置杀
  掉腾出端口
- **重启后接管**：spawn 分支始终 `owned=true`——手动重启后新实例由壳完整管理

## 升级契约的显式步骤

1. 契约变化（flag 改名 / 端口默认值变 / --patch 语义变 / headless 退出码变）→ 先改本文件
2. bump 启动器版本（启动器与 DSH 版本解耦，见 DESIGN §6）

## 已知稳定面（勿依赖）

- dsh 的 Web UI 内部路由 / API 结构（可能变，不解析）
- `dist/` 前端构建产物路径（SW 出现后可能有 `service-worker.js`，勿假设）
- CLI 的 stdout 文案（调试用，不解析）

## dsh 行为敏感面清单（每次跟进 dsh 新版逐条对照）

壳「跟随系统 dsh」意味着 dsh 侧任何行为变化都会在**更新动作之后**考验壳的每条链路。
下表是壳隐式依赖的 dsh 行为、断裂症状与已落地的防护；跟进新版时逐条核验：

| # | 敏感面 | 壳在哪里依赖 | 断裂症状 | 防护 |
|---|---|---|---|---|
| S1 | web 探测应答语义（2xx/401/403=存活） | `supervisor::http_ok_path`（就绪探测、认领探活、页面探活）、`doctor::probe_port` | 就绪永不就绪、启动超时；doctor 把健康引擎当僵尸**误杀** | 401/403 视为存活（2026-09-06 实测 rc.1 默认鉴权后误判） |
| S2 | web 鉴权入口 URL（启动打印 `?token=…`） | `supervisor::ui_url()` 从 engine.log 提取末次 token URL；托盘/向导打开浏览器 | 用户打开裸 URL 看到 401 文本 | ui_url 提取 + 裸 URL 回退；打印格式变则更新 `ui_url_from_log` |
| S3 | 更新期间的进程/磁盘一致性 | `installer::install_dsh`（停引擎 → npm → 拉回） | 内存旧代码 + 磁盘新文件混装 → web 启动报错（如 client-modules boot manifest batches） | 更新前停引擎；doctor `engine-version-stale` 探测兜底 |
| S4 | `dsh web` 子命令**拒绝父级参数** | `supervisor::build_command`：`--patch` 必须放在 `web` **之后** | `dsh --patch x web …` 报 "web takes none of parent …" 退出(1)，引擎起不来 | 参数序已在 build_command 固化（2026-08-20 实测） |
| S5 | `dsh --version` 输出一行版本文本 | `runtime::dsh_version`（state.json 快照、doctor 版本比对、托盘状态行） | 版本展示异常、doctor 版本比对失效 | capture_trimmed + 3s 超时；格式变则更新解析 |
| S6 | dsh stdout/stderr 重定向进 engine.log | S2 的 token URL、npm 安装期日志心跳都靠它 | token URL 丢失 → 回退裸 URL；安装期无心跳 | 壳侧重定向（`supervisor::start`），dsh 改日志通道时需重接 |
| S7 | `dsh plugin --profile web add/remove`（C5，转发 pnpm） | 管理页插件装/卸（`status::run_dsh_capture`） | 插件装/卸失败 | 退出码 + tail 文本透出给管理页 |
| S8 | npm 全局布局（`npm install -g` 落点 = dsh 命令同目录） | `installer::npm_for_dsh`、`which()` | 装完版本没变（PATH 里另一套 node 生态在前） | npm 取「与 dsh 同目录」的那一个（2026-08-20 实测） |

## 变更记录

- 2026-09-06：C2 由「HTTP 200」放宽为「存活应答（2xx 或 401/403）」——dsh 0.1.2-rc.1 起 web
  默认本地鉴权，裸探测 401 被误判死亡（就绪永不就绪 + doctor 误杀健康引擎，实测）。新增
  S1-S8 敏感面清单；`installer::install_dsh` 更新改为停引擎→npm→拉回（防新旧代码混装，
  同日实测 boot manifest batches 报错）
- 2026-08-19：C1 **移除 npx 回退**——用户拍板「不走临时拉取，缺失就正常安装」：node 用 winget 装 LTS、
  dsh 用 `npm install -g @deepseek-ai/dsh`（wizard 自动触发 + 管理页手动）；`config.pin_dsh_version`
  字段随 npx 通道一并移除；PATH 探测合并注册表与 `npm prefix -g`（装完不重启进程即可用）
- 2026-08-17（PTY 回归）：C1 的 npx 通道按 `config.pin_dsh_version` 锁版本（默认 `0.1.0-rc.6`，官方 rc.7 Windows PTY 回归；配置项 `#[serde(default)]` 缺省即钉 rc.6，置 null 手动关闭）。系统 dsh 直启路径不锁版本。设计原则「不管理版本」的例外：npx 通道跟随 latest 会吃到回归，pin 是配置开关（可置 null 关闭）。（**2026-08-19 随 npx 回退移除**）
- 2026-08-17：v2 — 移除隔离（C3 由 DSH_HOME 隔离改为 `--patch` overlay），移除版本管理，C5 由捆绑 pnpm 改为系统 dsh 直启
- 2026-08-14：v1 定稿（C1-C3 实现，C4 预留）。
