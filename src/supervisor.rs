//! DSH 引擎守护（Rust 管家壳）：spawn dsh web / 退避重启 / HTTP 健康探测 / 滚动日志 / 杀进程树。
//!
//! 架构：壳只碰 dsh 的「门把手」——启动命令 / 端口探测 / 进程管理（docs/cli-contract.md），
//! 不依赖其内部 API，因此 DSH 发新版不会破坏壳。
//!
//! 与 md-agent engine.rs 的差异（本项目的改进点）：
//! - 启动走 **系统 dsh 命令**（PATH 直启，无 npx 临时拉取回退——2026-08-19 用户拍板）：
//!   dsh 缺失就正常安装（src/installer.rs：npm install -g @deepseek-ai/dsh），
//!   壳不做版本管理/数据隔离，跟随系统 dsh 的「正常设计逻辑」（docs/cli-contract.md）
//! - 崩溃重启用**指数退避**（md-agent 是固定 1s）
//! - 就绪探测用 **HTTP GET 200**（契约 C2），而非仅 TCP 可连
//! - 不设置 DSH_HOME：dsh 用其系统默认目录（%USERPROFILE%\.dsh），与终端里用法完全一致
//! - 日志滚动（>5MB 轮转 .1），而非无限追加

use crate::config::AppConfig;
use crate::runtime;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

/// 引擎状态（托盘状态行读取）
#[derive(Debug, Clone, serde::Serialize)]
pub struct SuperStatus {
    pub running: bool,
    /// 已通过 HTTP 健康探测（可打开界面）
    pub ready: bool,
    pub port: u16,
    pub pid: Option<u32>,
    pub last_error: Option<String>,
    /// 是否自动重启（手动 start 置 true、手动 stop 置 false；异常退出自动拉起）
    pub auto_restart: bool,
    /// 当前版本（系统 dsh `--version` 探测；不可得 → None，状态行显示「系统 dsh」）
    pub version: Option<String>,
    /// 连续重启次数
    pub restarts: u32,
    /// 该 dsh 是否由本进程 spawn（拥有）：false = 认领的外部/残留 dsh，
    /// stop/重启不误杀它（只解除认领）
    pub owned: bool,
    /// 当前阶段提示（首次安装/下载/启动中…，托盘状态行展示；空 = 无阶段）
    pub stage: String,
    /// 阶段已持续的秒数（stage 非空时才有；托盘/向导显示「已 X 分 Y 秒」）
    pub stage_elapsed: Option<u64>,
}

impl Default for SuperStatus {
    fn default() -> Self {
        Self {
            running: false,
            ready: false,
            port: 3080,
            pid: None,
            last_error: None,
            auto_restart: false,
            version: None,
            restarts: 0,
            owned: true,
            stage: String::new(),
            stage_elapsed: None,
        }
    }
}

struct SuperState {
    child: Option<Child>,
    status: SuperStatus,
    /// 本次启动时刻：连续运行超 HEALTHY_RESET_SECS 视为健康，重启预算清零
    /// （md-agent 同款隐患：start() 内清零 restarts 会让崩溃上限永不触发 → 无限重启）
    last_start: std::time::Instant,
    /// stage 被设置的时刻（status() 据此计算 stage_elapsed；None = 无阶段）
    stage_since: Option<std::time::Instant>,
    /// 急救兜底是否已用过（防止崩溃上限耗尽后无限循环 Emergency）
    emergency_used: bool,
    /// 上次对认领的外部 dsh 探活的时刻（monitor 每 ADOPT_PROBE_INTERVAL_SECS 探一次）
    last_adopt_probe: std::time::Instant,
    /// 认领探活连续失败次数（http 不 200 且端口无人/原 pid 监听）；≥3 判定外部 dsh 已死
    adopt_misses: u32,
    /// 上次对自有引擎页面探活的时刻（monitor 每 PAGE_PROBE_INTERVAL_SECS 探一次）
    last_page_probe: std::time::Instant,
    /// 页面探活连续失败次数；≥3 判定界面无响应 → 杀进程走既有崩溃重启链路
    page_misses: u32,
}

/// 连续存活超此时长（秒）→ 重启预算清零（短时间内反复崩溃才累计）
const HEALTHY_RESET_SECS: u64 = 30;

/// 认领的外部 dsh 探活周期（秒）：monitor 每 1s 循环，但探活节流到该间隔
const ADOPT_PROBE_INTERVAL_SECS: u64 = 5;

/// 认领探活连续失败多少次判定外部 dsh 已死
const ADOPT_PROBE_MISS_LIMIT: u32 = 3;

/// 自有引擎页面探活周期（秒）：已就绪的 owned 引擎每 30s HTTP 探测一次。
/// 比 adopt 探活保守——判死会杀进程重启，窗口要滤掉 dsh 内部组件短暂重启的抖动。
const PAGE_PROBE_INTERVAL_SECS: u64 = 30;

/// 页面探活连续失败多少次判定界面无响应（约 1.5 分钟）
const PAGE_PROBE_MISS_LIMIT: u32 = 3;

static STATE: OnceLock<Arc<Mutex<SuperState>>> = OnceLock::new();
static MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);
/// 最近一次 engine.log 写入时刻（unix 秒）：心跳检测「日志静默」用
static LAST_LOG_AT: AtomicU64 = AtomicU64::new(0);

fn state() -> &'static Arc<Mutex<SuperState>> {
    STATE.get_or_init(|| {
        let now = std::time::Instant::now();
        Arc::new(Mutex::new(SuperState {
            child: None,
            status: SuperStatus::default(),
            last_start: now,
            stage_since: None,
            emergency_used: false,
            last_adopt_probe: now,
            adopt_misses: 0,
            last_page_probe: now,
            page_misses: 0,
        }))
    })
}

/// 当前引擎状态快照
pub fn status() -> SuperStatus {
    let mut out = SuperStatus::default();
    if let Ok(st) = state().lock() {
        out = st.status.clone();
        // stage 非空 → 补算已持续秒数（stage_since 由 set_stage 维护）
        if !out.stage.is_empty() {
            if let Some(since) = st.stage_since {
                out.stage_elapsed = Some(since.elapsed().as_secs());
            }
        }
    }
    out
}

