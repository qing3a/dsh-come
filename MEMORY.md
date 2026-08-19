# dsh-come — 项目记忆

> 系统托盘常驻壳：启动 dsh + 崩溃重启 + 极简向导。
> 越做越薄，只碰 dsh 的「门把手」——启动命令/端口探测/进程管理。

## 架构定位

```
dsh-come (Rust, 系统托盘 / 无头双模式)
  ├── tray.rs     → 托盘图标 + 菜单（打开界面置顶/状态行/重启/关闭/日志/退出；3s 刷新）
  ├── supervisor.rs → spawn dsh web + 指数退避重启 + 三层健康探测 + 崩溃逐级诊疗 + state/control 文件 IPC
  ├── runtime.rs  → 命令构建（系统 dsh 直启，无 npx 回退）+ 路径/运行器定位 + state/control 路径
  ├── installer.rs→ 环境探测（node/npm/dsh/winget，进程+注册表+npmpfx PATH）+ 异步安装 + 安装状态
  ├── doctor.rs   → 证据驱动自愈诊疗（扫描取证→推理分级→按模式授权处置→兜底升级）
  ├── job.rs      → Windows Job Object（dsh 整树受 KILL_ON_JOB_CLOSE 约束；terminate_job 替代 taskkill /T）
  ├── notify.rs   → 桌面通知（notify-rust；崩溃/重启/达上限/引导；失败静默）
  ├── status.rs   → 管理页（std 零依赖 HTTP：状态+安装 Node/dsh+启停）
  ├── wizard.rs   → 首次引导（缺失自动正常安装 node/dsh → 装完重试启动）
  ├── config.rs   → 配置加载（doctor_mode / status_port 字段）
  └── main.rs     → 入口（status/stop/config edit/doctor 子命令 + --no-tray 无头 + 托盘降级 + 管理页）
```

**只做**：进程守护。
**不做**：插件市场（归 dsh-market）、版本管理（归 npm）、状态面板（归 dsh web UI）。

## 2026-08-17/18 瘦身记录

- 9 文件 2,837 行 → 6 文件 1,236 行（-56%）
- 删除 4 文件：plugins.rs(362) / version.rs(243) / status_page.rs(124) / **updater.rs(293)**
  （updater.rs 在 slimming-plan 里漏列，实际也被删；2026-08-18 补记）
- 另删 examples/gen_icon.rs（引用已移除的 resvg 依赖，`cargo test` 会挂）与 dev-dep png
- tray.rs：852→261 行，菜单 10+→5 项（状态行/打开/重启/日志/退出）
- wizard.rs：317→53 行，移除 tiny_http 向导页
- Cargo.toml 移除 4 依赖：resvg/winreg/tiny_http + png(dev)
- assets 清理：删除 status.html + wizard.html
- 2026-08-18 修编译：config.rs 原引用已删的 `crate::version::DEFAULT_PIN` → 内联常量（编译不过）

## 保留的 11 个文件

| 文件 | 行数 | 职责 |
|------|------|------|
| doctor.rs | 873 | 证据驱动自愈诊疗（扫描→分级→按模式处置→兜底） |
| supervisor.rs | ~660 | 进程守护 + 退避重启 + 三层健康探测 + 崩溃逐级诊疗 + state/control IPC |
| installer.rs | ~300 | 环境探测 + 异步安装（winget/npm）+ 安装状态（管理页用） |
| tray.rs | ~280 | 系统托盘 + 菜单（打开界面置顶/关闭引擎/3s 刷新） |
| runtime.rs | ~200 | 命令构建（dsh 直启，无 npx）+ 路径/运行器定位 + state/control 路径 |
| config.rs | ~80 | 配置加载（doctor_mode / status_port 字段） |
| main.rs | ~205 | 入口（子命令 + --no-tray + 托盘降级 + 管理页） |
| wizard.rs | ~110 | 首次引导 + 缺失自动安装 + 重试升级 |
| job.rs | ~95 | Windows Job Object：KILL_ON_JOB_CLOSE 整树受控 + terminate_job |
| notify.rs | ~25 | 桌面通知（notify-rust；summary/body API） |
| status.rs | ~180 | 管理页（HTML + 安装/启停 API） |

## come.patch.yml

dsh-come 维护的 patch overlay，经 `dsh --patch` 挂载到启动参数（cli-contract v2 C3）：
`%LOCALAPPDATA%\dsh-desktop\come.patch.yml`（`src/runtime.rs::ensure_come_patch` 幂等写入）

当前内容：`dsh-market.config.allowRestart: false`——禁用其 detached 一键重启（重启归壳
supervisor 管，防绕过崩溃自愈/退避/日志）。dsh-market 未安装时加载期仅 warn 一条，无副作用。

