//! DSH 引擎守护（Rust 管家壳）：spawn dsh web / 退避重启 / HTTP 健康探测 / 滚动日志 / 杀进程树。
//!
//! 架构：壳只碰 dsh 的「门把手」——启动命令 / 端口探测 / 进程管理（docs/cli-contract.md），
//! 不依赖其内部 API，因此 DSH 发新版不会破坏壳。
//!
//! 与 md-agent engine.rs 的差异（本项目的改进点）：
//! - 启动走 **npx 通道**：`node npx-cli.js --yes @deepseek-ai/dsh@<ver> web ...`，
//!   版本解析/下载/缓存全交给 npm 生态，壳只维护 state.current 一个版本号
//!   （npx-cli.js 是 js 非 .cmd，node 直启免 cmd /C 包装）
//! - 崩溃重启用**指数退避**（md-agent 是固定 1s）
//! - 就绪探测用 **HTTP GET 200**（契约 C2），而非仅 TCP 可连
//! - spawn 时设置 cwd + DSH_HOME（契约 C3），数据/配置全隔离在启动器 home
//! - 日志滚动（>5MB 轮转 .1），而非无限追加

use crate::config::AppConfig;
use crate::runtime;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
    /// 当前版本（from state.current，托盘状态行展示）
    pub version: Option<String>,
    /// 连续重启次数
    pub restarts: u32,
    /// 当前阶段提示（首次安装/下载/启动中…，托盘状态行展示；空 = 无阶段）
    pub stage: String,
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
            stage: String::new(),
        }
    }
}

struct SuperState {
    child: Option<Child>,
    status: SuperStatus,
    /// 本次启动时刻：连续运行超 HEALTHY_RESET_SECS 视为健康，重启预算清零
    /// （md-agent 同款隐患：start() 内清零 restarts 会让崩溃上限永不触发 → 无限重启）
    last_start: std::time::Instant,
}

/// 连续存活超此时长（秒）→ 重启预算清零（短时间内反复崩溃才累计）
const HEALTHY_RESET_SECS: u64 = 30;

static STATE: OnceLock<Arc<Mutex<SuperState>>> = OnceLock::new();
static MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

fn state() -> &'static Arc<Mutex<SuperState>> {
    STATE.get_or_init(|| {
        Arc::new(Mutex::new(SuperState {
            child: None,
            status: SuperStatus::default(),
            last_start: std::time::Instant::now(),
        }))
    })
}

/// 当前引擎状态快照
pub fn status() -> SuperStatus {
    state().lock().map(|s| s.status.clone()).unwrap_or_default()
}

/// 设置当前阶段提示（首次安装/下载/启动中…；空字符串清除）——托盘状态行实时反馈
pub fn set_stage(s: &str) {
    if let Ok(mut st) = state().lock() {
        st.status.stage = s.to_string();
    }
}

/// 日志入口（供 updater / tray 等其他模块写引擎滚动日志）
pub fn log(line: &str) {
    append_log(line);
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
                let _ = std::fs::rename(&path, path.with_extension("log.1"));
            }
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {line}", chrono::Local::now().format("%H:%M:%S"));
    }
}

// ---------- 健康探测（契约 C2） ----------