/// 设置当前阶段提示（首次安装/下载/启动中…；空字符串清除）——托盘状态行实时反馈。
/// 同时记录 stage_since，供 status() 计算「已耗时」。
pub fn set_stage(s: &str) {
    if let Ok(mut st) = state().lock() {
        st.status.stage = s.to_string();
        st.stage_since = if s.is_empty() {
            None
        } else {
            Some(std::time::Instant::now())
        };
    }
}

/// 秒 → 人类可读时长：「45 秒」/「1 分 24 秒」（en：「45s」/「1m 24s」）。托盘/向导展示安装已耗时。
pub fn fmt_elapsed(secs: u64) -> String {
    if crate::i18n::is_en() {
        if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m {}s", secs / 60, secs % 60)
        }
    } else if secs < 60 {
        format!("{secs} 秒")
    } else {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 日志入口（供 tray / plugins 等其他模块写引擎滚动日志）
pub fn log(line: &str) {
    append_log(line);
}

// ---------- 瞬时提示（flash） ----------

/// 瞬时提示：托盘状态行短暂显示操作结果（插件装/卸、更新检查等），过期自动消失。
/// 独立于 stage——stage 是安装进度（长生命周期），flash 是一次性结果（12s 过期）。
static FLASH: OnceLock<Mutex<Option<(String, std::time::Instant)>>> = OnceLock::new();

/// 设置瞬时提示文案（覆盖旧提示；12s 后 status_line 不再显示）
pub fn set_flash(msg: &str) {
    let m = FLASH.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = m.lock() {
        *g = Some((msg.to_string(), std::time::Instant::now()));
    }
}

/// 未过期的 flash 文案；过期或从未设置 → None
pub fn flash() -> Option<String> {
    let m = FLASH.get()?;
    let g = m.lock().ok()?;
    let (msg, at) = g.as_ref()?;
    if at.elapsed() > Duration::from_secs(12) {
        None
    } else {
        Some(msg.clone())
    }
}

/// 子进程不弹控制台黑窗口（Windows）：spawn 的 node/taskkill 默认会创建控制台窗口，
/// 用户实测「经常弹出 nodejs 黑色窗口」即此因。stdout 重定向 ≠ 无窗口，
/// 必须显式 CREATE_NO_WINDOW。所有 spawn 点（引擎/taskkill/冒烟/插件）统一调用。
#[cfg(target_os = "windows")]
pub fn hide_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub fn hide_window(_cmd: &mut std::process::Command) {}

/// 让子进程落在独立进程组（Unix）：spawn 前调用，`kill -pgid` 可整树清理。
/// Windows 无需此设置（Job Object / taskkill /T 负责进程树）。
#[cfg(not(target_os = "windows"))]
pub fn start_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(target_os = "windows")]
pub fn start_process_group(_cmd: &mut std::process::Command) {}

// ---------- 滚动日志 ----------

/// 日志追加：超 5MB 把 engine.log 轮转为 engine.log.1（丢弃更旧的），再继续追加
fn append_log(line: &str) {
    let path = runtime::engine_log();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if path.exists() {
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 5 * 1024 * 1024 {
                // 先删旧的 .log.1 再轮转（Windows 上 rename 覆盖已存在目标会失败）
                let _ = std::fs::remove_file(path.with_extension("log.1"));
                let _ = std::fs::rename(&path, path.with_extension("log.1"));
            }
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {line}", chrono::Local::now().format("%H:%M:%S"));
        // 记录最近写入时刻：心跳检测（heartbeat_if_silent）据此判断「日志静默」
        LAST_LOG_AT.store(unix_secs(), Ordering::Relaxed);
    }
}

// ---------- 健康探测（契约 C2） ----------

/// HTTP GET 返回 200 即视为就绪（v1 只看状态码；后续加页面版本指纹，见 DESIGN §7.4）。
fn http_ok_path(port: u16, timeout_ms: u64, path: &str) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .send()
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// 兼容旧调用：探首页 `/`。
pub fn http_ok(port: u16, timeout_ms: u64) -> bool {
    http_ok_path(port, timeout_ms, "/")
}

/// 健康探测：优先专用健康口 `/api/health`（需 dsh 侧插件暴露，见 resources/dsh-health-plugin.js），
/// 缺失自动降级到首页 `/` —— 不依赖插件也能探活。
pub fn health_ok(port: u16, timeout_ms: u64) -> bool {
    http_ok_path(port, timeout_ms, "/api/health") || http_ok_path(port, timeout_ms, "/")
}

/// 心跳：engine.log 静默超过 15s 且仍在阶段提示中（npm 非 TTY 输出被抑制，用户看不到
/// 动静）→ 追加一行「仍在进行…」，让托盘/向导/日志都能看出没有卡死。写日志会刷新
/// LAST_LOG_AT，故最多每 15s 打一次，不会刷屏。
fn heartbeat_if_silent() {
    const SILENT_BEFORE_HEARTBEAT: u64 = 15;
    let now = unix_secs();
    let last = LAST_LOG_AT.load(Ordering::Relaxed);
    if last == 0 || now.saturating_sub(last) < SILENT_BEFORE_HEARTBEAT {
        return;
    }
    let st = status();
    if st.stage.is_empty() {
        return; // 无阶段提示（已就绪/已停止）不打扰
    }
    append_log(&format!(
        "仍在进行：{stage}（npm 安装期输出少属正常，请耐心等待）",
        stage = st.stage
    ));
}

// ---------- 启动 / 停止 / 重启 ----------