## 联动方式

- **→ dsh**：supervisor 启动 `dsh web`（端口 3080），HTTP 探测就绪后打开浏览器
- **→ md-agent**：supervisor 可选守护 md-agent 进程（未来）
- **→ md-hr**：不直接联动，通过 dsh 插件间接

## 自愈诊疗（doctor.rs）

证据驱动的自愈系统：扫描取证 → 推理分级 → 按模式授权处置 → 兜底升级。**不写死检查**，
所有「发现」来自对环境的实际扫描与推理（孤儿 file:// 入口 / 损坏配置 / 残缺下载 / 端口被占 /
孤儿进程），将来换别的插件、别的原因拖垮 dsh 也能识别。

- **模式阶梯**（规避「严苛」字眼，语义温和准确）：
  巡检 Inspect（只报不改）/ 处置 Treat（自动🟢绿，黄红只推荐）/ 主治 Attend（自动🟢绿+🟡黄，🔴红只推荐）/
  急救 Emergency（全量兜底，🔴红先备份）。失败逐级升级：巡检→处置→主治→急救→急救。
- **影响半径**：🟢绿=可逆零风险可自动（重建 come.patch.yml）/ 🟡黄=需确认或主治及以上（结束占端口进程、删孤儿配置条目、清残缺下载）/ 🔴红=先备份再动，仅急救（重置损坏的 cordis.patch.yml）。
- **接入点**：`main::run_first_boot(cfg, mode)` 默认处置；`wizard` 重试逐级升级（处置→主治→急救），
  no-runner 即停 + 重试上限 3 次；`supervisor` 监测线程每次崩溃按重启次数升级模式（1→处置/2→主治/≥3→急救），
  耗尽后跑一次 Emergency 兜底（emergency_used 防循环）再放弃。`dsh-come doctor [--mode X]` CLI 子命令可手动跑诊疗报告。
- **2026-08-18 加固（wmic→PowerShell + 防误杀）**：进程表改 `Get-CimInstance Win32_Process` 一次全量取
  （wmic 在 Win11 24H2+ 已弃用）；孤儿进程检测排除活引擎树（`engine_tree_pids` 从 supervisor pid 收子树）；
  孤儿按端口证据分级——含本端口 🟡黄主治自动、仅名字像 dsh 🔴红仅急救（防 CLI 急救误杀用户另开的实例）；
  契约例外声明见 docs/cli-contract.md「自愈诊疗例外」。验证：cargo test 15 全绿 + release 构建通过。
- **路径定位原则（用户硬要求：不写死绝对地址，扫描安装位置）**：
  - `runtime.rs` 全环境变量（DSH_HOME/USERPROFILE/LOCALAPPDATA/DSH_DESKTOP_HOME）+ PATH 扫描，无硬编码绝对路径。
  - `profile_patch_path()` 改为**扫描定位** cordis.patch.yml：先约定路径 `<home>/.dsh/profiles/web/cordis.patch.yml`，
    缺失则在 dsh 根目录内递归扫描同名文件（`scan_named`，有界深度 6，防大目录卡死），再退而扫启动器根（dsh-desktop）；
    都没命中才回退约定路径（上层据此判「无 patch」跳过）——dsh 改 profile 存放位置仍能发现。
  - `probe_partial_downloads` 扫「dsh 数据根 + 启动器根」两处，去重合并成一条发现（避免上百条噪音）。
  - 单测 `detects_orphan_file_entry` 用 `std::env::temp_dir()` 推导「必存在」路径，不依赖机器实际绝对路径。

## 认领语义（adopt，2026-08-18 用户拍板保留并完整修复）

端口已有**健康** dsh 运行时，壳**接管**而非重复启动（`owned=false`，`start()` 认领分支，见下「运行态澄清」）：

- **不杀外部进程**：stop/重启/退出只解除认领；`taskkill` 仅对壳 spawn 的引擎（`owned=true`）
- **周期探活 + 判死自动接管**：monitor 对 adopted 每 5s 探活（`adopt_probe` 纯函数：HTTP 200 或原 pid 仍监听 → 存活；
  端口换主人 → 更新认领目标；连续 3 次失败 → 判死 → **kill 残留 + spawn owned 实例接管**——保证 dsh
  一直运行（命令行 ctrl+C/关窗口 杀掉后壳自起；2026-08-18 由「只降级」改为「自动接管」）
- **重启后接管**：spawn 分支复位 `owned=true`——修掉「认领后手动重启 → 新实例退出时因 owned=false
  不杀 → 残留进程」衍生 bug
- **doctor 协调**：`probe_port` 区分健康占用者（→ 接管提示 🟢 不杀）与僵尸占用（HTTP 不 200 → 按分级
  杀 🟡），与认领语义一致