/// HTTP GET 返回 200 即视为就绪（v1 只看状态码；后续加页面版本指纹，见 DESIGN §7.4）
pub fn http_ok(port: u16, timeout_ms: u64) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// 等待就绪：最多 `startup_timeout_secs` 秒内轮询 HTTP 200
fn wait_ready(port: u16, timeout_secs: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if http_ok(port, 1000) {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ---------- 启动 / 停止 / 重启 ----------

/// npx 通道参数（纯函数便于测试）：`--yes @deepseek-ai/dsh@<ver> web --host <host> --port <port>`
/// - `--yes`：非交互环境自动确认（首次下载 dsh 包时 npx 会提示，必须显式传）
/// - `@deepseek-ai/dsh@<ver>`：钉版本号（不追 latest，state.current 即锁定值）
/// - 之后参数全部透传给 dsh CLI → web app（契约 C1）
pub fn npx_argv(ver: &str, host: &str, port: u16) -> Vec<String> {
    vec![
        "--yes".to_string(),
        format!("@deepseek-ai/dsh@{ver}"),
        "web".to_string(),
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
    ]
}

/// 构造启动命令（契约 C1/C3）：node npx-cli.js <npx_argv>，cwd + DSH_HOME 隔离在启动器 home
fn build_command(cfg: &AppConfig, ver: &str) -> Result<(Command, PathBuf), String> {
    let node = runtime::node_exe();
    let npx = runtime::npx_cli_js();
    let home = runtime::home_dir();
    if !node.is_file() {
        return Err(format!(
            "未找到捆绑 Node（{}）。请先放置 portable Node（node\\node.exe）或运行打包脚本。",
            node.display()
        ));
    }
    if !npx.is_file() {
        return Err(format!(
            "未找到捆绑 npx-cli（{}）。请检查 portable Node 是否完整。",
            npx.display()
        ));
    }
    let mut cmd = Command::new(&node);
    cmd.arg(&npx)
        .args(npx_argv(ver, &cfg.host, cfg.port))
        .current_dir(&home)
        .env("DSH_HOME", &home);
    hide_window(&mut cmd);
    Ok((cmd, home))
}

/// 启动 dsh 引擎（已运行则幂等返回）。auto_restart 置 true → 异常退出自动拉起。
pub fn start(cfg: &AppConfig, ver: &str) -> Result<(), String> {
    let mut st = state().lock().map_err(|e| e.to_string())?;
    if st.child.is_some() {
        return Ok(()); // 已在跑，幂等
    }
    let (mut command, home) = build_command(cfg, ver)?;
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
            "启动 dsh 失败（node={} home={}）：{e}",
            runtime::node_exe().display(),
            home.display()
        )
    })?;
    let pid = child.id();
    st.child = Some(child);
    st.status.running = true;
    st.status.ready = false;
    st.status.port = cfg.port;
    st.status.pid = Some(pid);
    st.status.last_error = None;
    st.status.auto_restart = true;
    st.status.version = Some(ver.to_string());
    // 阶段提示：调用方（bootstrap 首次安装）可能已设置更明确的阶段（如「下载 DSH…」），
    // 只在为空时兜底「启动中…」
    if st.status.stage.is_empty() {
        st.status.stage = "启动中…".to_string();
    }
    // 注意：不在 start() 里清零 restarts——监测线程递增后被清零会让崩溃上限永不触发。
    // 预算清零由「健康期重置」负责（连续运行超 HEALTHY_RESET_SECS）。
    st.last_start = std::time::Instant::now();
    drop(st); // 释放锁再起后台线程

    append_log(&format!("dsh 引擎启动 pid={pid} port={} ver={ver}", cfg.port));
    ensure_monitor(cfg.clone());

    // 就绪探测线程：HTTP 200 后置 ready（托盘状态行 /「打开界面」使能）；清除阶段提示
    let p = cfg.port;
    let timeout = cfg.startup_timeout_secs;
    std::thread::spawn(move || {
        wait_ready(p, timeout);
        if let Ok(mut st) = state().lock() {
            st.status.ready = true;
            st.status.stage.clear();
        }
        let msg = if http_ok(p, 1000) {
            format!("界面就绪: http://127.0.0.1:{p}")
        } else {
            format!("启动超时（{timeout}s 内未见 HTTP 200），端口 {p}")
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

/// 杀进程树：taskkill /T 杀整棵树（node → 子进程），child.kill() 只杀直接子进程不够
fn kill_child(st: &mut SuperState) {
    if let Some(pid) = st.status.pid {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()]);
        hide_window(&mut cmd);
        let _ = cmd.status();
    }
    if let Some(mut c) = st.child.take() {
        let _ = c.kill();
    }
}

/// 重启（stop + start）。当前托盘菜单无重启项，v2（更新后重启）使用。
#[allow(dead_code)]
pub fn restart(cfg: &AppConfig, ver: &str) -> Result<(), String> {
    stop()?;
    start(cfg, ver)
}

/// 退出清理（main 退出钩子）：关自动重启 + 杀进程（防残留 Node 占端口）
pub fn shutdown() {
    let _ = stop();
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
            let mut st = match state().lock() {
                Ok(s) => s,
                Err(_) => continue,
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
                            let ver = st.status.version.clone().unwrap_or_default();
                            drop(st); // 释放锁再 start（start 会重新锁）
                            append_log(&format!("自动重启（{n}/{max}），退避 {delay}s"));
                            std::thread::sleep(Duration::from_secs(delay));
                            if let Err(e) = start(&cfg, &ver) {
                                append_log(&format!("重启失败: {e}"));
                            }
                            continue;
                        } else {
                            let n = st.status.restarts;
                            let msg = format!("连续崩溃 {n} 次，已停止自动重启（详见 engine.log）");
                            st.status.last_error = Some(msg.clone());
                            append_log(&msg);
                        }
                    } else {
                        st.status.last_error = None; // 手动 stop，正常
                    }
                }
                None => {
                    // 子进程存活：连续运行超健康期 → 重启预算清零（避免一次健康运行前的旧崩溃计数累加）
                    if st.status.restarts > 0 && st.last_start.elapsed() > Duration::from_secs(HEALTHY_RESET_SECS) {
                        st.status.restarts = 0;
                        st.status.last_error = None;
                        append_log("连续运行超健康期，重启预算已清零");
                    }
                }
            }
        }
    });
}

/// 指数退避：1,2,4,8,... 封顶 backoff_max_secs
fn backoff_delay(restart_n: u32, max_secs: u64) -> u64 {
    let exp = 1u64 << (restart_n.saturating_sub(1).min(30));
    exp.min(max_secs.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npx_argv_pins_version_and_passes_port() {
        let argv = npx_argv("0.1.0-rc.6", "127.0.0.1", 3080);
        assert_eq!(
            argv,
            vec![
                "--yes",
                "@deepseek-ai/dsh@0.1.0-rc.6",
                "web",
                "--host",
                "127.0.0.1",
                "--port",
                "3080",
            ]
        );
        // 版本号被钉死（不追 latest），这是「监控渠道」与「盲目 latest」的区别
        assert!(argv.iter().any(|a| a == "@deepseek-ai/dsh@0.1.0-rc.6"));
    }

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
}