/// 构造启动命令（契约 C1）：系统 dsh 直启（`dsh web --host … --port …`）。
/// dsh 命令经 cmd /C 包装（Windows .cmd 包装脚本 CreateProcess 不能直接执行）；
/// 不设置 DSH_HOME，dsh 用其系统默认目录，与终端里正常使用完全一致。
fn build_command(cfg: &AppConfig) -> Result<Command, String> {
    let runner = runtime::dsh_runner().ok_or_else(|| {
        "未找到系统 dsh。请先在管理页（http://127.0.0.1:<status_port>）安装 dsh，或执行 `npm install -g @deepseek-ai/dsh`。".to_string()
    })?;
    let mut args = vec![
        "web".to_string(),
        "--host".to_string(),
        cfg.host.clone(),
        "--port".to_string(),
        cfg.port.to_string(),
    ];
    // 壳 patch overlay（come.patch.yml，dsh-market 配置等）经 dsh CLI --patch 传入。
    // 注意：dsh 0.1.0-rc.6 的 `web` 子命令**拒绝父级参数**——`dsh --patch x web …`
    // 会报 "web takes none of parent --profile, --patch, …" 并退出(1)（2026-08-20 实测），
    // 所以 --patch 必须放在 `web` 之后：`dsh web --patch x --host … --port …`（实测可行）。
    if let Some(p) = come_patch_arg() {
        args.insert(1, p.display().to_string());
        args.insert(1, "--patch".to_string());
    }
    let mut cmd = runtime::dsh_command(&runner, &args);
    // 非 TTY（被重定向进 engine.log）时 npm 静默抑制进度条 → 用 npm_config_loglevel=http
    // 让 npm 每发一个 HTTP 请求打一行（npm http fetch GET 200 …），engine.log 有持续的
    // 「活着」信号；FORCE_COLOR=0 去 ANSI 颜色码。
    cmd.env("npm_config_loglevel", "http").env("FORCE_COLOR", "0");
    // Unix：独立进程组（引擎+node 后代同一组），杀树 kill -pgid 覆盖整树
    start_process_group(&mut cmd);
    hide_window(&mut cmd);
    Ok(cmd)
}

/// 壳 patch overlay 参数：come.patch.yml 存在才传（不存在 = 无 overlay 需求，不传）。
fn come_patch_arg() -> Option<std::path::PathBuf> {
    let p = crate::runtime::come_patch_path();
    p.is_file().then_some(p)
}

/// 启动 dsh 引擎（已运行则幂等返回）。auto_restart 置 true → 异常退出自动拉起。
pub fn start(cfg: &AppConfig) -> Result<(), String> {
    let mut st = state().lock().map_err(|e| e.to_string())?;
    // 认领已在运行的 dsh：本进程 STATE 是全新时（dsh-come 刚启动 / 重启），若端口已被
    // 外部或上次会话残留的 dsh 占住，直接再 spawn 一个会因端口冲突秒退、监测线程不断重启，
    // 状态在「启动中…/已停止」间抖动——而真正服务的那个 dsh 它从不认领，正是用户看到的现象。
    // 端口已被健康 dsh 占用 → 直接认领（owned=false，stop/重启不误杀外部进程）。
    if health_ok(cfg.port, 1500) {
        if let Some(pid) = crate::doctor::listening_pid_on_port(cfg.port) {
            st.status.running = true;
            st.status.ready = true;
            st.status.pid = Some(pid);
            st.status.owned = false; // 非本进程启动，stop/重启不杀它
            st.status.auto_restart = false; // 外部进程，我们不负责崩溃重启
            st.status.stage.clear();
            st.status.last_error = None;
            st.status.version = runtime::resolved_version(cfg);
            st.adopt_misses = 0;
            st.page_misses = 0;
            st.last_adopt_probe = std::time::Instant::now();
            drop(st); // 释放锁再起监测线程
            let ver = runtime::resolved_version(cfg).unwrap_or_else(|| "系统 dsh".to_string());
            append_log(&format!(
                "检测到 dsh 已在端口 {} 运行（pid={}），已认领，不再重复启动（{ver}）",
                cfg.port, pid
            ));
            ensure_monitor(cfg.clone());
            return Ok(());
        }
    }
    if st.child.is_some() {
        return Ok(()); // 已在跑，幂等
    }
    let home = runtime::system_home_dir();
    let runner_desc = runtime::dsh_runner().map(|r| r.describe()).unwrap_or_else(|| "（无）".to_string());
    let mut command = build_command(cfg)?;
    // stdout/stderr 进滚动日志（管道不读会写满阻塞子进程——md-agent 踩坑）
    let log_path = runtime::engine_log();
    if let Some(dir) = log_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        command.stdout(Stdio::from(f.try_clone().map_err(|e| e.to_string())?));
        command.stderr(Stdio::from(f));
    } else {
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    }
    let child = command.spawn().map_err(|e| {
        format!(
            "启动 dsh 失败（{}，DSH_HOME={}）：{e}",
            runner_desc,
            home.display()
        )
    })?;
    let pid = child.id();
    // 纳入 Job Object（Windows）：整树（cmd→dsh.cmd→node）受 KILL_ON_JOB_CLOSE 约束——
    // dsh-come 崩溃/退出时 OS 强杀整树，消除「守护进程崩→dsh 变孤儿占端口」。
    // 失败仅记日志降级（仍可用 taskkill /T），不影响启动。
    // Unix：记录进程组根 pid 供 kill_tree（kill -pgid）整树清理。
    #[cfg(target_os = "windows")]
    if !crate::job::assign_child(pid) {
        append_log("⚠️ 引擎未纳入 Job Object（降级 taskkill /T；崩溃自愈仍可工作，但崩溃兜底稍弱）");
    }
    #[cfg(not(target_os = "windows"))]
    crate::job::record_engine_pid(pid);
    st.child = Some(child);
    st.status.running = true;
    st.status.ready = false;
    st.status.port = cfg.port;
    st.status.pid = Some(pid);
    st.status.owned = true; // 本进程 spawn 的引擎；退出时 taskkill 整树（外部认领的 dsh 不杀）
    st.status.last_error = None;
    st.status.auto_restart = true;
    st.status.version = runtime::resolved_version(cfg);
    st.adopt_misses = 0;
    st.page_misses = 0;
    // 阶段提示：调用方可能已设置更明确的阶段（如「启动系统 dsh…」），
    // 只在为空时兜底「启动中…」
    if st.status.stage.is_empty() {
        st.status.stage = crate::i18n::tr("启动中…", "Starting…").to_string();
    }
    // 注意：不在 start() 里清零 restarts——监测线程递增后被清零会让崩溃上限永不触发。
    // 预算清零由「健康期重置」负责（连续运行超 HEALTHY_RESET_SECS）。
    st.last_start = std::time::Instant::now();
    drop(st); // 释放锁再起后台线程

    let ver = runtime::resolved_version(cfg).unwrap_or_else(|| "系统 dsh".to_string());
    append_log(&format!("dsh 引擎启动 pid={pid} port={} ver={ver}（{runner_desc}）", cfg.port));
    ensure_monitor(cfg.clone());

    // 就绪探测线程：HTTP 200 后置 ready（托盘状态行 /「打开界面」使能）；清除阶段提示。
    let p = cfg.port;
    let startup_timeout = cfg.startup_timeout_secs;
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(startup_timeout);
        let mut ready_ok = false;
        while std::time::Instant::now() < deadline {
            if health_ok(p, 1000) {
                ready_ok = true;
                break;
            }
            heartbeat_if_silent(); // 首次安装/下载静默时给日志注入「还在干活」心跳
            std::thread::sleep(Duration::from_millis(500));
        }
        if let Ok(mut st) = state().lock() {
            // 只有真的探测到 HTTP 200 才置 ready；超时则保持 ready=false，
            // 让向导的 Starting→超时失败分支有机会触发，向导会显示「未就绪」并给重试按钮
            if ready_ok {
                st.status.ready = true;
            }
            // 超时：清 stage（否则一直显示「启动中…」像卡住），
            // 但不记 last_error（监测线程会根据进程状态决定是否重启）
            st.status.stage.clear();
        }
        let msg = if ready_ok {
            format!("界面就绪: http://127.0.0.1:{p}")
        } else {
            format!("启动超时（{startup_timeout}s 内未见 HTTP 200），端口 {p} — 向导将显示失败并重试")
        };
        append_log(&msg);
    });
    Ok(())
}