- **wizard 就绪超时**：spawn 成功但超时未就绪 → stop + 升级模式重试（不再静默放弃），阶梯统一用
  `Mode::for_restart`（与 monitor 同一套）；`doctor_mode` 配置接入 `run_first_boot`（不再死旋钮）

## 页面级守护（2026-08-18，三段式探活）

自有引擎（owned）的守护从「进程级」补到「页面级」——进程活着但 HTTP 挂（dsh web 内部崩/卡死）时
不再永久显示「运行中 ✓」：

- **三段式**（monitor 每 30s，仅对已就绪引擎探测，启动期不探防误杀）：
  首次失败 → `set_stage("界面无响应…")` 托盘提示（**提示与动作分离，用户先看见守护在干活**）；
  连续失败累积（每失败写一条日志留证据）；第 3 次判死 → `kill_tree` 杀进程树 → 下一轮 `try_wait`
  看到退出码 → **走既有「崩溃→退避重启+诊疗升级」链路**（restarts 预算/doctor 自动生效，不复制重启逻辑）
- 判死参数保守（30s×3 ≈ 1.5 分钟），滤掉 dsh 内部组件短暂重启的抖动；恢复 200 → 计数清零、清提示
- 纯函数 `page_probe`（可测）：200→Alive 清零 / 未超限→Degraded 提示 / 超限→Dead
- **死代码收尾**：`wipe_data`（v1 清空数据菜单配套，无调用方）已删除——git 历史可恢复，重置数据
  归 doctor 能力域；`set_stage` 借页面探活提示重新获得调用方（编译警告清零）

## 稳定性加固（2026-08-19）：Job Object + 监控自愈 + 看门狗

用户评估后**否决了换 Electron**——Electron 只是把同一套 Windows 进程管理逻辑用 JS 重写，且多一条
渲染进程崩溃面，守护进程仍无守护。改为 Rust 原生加固，零框架替换：

- **Windows Job Object（src/job.rs，新增）**：dsh-come 启动时建作业，`AssignProcessToJobObject`
  把 spawn 的 `cmd` immediate 子进程纳入，其后代（dsh.cmd → node）默认继承作业。
  `KILL_ON_JOB_CLOSE` 让 dsh-come 退出/崩溃时 OS 强杀整树 → 消除「守护进程崩→dsh 变孤儿占端口」。
  `terminate_job()`（stop/重启）由 OS 一次性杀整树，比 `taskkill /T` 可靠（不漏杀脱离的孙进程）。
  失败仅降级 taskkill（日志提示）。作业句柄故意不 CloseHandle（进程退出触发 KILL_ON_JOB_CLOSE）。
  Cargo.toml 给 windows-sys 加 `Win32_System_JobObjects` 特性。
- **监控线程防静默失效（supervisor.rs）**：
  - 锁中毒恢复：monitor 取锁 `Err(e) => e.into_inner()`，上一轮 panic 遗留的 PoisonError 不再让监控永久失明。
  - `doctor::heal` / `start` 的调用包 `panic::catch_unwind(AssertUnwindSafe(...))`：
    自动重启 / 急救兜底 / 认领接管三处若 panic，记日志「⚠️ 监测线程…panic，已捕获并继续守护」后 `continue`，
    监控线程不再静默死掉（这是「守护本身有守护」的最后一道）。
- **看门狗脚本（scripts/install-watchdog.ps1）**：注册计划任务「DSH伴侣守护」——登录启动 + 每分钟
  重复检查（已在运行则 IgnoreNew，配合单实例 mutex），给 dsh-come 自己一个守护：崩溃/被关后约 1 分钟
  自动复活。注意：主动从托盘退出也会被拉起（常驻预期）；不想自动复活删任务即可。
- **验证**：`cargo check` + `cargo test`（17 全绿）+ `cargo build --release`（dist/dsh-come.exe 已更新为 4.07MB 新构建）。

## 方案吸收（2026-08-19）：dsh-watchdog 方案里只吸收低成本高价值项

用户拿到一份「dsh-watchdog」完整规划（Rust+托盘+headless+npm 分发+tokio+多实例+OOM+webhook），
要求分析吸收哪些。判定：约 80% 是 dsh-come 已有能力的重写版；用户拍板走**推荐档**——只吸收
低成本高价值项，不重写、不跨平台、不碰 npm、不引 tokio。已落地：

- **无头模式升级（main.rs）**：`--no-tray` 从「死循环睡」改为 `headless_loop()`（ctrlc 优雅退出 →
  shutdown 清 dsh）；tray 创建失败（无桌面会话）不再整体 shutdown，而是**降级无头继续守护**
  （tray.rs `run_tray` 返回 `Result<(),String>`）。