/// 停止引擎（auto_restart 置 false，防监测线程重启）；kill 进程树（node 链）
pub fn stop() -> Result<(), String> {
    let mut st = state().lock().map_err(|e| e.to_string())?;
    st.status.auto_restart = false;
    st.status.ready = false;
    kill_child(&mut st);
    st.status.running = false;
    st.status.pid = None;
    Ok(())
}

/// 杀进程树：无论 owned 与否统一真正关闭（2026-08-19 用户要求「关闭引擎不区分内外」）。
/// 注意：外部认领的 dsh **不在 Job Object 内**，terminate_job 对空作业返回成功却杀不到——
/// 所以 Job 强杀（壳 spawn 整树，最可靠）与 taskkill 兜底（外部认领进程）**都执行**。
fn kill_child(st: &mut SuperState) {
    // 1) Job Object 强杀整树（覆盖壳 spawn 的 dsh：cmd→dsh.cmd→node 孙进程，不漏杀）
    let _ = crate::job::terminate_job();
    // 2) 对当前 pid 兜底杀进程树（覆盖外部认领的 dsh——不在 job 内；对 job 内已死进程无副作用）
    if let Some(pid) = st.status.pid {
        kill_tree(pid);
    }
    if let Some(mut c) = st.child.take() {
        let _ = c.kill();
    }
}

/// 重启（stop + start）。托盘「重启引擎」菜单使用。
pub fn restart(cfg: &AppConfig) -> Result<(), String> {
    stop()?;
    start(cfg)
}

/// 退出清理（main 退出钩子）：按托盘复选框「退出时关闭引擎」决定是否杀 dsh。
/// - 勾选（默认）→ 关闭自动重启 + 杀进程（防残留 Node 占端口）
/// - 未勾选 → 保留引擎运行（dsh 继续服务，下次启动 start() 认领接管）
pub fn shutdown() {
    if crate::config::load().exit_close_engine {
        let _ = stop();
    } else {
        append_log("退出 dsh-come（按设置保留引擎运行，dsh 继续服务，下次启动将认领）");
    }
}

// ---------- 状态持久化（供 CLI `status` 跨进程读取） ----------

/// 持久化当前状态到 state.json（监测线程每轮调用）。失败静默（不影响守护）。
pub fn write_state() {
    let st = status();
    if let Ok(s) = serde_json::to_string_pretty(&st) {
        let p = runtime::state_path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, s);
    }
}

/// 读取 state.json（CLI `status` 用）；缺失/异常返回「暂不可用」占位 JSON。
pub fn read_state_json() -> String {
    let p = runtime::state_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => s,
        Err(_) => "{\"running\":false,\"message\":\"状态文件暂不可用（守护刚启动？）\"}".to_string(),
    }
}

/// state.json 距上次写入的秒数；文件缺失/不可读 → None。
/// 监测线程每轮（~1s）重写 state.json，**文件 mtime 即守护心跳**——
/// CLI `status` 据此判断守护是否存活（审计 P2-4：单实例锁在创建失败时退化无锁，
/// 用它判活会把「守护其实在跑」误报成「未运行」；心跳与锁语义解耦）。
pub fn state_stale_secs() -> Option<u64> {
    let meta = std::fs::metadata(runtime::state_path()).ok()?;
    let mtime = meta.modified().ok()?;
    // mtime 略超前（时钟偏移）→ elapsed 报错：按 0 秒（新鲜/存活）处理，
    // 避免把「守护在跑」误报成「未运行」。
    Some(mtime.elapsed().map(|d| d.as_secs()).unwrap_or(0))
}

/// 发送停止请求：写 control.json（CLI `stop` 用）；监测线程下一轮消费。
pub fn request_stop() {
    let p = runtime::control_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&p, "{\"stop\":true}");
    }
}

/// 发送停止请求并等待生效：监测线程每 1s 消费 control.json 停引擎并重写 state.json，
/// 这里轮询其 `running` 字段（≤2.5s，覆盖两轮调度抖动），避免调用方紧随的
/// `status` 读到「仍在运行」的旧值。返回 (是否已确认停止, 最终 state.json 原文)。
pub fn request_stop_and_wait() -> (bool, String) {
    request_stop();
    let deadline = std::time::Instant::now() + Duration::from_millis(2500);
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let s = read_state_json();
        let running = serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .and_then(|v| v["running"].as_bool());
        if running == Some(false) {
            return (true, s);
        }
        if std::time::Instant::now() >= deadline {
            return (false, s);
        }
    }
}

/// 监测线程每轮调用：若收到停止请求则停掉 dsh（auto_restart=false，防重启）并清请求。
/// 不持锁调用（内部 stop() 自行加锁）。返回是否处理了停止请求。
pub fn consume_stop_request() -> bool {
    let p = runtime::control_path();
    if !p.is_file() {
        return false;
    }
    let _ = std::fs::remove_file(&p);
    let _ = stop();
    true
}

// ---------- 监测线程（退避重启） ----------

/// 确保监测线程存在：每 1s 查子进程退出状态，异常退出且 auto_restart → 指数退避自动重启
/// 退避序列：1s,2s,4s,...封顶 backoff_max_secs；连续超过 max_restarts 次后放弃并记录 last_error
fn ensure_monitor(cfg: AppConfig) {
    if MONITOR_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            // 消费停止请求（CLI `stop` 写 control.json）：不持锁，内部 stop() 自行加锁
            let _ = consume_stop_request();
            // 锁中毒恢复：上一轮若 panic 遗留 PoisonError，into_inner 取回守卫继续守护，
            // 而不是 silently `continue` 让监控永久失明。
            let mut st = match state().lock() {
                Ok(s) => s,
                Err(e) => e.into_inner(),
            };
            let exit = st
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten());
            match exit {
                Some(code) => {
                    // 子进程已退出
                    st.child = None;
                    st.status.running = false;
                    st.status.ready = false;
                    st.status.pid = None;
                    let msg = format!("dsh 引擎已退出（code={code}）");
                    append_log(&msg);
                    if st.status.auto_restart {
                        let max = cfg.max_restarts;
                        if st.status.restarts < max {
                            st.status.restarts += 1;
                            let n = st.status.restarts;
                            let delay = backoff_delay(n, cfg.backoff_max_secs);
                            // 崩溃自愈：按重启次数升级诊疗模式（处置→主治→急救），
                            // 把「先检测→推荐执行→兜底急救」落到每次重启前。
                            let mode = crate::doctor::Mode::for_restart(n);
                            let cfg2 = cfg.clone();
                            let label = format!(
                                "自动重启（{n}/{max}），退避 {delay}s；重启前跑诊疗（模式={}）",
                                mode.label()
                            );
                            drop(st); // 释放锁再 heal/start（二者会重新锁）
                            append_log(&label);
                            if n == 1 {
                                crate::notify::toast(
                                    crate::i18n::tr("DSH 伴侣", "DSH Companion"),
                                    crate::i18n::tr("引擎已崩溃，正在自动重启…", "Engine crashed; restarting automatically…"),
                                );
                            }
                            // catch_unwind：heal/start 万一 panic，监控线程不能静默死掉——
                            // 捕获后继续守护（这是「守护本身有守护」的最后一道）。
                            let bad = panic::catch_unwind(AssertUnwindSafe(|| {
                                crate::doctor::heal(&cfg2, mode);
                                std::thread::sleep(Duration::from_secs(delay));
                                if let Err(e) = start(&cfg2) {
                                    append_log(&format!("重启失败: {e}"));
                                }
                            }));
                            if bad.is_err() {
                                append_log("⚠️ 监测线程自动重启时 panic，已捕获并继续守护（避免守护静默失效）");
                            }
                            continue;
                        } else if !st.emergency_used {
                            // 崩溃上限耗尽：跑一次「急救」兜底（最后手段），再试最后一次
                            st.emergency_used = true;
                            let cfg2 = cfg.clone();
                            let em_delay = backoff_delay(max, cfg.backoff_max_secs);
                            drop(st);
                            append_log(&format!(
                                "连续崩溃达上限，启动急救兜底（doctor::Emergency）后做最后一次尝试"
                            ));
                            let bad = panic::catch_unwind(AssertUnwindSafe(|| {
                                crate::doctor::heal(&cfg2, crate::doctor::Mode::Emergency);
                                std::thread::sleep(Duration::from_secs(em_delay));
                                if let Err(e) = start(&cfg2) {
                                    append_log(&format!("急救后仍失败: {e}"));
                                }
                            }));
                            if bad.is_err() {
                                append_log("⚠️ 监测线程急救兜底时 panic，已捕获并继续守护");
                            }
                            // 重新取锁，标记放弃（锁中毒则恢复继续）
                            if let Ok(mut s2) = state().lock() {
                                let n = s2.status.restarts;
                                let m = format!("连续崩溃 {n} 次，急救兜底后仍失败（详见 engine.log）");
                                s2.status.last_error = Some(m.clone());
                                append_log(&m);
                            }
                            continue;
                        } else {
                            let n = st.status.restarts;
                            let msg = format!("连续崩溃 {n} 次，急救兜底后仍失败，已停止自动重启（详见 engine.log）");
                            st.status.last_error = Some(msg.clone());
                            append_log(&msg);
                            crate::notify::toast(
                                crate::i18n::tr("DSH 伴侣", "DSH Companion"),
                                &format!(
                                    "{} {n} {}",
                                    crate::i18n::tr("引擎连续崩溃", "Engine crashed"),
                                    crate::i18n::tr("次，已停止自动重启", "times; auto-restart disabled")
                                ),
                            );
                        }
                    } else {
                        st.status.last_error = None; // 手动 stop，正常
                    }
                }
                None => {
                    if !st.status.owned && st.status.running {
                        // 认领的外部 dsh（无 child 句柄，try_wait 恒 None）：周期探活。
                        // 判定死亡 → 状态降级为已停止（不自动重启——外部进程不归本壳管；
                        // 用户可手动「重启引擎」，start() 的 spawn 分支会自起 owned 实例）。
                        if st.last_adopt_probe.elapsed() >= Duration::from_secs(ADOPT_PROBE_INTERVAL_SECS) {
                            st.last_adopt_probe = std::time::Instant::now();
                            let port_ok = health_ok(st.status.port, 1000);
                            let claimed = st.status.pid.unwrap_or(0);
                            let listener = crate::doctor::listening_pid_on_port(st.status.port);
                            let (misses, verdict) = adopt_probe(st.adopt_misses, port_ok, listener, claimed);
                            st.adopt_misses = misses;
                            match verdict {
                                AdoptProbe::Alive => {}
                                AdoptProbe::UpdatePid => {
                                    st.status.pid = listener;
                                    append_log(&format!("认领的 dsh 端口换主人（新 pid={listener:?}），已更新认领"));
                                }
                                AdoptProbe::Dead => {
                                    // 外部 dsh 已死：不再只降级——自动接管，spawn 一个 owned 实例，
                                    // 保证「dsh 一直运行」（命令行 ctrl+C/关窗口 杀掉后壳自起）。
                                    let dead_pid = st.status.pid.take();
                                    st.status.running = false;
                                    st.status.ready = false;
                                    st.status.last_error = None;
                                    st.adopt_misses = 0;
                                    let cfg2 = cfg.clone();
                                    drop(st); // 释放锁再 kill/spawn（start 会重新锁）
                                    append_log("认领的外部 dsh 已退出，由本壳接管：自动重启引擎");
                                    crate::notify::toast(
                                        crate::i18n::tr("DSH 伴侣", "DSH Companion"),
                                        crate::i18n::tr(
                                            "外部 dsh 已退出，已自动接管并重启",
                                            "External dsh exited; taken over and restarted",
                                        ),
                                    );
                                    if let Some(pid) = dead_pid {
                                        kill_tree(pid); // 原进程若残留（UI 卡但进程还在）→ 清掉防占端口
                                    }
                                    let bad = panic::catch_unwind(AssertUnwindSafe(|| {
                                        if let Err(e) = start(&cfg2) {
                                            append_log(&format!("接管重启失败: {e}"));
                                        }
                                    }));
                                    if bad.is_err() {
                                        append_log("⚠️ 接管重启时 panic，已捕获并继续守护");
                                    }
                                    continue; // st 已在上面 drop，直接进入下一轮（避免末尾再借 st）
                                }
                            }
                        }
                    } else if st.status.owned && st.status.ready && st.status.running {
                        // 自有引擎页面级守护：已就绪才探测（启动期不探，避免误杀冷启动/重启中的引擎）。
                        // 三段式：首次失败提示（托盘状态行「界面无响应」）→ 连续失败累积 →
                        // 判死杀进程树，让下一轮 try_wait 看到退出 → 走既有「崩溃→退避重启+诊疗升级」链路。
                        if st.last_page_probe.elapsed() >= Duration::from_secs(PAGE_PROBE_INTERVAL_SECS) {
                            st.last_page_probe = std::time::Instant::now();
                            let port_ok = health_ok(st.status.port, 1000);
                            let (misses, verdict) = page_probe(st.page_misses, port_ok);
                            st.page_misses = misses;
                            match verdict {
                                PageProbe::Alive => {
                                    if st.page_misses == 0 && !st.status.stage.is_empty() {
                                        st.status.stage.clear(); // 界面恢复，清掉无响应提示
                                    }
                                }
                                PageProbe::Degraded => {
                                    set_stage(&format!(
                                        "{}（{} {}）",
                                        crate::i18n::tr("界面无响应…", "UI unresponsive…"),
                                        st.page_misses,
                                        crate::i18n::tr("次探测失败", "probe failures")
                                    ));
                                    append_log(&format!(
                                        "页面探活失败 {}/{}：http://127.0.0.1:{}/ 不响应",
                                        st.page_misses, PAGE_PROBE_MISS_LIMIT, st.status.port
                                    ));
                                }
                                PageProbe::Dead => {
                                    append_log("页面探活连续失败，判定界面无响应，杀进程走重启链路");
                                    // 自有引擎在作业内 → 优先 terminate_job；作业不可用降级 taskkill
                                    if !crate::job::terminate_job() {
                                        if let Some(pid) = st.status.pid {
                                            kill_tree(pid);
                                        }
                                    }
                                    st.page_misses = 0; // 下一轮 try_wait 走崩溃链路，重置本计数
                                }
                            }
                        }
                    } else {
                        // 子进程存活：连续运行超健康期 → 重启预算清零（避免一次健康运行前的旧崩溃计数累加）
                        if st.status.restarts > 0 && st.last_start.elapsed() > Duration::from_secs(HEALTHY_RESET_SECS) {
                            st.status.restarts = 0;
                            st.status.last_error = None;
                            append_log("连续运行超健康期，重启预算已清零");
                        }
                    }
                }
            }
            // 持久化状态快照（CLI `status` 跨进程读取）；st 已无需，先释放锁再写
            drop(st);
            write_state();
        }
    });
}