- **桌面通知（notify.rs 新增，notify-rust）**：崩溃首次自动重启 / 连续崩溃达上限停止 / 认领接管
  三处弹系统通知。Windows toast 需 AUMID 才能保证弹出（未注册可能只进通知中心），失败静默不阻塞。
  注意 notify-rust 4 API 是 `.summary()/.body()` 不是 `.title()`（编译踩坑已记）。
- **探活升级（supervisor.rs `health_ok`）**：优先探测 `/api/health`（需 dsh 侧 cordis 插件暴露，
  见 `resources/dsh-health-plugin.js` 参考实现），缺失自动降级首页 `/`——不依赖插件也能探活。
  4 处调用（认领/就绪/页面/adopt）全部切到 `health_ok`。插件是**参考文件**：dsh 不自动加载
  `~/.dsh/plugins` 裸文件（走 cordis.yml 组合），需手动启用，看门狗侧无需改动。
- **CLI 控制（main.rs + supervisor IPC）**：新增 `status`（读 state.json JSON 输出）、
  `stop`（写 control.json，监测线程下一轮消费停 dsh，看门狗继续后台）、`config edit`（打开配置）、
  `start`（已在跑被单实例锁拦下）。实现：单实例锁结果区分「守护在跑/不在」，跨进程用
  `root_dir/state.json`（monitor 每轮写）+ `control.json`（stop 请求，消费后删除）。
- **测试坑**：monitor 循环末尾加 `drop(st); write_state()` 时，认领接管分支 `drop(st)` 后未
  `continue` → use-after-move 编译错；在接管分支末尾补 `continue` 修复（st 已 move，直接下一轮）。
- **验证**：cargo test 17 全绿 + release 构建（dist/dsh-come.exe 4.2MB，含通知库）+ CLI 冒烟
  （status 未运行 JSON / --help 正常 / 无头守护可常驻、跨进程 status/stop 文件流可用）。

## 待办

- [x] cargo build / cargo test 编译验证（2026-08-18 通过；5 测试全绿）
- [x] doctor.rs 自愈模块（2026-08-18 新增；15 单测全绿；已按要求改为扫描定位路径）
- [x] doctor.rs 加固（2026-08-18：wmic→PowerShell、防误杀活引擎、孤儿进程分级、wizard no-runner 即停）——cargo test 15 全绿 + release 构建通过，已重建 dist/dsh-come.exe
- [x] 认领收敛（2026-08-18：adopted 探活判死 + spawn owned 复位 + doctor 端口协调 + wizard 超时重试 + 单实例锁 + 日志轮转删旧 + npx 标识 + doctor_mode 接入 + 删死字段）——cargo test 16 全绿
- [x] 页面级守护（2026-08-18：三段式探活提示→累积→判死走崩溃链路；删 wipe_data 死代码；set_stage 接入）——cargo test 17 全绿、编译警告清零
- [ ] 考虑 supervisor 增加 md-agent 守护（integration-plan Phase 2，未启动）

## 运行态与配置澄清（2026-08-18）

- **md-hr 不是 dsh-come 装的**：全项目搜 `recruit-workbench`/`md-hr` 零命中；`come.patch.yml` 只含 `dsh-market`。来源是 dsh 自己的 profile `~/.dsh/profiles/web/cordis.patch.yml` 里的 `recruit-workbench`（`file:///C:/.../md-hr/src/adapter-dsh/index.ts`，通道B 硬加载）。dsh 每次 `dsh web` 都按此自动加载 md-hr。
- **2026-08-18 已按用户决策移除该 entry**（备份 `cordis.patch.yml.bak`）：`cordis.patch.yml` 现为空列表 `[]`，dsh 启动不再自动加载 md-hr——**md-hr 现为 opt-in**，需要时从 .bak 恢复 insert 条目。符合"壳只碰门把手"与"让用户主动决策"。
- **状态误报「未启动/正在尝试启动」根因**：`supervisor::start()` 只认自己 spawn 的 child 句柄；若 dsh 已外部/上次会话跑在 3080 上，新 dsh-come 的 STATE 全新 → 又 spawn 一个 → 端口冲突秒退 → 监测线程退避重启 → 状态在「启动中/已停止」抖动，真在服务的 dsh 不被认领。修复：`start()` 在 spawn 前先 `http_ok` 探端口，已就绪则**认领**（running/ready=true、pid=监听者、owned=false 不误杀），新增 `owned` 字段；`kill_child` 仅 `owned` 时杀。改 supervisor.rs + doctor.rs（listening_pid_on_port/parse_listening_pid 提为 pub(crate) 复用）。交其他 AI 编译验证。