/// 指数退避：1,2,4,8,... 封顶 backoff_max_secs
fn backoff_delay(restart_n: u32, max_secs: u64) -> u64 {
    let exp = 1u64 << (restart_n.saturating_sub(1).min(30));
    exp.min(max_secs.max(1))
}

/// 认领探活判定（纯函数，可测）：一次探活输入 → (新失败计数, 判定)。
/// - HTTP 200 → 存活，失败计数清零
/// - 端口仍被原 pid 监听但 HTTP 不 200 → 可能只是 UI 卡，失败 +1；连续超限判死
/// - 端口被别的 pid 监听 → 换主人，更新认领目标（计数清零）
/// - 端口无人监听 → 失败 +1；连续超限判死
fn adopt_probe(
    misses: u32,
    port_ok: bool,
    listener: Option<u32>,
    claimed_pid: u32,
) -> (u32, AdoptProbe) {
    if port_ok {
        return (0, AdoptProbe::Alive);
    }
    let miss = misses + 1;
    let dead = miss >= ADOPT_PROBE_MISS_LIMIT;
    match listener {
        Some(p) if p == claimed_pid => (miss, if dead { AdoptProbe::Dead } else { AdoptProbe::Alive }),
        Some(_) => (0, AdoptProbe::UpdatePid),
        None => (miss, if dead { AdoptProbe::Dead } else { AdoptProbe::Alive }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdoptProbe {
    /// 外部 dsh 仍健康，保持认领
    Alive,
    /// 端口换主人（pid 变了），更新认领目标
    UpdatePid,
    /// 判定外部 dsh 已死，解除认领（状态降级）
    Dead,
}

/// 页面探活判定（纯函数，可测）：一次 HTTP 探测 → (新失败计数, 判定)。
/// 三段式：200 → 存活清零；连续失败累积（提示阶段，不动作）；超限 → 判死（杀进程走重启链路）。
fn page_probe(misses: u32, port_ok: bool) -> (u32, PageProbe) {
    if port_ok {
        return (0, PageProbe::Alive);
    }
    let miss = misses + 1;
    if miss >= PAGE_PROBE_MISS_LIMIT {
        (miss, PageProbe::Dead)
    } else {
        (miss, PageProbe::Degraded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageProbe {
    /// HTTP 恢复 200：存活，清无响应提示
    Alive,
    /// 无响应但未判死：托盘状态行提示（不动作）
    Degraded,
    /// 连续失败超限：判界面无响应，杀进程走既有崩溃重启链路
    Dead,
}

/// 杀进程树（平台化）。
///
/// 两个平台的原始语义**不等价**，这是本函数存在的原因：
/// - Windows `taskkill /T` 杀的是**进程树**（按父子关系），天然安全。
/// - Unix `kill(-pid)` 杀的是**进程组**，它隐含假设 `pgid == pid`。
///
/// 该假设只在引擎由壳 spawn 时成立（`build_command` 调 `start_process_group` →
/// `process_group(0)`，子进程自成一组）。但**外部认领的 dsh 是用户在终端里启的**，
/// 它的 pgid 就是那个终端会话——直接组杀会把用户的 bash 及同会话内其他命令一起带走。
///
/// 因此 Unix 下先判定组长身份：自成一组才组杀，否则退回「单进程 + 递归后代」。
/// 注意这**不改变**「关闭引擎不区分是否本壳启动」的产品语义（见 `kill_child`）：
/// 外部 dsh 照样会被关掉，只是不再连带整个终端会话。
///
/// 页面探活判死用：杀掉后下一轮 monitor `try_wait` 看到退出码 → 走既有「崩溃 → 退避重启 +
/// 诊疗升级」链路，restarts 预算与 doctor 自动生效，无需复制重启逻辑。
fn kill_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()]);
        hide_window(&mut cmd);
        let _ = cmd.status();
    }
    #[cfg(unix)]
    {
        if process_is_group_leader(pid) {
            // 壳 spawn 的引擎：整组 TERM → 3s → 未退出再 KILL
            kill_group(pid);
        } else {
            // 外部认领：先自底向上清后代（防子进程变孤儿继续占端口），再杀本体
            let kids = descendant_pids(pid);
            for child in kids.iter().rev() {
                terminate_pid(*child);
            }
            terminate_pid(pid);
        }
    }
}

/// 该进程是否自成一个进程组的组长（`pgid == pid`）。
/// 为真 ⇒ 它是自己起的（壳 spawn 时 `process_group(0)`），组杀不会波及他人。
#[cfg(unix)]
fn process_is_group_leader(pid: u32) -> bool {
    // SAFETY: getpgid 是纯查询。返回 -1 表示进程已消失或无权限，
    // 按「不是组长」处理——走保守的单进程路径，宁可漏杀也不误杀。
    unsafe { libc::getpgid(pid as libc::pid_t) == pid as libc::pid_t }
}

/// 组杀（调用前已确认目标是组长）：先 TERM 给优雅退出机会，未果 SIGKILL。
#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: 负数 pid 表示「该 id 的进程组」；信号常量合法
    let pgid = -(pid as libc::pid_t);
    unsafe {
        let _ = libc::kill(pgid, libc::SIGTERM);
    }
    // 等 3s 让引擎收尾（保存状态/释放端口）
    std::thread::sleep(Duration::from_secs(3));
    // SAFETY: 0 号信号仅探活，不产生副作用
    if unsafe { libc::kill(pgid, 0) } == 0 {
        unsafe {
            let _ = libc::kill(pgid, libc::SIGKILL);
        }
    }
}

/// 终止单个进程：TERM → 短等 → 仍存活则 KILL。
#[cfg(unix)]
fn terminate_pid(pid: u32) {
    let p = pid as libc::pid_t;
    // SAFETY: 常规信号发送；pid 均来自进程表枚举，非负
    unsafe {
        let _ = libc::kill(p, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(300));
    if unsafe { libc::kill(p, 0) } == 0 {
        unsafe {
            let _ = libc::kill(p, libc::SIGKILL);
        }
    }
}

/// 递归收集 `root` 的所有后代 pid。
///
/// **仅 Linux 实现**：读 `/proc/<pid>/stat` 的 ppid 字段，纯文本解析，不依赖
/// `ps`/`pgrep`（容器与精简系统常缺失，且 BSD/BusyBox 输出格式不一）。
///
/// 其他 Unix（macOS/BSD）返回空，调用方退化为「只杀本体」。这是**有意的取舍**：
/// macOS 枚举子进程需 `sysctl(KERN_PROC_ALL)` 并自行定义 `kinfo_proc` 布局，
/// 而该结构体随系统版本变化、libc 并未提供，写错即内存不安全；
/// 相比之下，残留子进程（占端口）的危害远小于误杀用户整个终端会话。
#[cfg(target_os = "linux")]
fn descendant_pids(root: u32) -> Vec<u32> {
    // 先快照全部进程的 (pid, ppid)，再在内存里做 BFS，避免反复扫 /proc
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        // comm 字段可能含空格与括号（如 "node (worker)"），
        // 所以用**最后一个** ')' 切分，ppid 是其后第 2 个字段（第 1 个是 state）
        let Some(after_comm) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        let mut fields = after_comm.split_whitespace();
        let _state = fields.next();
        let Some(ppid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        pairs.push((pid, ppid));
    }

    bfs_descendants(&pairs, root)
}

/// 从 (pid, ppid) 快照里做 BFS，收集 `root` 的全部后代。
/// 抽成纯函数以便脱离 /proc 单测（进程树遍历的正确性不依赖读文件的方式）。
/// 返回顺序是「父在子前」，调用方 `.rev()` 即得自底向上（先杀子、后杀父）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bfs_descendants(pairs: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for &(pid, ppid) in pairs {
            // pid != parent 守卫：防御 /proc 里异常的自引用，避免死循环
            if ppid == parent && pid != parent && !out.contains(&pid) {
                out.push(pid);
                queue.push(pid);
            }
        }
    }
    out
}

#[cfg(all(unix, not(target_os = "linux")))]
fn descendant_pids(_root: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sequence_capped() {
        assert_eq!(backoff_delay(1, 30), 1);
        assert_eq!(backoff_delay(2, 30), 2);
        assert_eq!(backoff_delay(3, 30), 4);
        assert_eq!(backoff_delay(5, 30), 16);
        // 封顶
        assert_eq!(backoff_delay(9, 30), 30);
        assert_eq!(backoff_delay(100, 30), 30);
        // 至少 1s
        assert_eq!(backoff_delay(1, 0), 1);
    }

    #[test]
    fn fmt_elapsed_readable() {
        assert_eq!(fmt_elapsed(0), "0 秒");
        assert_eq!(fmt_elapsed(45), "45 秒");
        assert_eq!(fmt_elapsed(84), "1 分 24 秒");
        assert_eq!(fmt_elapsed(723), "12 分 3 秒");
    }

    #[test]
    fn adopt_probe_verdicts() {
        // HTTP 正常 → 存活，失败计数清零
        assert_eq!(adopt_probe(2, true, None, 100), (0, AdoptProbe::Alive));
        assert_eq!(adopt_probe(2, true, Some(100), 100), (0, AdoptProbe::Alive));
        // 端口换主人 → UpdatePid，计数清零
        assert_eq!(adopt_probe(1, false, Some(200), 100), (0, AdoptProbe::UpdatePid));
        // 原 pid 还在但 HTTP 不 200 → 累积；超限判死
        assert_eq!(adopt_probe(0, false, Some(100), 100), (1, AdoptProbe::Alive));
        assert_eq!(adopt_probe(2, false, Some(100), 100), (3, AdoptProbe::Dead));
        // 端口无人监听 → 累积；超限判死
        assert_eq!(adopt_probe(0, false, None, 100), (1, AdoptProbe::Alive));
        assert_eq!(adopt_probe(2, false, None, 100), (3, AdoptProbe::Dead));
        // 判定死后再探活恢复 → 计数清零
        assert_eq!(adopt_probe(3, true, None, 100), (0, AdoptProbe::Alive));
    }

    #[test]
    fn page_probe_three_stage() {
        // 200 → 存活清零
        assert_eq!(page_probe(2, true), (0, PageProbe::Alive));
        // 失败累积：前两次 Degraded（提示，不动作）
        assert_eq!(page_probe(0, false), (1, PageProbe::Degraded));
        assert_eq!(page_probe(1, false), (2, PageProbe::Degraded));
        // 第三次超限 → Dead（杀进程走重启链路）
        assert_eq!(page_probe(2, false), (3, PageProbe::Dead));
        // 判死后计数保持在超限值；恢复后清零
        assert_eq!(page_probe(3, true), (0, PageProbe::Alive));
    }

    // ---------- P0-2：Unix 进程树遍历（纯逻辑，不依赖 /proc，全平台可测） ----------

    /// BFS 收集整棵子树，且保持「父在子前」的顺序（调用方 .rev() 得到自底向上）。
    #[test]
    fn bfs_descendants_collects_whole_subtree_in_parent_first_order() {
        // 1 → 2 → 3 → 4（链），1 → 5（分支）；9 → 8 属于另一棵树
        let pairs = [(2u32, 1u32), (3, 2), (4, 3), (5, 1), (9, 8)];
        let got = bfs_descendants(&pairs, 1);
        for expect in [2, 3, 4, 5] {
            assert!(got.contains(&expect), "后代 {expect} 应被收集，实际 {got:?}");
        }
        assert!(!got.contains(&9), "不属于 root 子树的进程不应被收集");
        assert!(!got.contains(&1), "root 自身不算后代");

        let i2 = got.iter().position(|&p| p == 2).unwrap();
        let i3 = got.iter().position(|&p| p == 3).unwrap();
        let i4 = got.iter().position(|&p| p == 4).unwrap();
        assert!(i2 < i3 && i3 < i4, "应保持父在子前，实际 {got:?}");
    }

    /// /proc 里若出现异常自引用（pid == ppid），必须跳过而非死循环。
    #[test]
    fn bfs_descendants_survives_self_reference() {
        let pairs = [(7u32, 7u32), (8u32, 7u32)];
        let got = bfs_descendants(&pairs, 7);
        assert_eq!(got, vec![8], "自引用应被跳过且不陷死循环");
    }

    /// 空快照或 root 不在树中 → 空结果（非 Linux 平台退化路径同样走这里）。
    #[test]
    fn bfs_descendants_handles_empty_and_unrelated() {
        assert!(bfs_descendants(&[], 1).is_empty(), "空快照应返回空");
        assert!(bfs_descendants(&[(2, 1)], 99).is_empty(), "无关 root 应返回空");
    }
}
