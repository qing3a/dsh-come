//! 首次安装向导：本地 HTTP 服务 + 内嵌页面，向首次运行用户展示安装进度/失败原因/重试。
//!
//! 载体：`--app` 独立窗口（与引擎 UI 一致的 Edge/Chrome 无地址栏窗口，tray::open_browser）。
//! 页面是内嵌 exe 的静态 HTML（assets/wizard.html），轮询本模块的本地 HTTP 服务
//! （127.0.0.1:3177，被占则依次试 3178..3181）拿状态；进度文案直接复用 supervisor 的
//! 现有 stage 状态机（Node 下载百分比/解压/下载 DSH…），本模块只增加 phase/error 两个
//! 驱动层状态，避免与引擎状态双轨。
//!
//! 生命周期：向导页即启动页——首次显示安装进度（Node/DSH 下载解压），正常启动显示「启动中…」；
//! executor 线程跑 `crate::run_first_boot`（ensure_node → bootstrap），失败显示原因并等 `/api/retry`
//! 重跑；成功则置 ready，页面收到后**在同一窗口**跳转引擎页；页面已关时由托盘兜底打开
//! （handed_off 检测页面活跃，避免重复开窗口）。

use crate::config::AppConfig;
use crate::runtime;
use crate::supervisor;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

/// 向导阶段（页面渲染主驱动；进度细节来自 supervisor stage）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WizardPhase {
    /// 准备运行环境（Node 自举 / DSH 下载）
    Installing,
    /// 引擎已 spawn，等 HTTP 就绪
    Starting,
    /// 引擎就绪，界面已（由向导）打开
    Ready,
    /// 安装失败，页面显示 error + 重试
    Failed,
}

#[derive(Debug, Clone)]
struct WizardState {
    phase: WizardPhase,
    error: Option<String>,
}

struct WizardCore {
    state: Mutex<WizardState>,
    retry_tx: mpsc::Sender<()>,
    engine_url: String,
    /// 最近一次 /api/status 轮询时刻（页面活跃检测：就绪后页面自行跳转引擎页，
    /// 托盘据此判断是否兜底打开——避免向导窗口已跳转时再开一个）
    last_poll: Mutex<std::time::Instant>,
}

static CORE: OnceLock<Arc<WizardCore>> = OnceLock::new();

const WIZARD_HTML: &str = include_str!("../assets/wizard.html");
const FAVICON: &str = include_str!("../assets/favicon.svg");

const WIZARD_PORT_START: u16 = 3177;
const WIZARD_PORT_ATTEMPTS: u16 = 5;

/// 向导窗口是否仍活跃（页面在轮询 /api/status）。托盘 rebuild 在就绪后据此决定：
/// - 向导活跃 → 页面收到 ready 会自行跳转引擎页（同窗口），托盘不重复打开
/// - 向导已关（>30s 无轮询）→ 托盘照常自动打开引擎界面
pub fn handed_off() -> bool {
    let Some(core) = CORE.get() else { return false };
    let poll = core
        .last_poll
        .lock()
        .map(|t| *t)
        .unwrap_or(std::time::Instant::now());
    poll.elapsed() < Duration::from_secs(30)
}

/// 起向导：绑本地端口 → HTTP 服务线程 + 引导 executor 线程 → 打开向导页。
/// 端口全被占（极罕见）：降级为后台静默安装（不弹窗，行为同旧版）。
pub fn start(cfg: &AppConfig) {
    if CORE.get().is_some() {
        return; // 防御：单例已存在（同一进程只触发一次首次判定）
    }
    let (server, port) = match bind_wizard_server() {
        Some(v) => v,
        None => {
            supervisor::log("向导端口（3177-3181）均被占用，降级为后台静默安装");
            let cfg = cfg.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::run_first_boot(&cfg) {
                    supervisor::log(&format!("首次引导失败: {e}"));
                }
            });
            return;
        }
    };

    let (retry_tx, retry_rx) = mpsc::channel::<()>();
    let engine_url = format!("http://127.0.0.1:{}", cfg.port);
    let core = Arc::new(WizardCore {
        state: Mutex::new(WizardState {
            phase: WizardPhase::Installing,
            error: None,
        }),
        retry_tx,
        engine_url: engine_url.clone(),
        last_poll: Mutex::new(std::time::Instant::now()),
    });
    let _ = CORE.set(core.clone());

    // HTTP 服务线程：serve 向导页 + 状态轮询 + 重试/打开引擎/打开日志 + 壳管理页
    crate::status_page::set_port(port);
    let http_core = core.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            handle_request(&http_core, request);
        }
    });

    // 引导 executor：跑安装；失败显示并等重试；成功打开引擎窗口
    let exec_cfg = cfg.clone();
    std::thread::spawn(move || run_executor(exec_cfg, core, retry_rx));

    supervisor::log(&format!("首次向导已启动: http://127.0.0.1:{port}"));
    crate::tray::open_browser(&format!("http://127.0.0.1:{port}/"));
}

/// 依次尝试 3177..3181 绑定向导本地服务；返回 (server, port)，全失败 None
fn bind_wizard_server() -> Option<(tiny_http::Server, u16)> {
    for offset in 0..WIZARD_PORT_ATTEMPTS {
        let port = WIZARD_PORT_START + offset;
        if let Ok(server) = tiny_http::Server::http(("127.0.0.1", port)) {
            return Some((server, port));
        }
    }
    None
}

fn handle_request(core: &Arc<WizardCore>, request: tiny_http::Request) {
    let path = request.url().split('?').next().unwrap_or("");
    // 页面轮询即活跃信号（就绪后由页面自行跳转引擎页，无需壳再开窗口）
    if path == "/api/status" {
        if let Ok(mut t) = core.last_poll.lock() {
            *t = std::time::Instant::now();
        }
    }
    let (status, content_type, body) = match path {
        "/" => (200, "text/html; charset=utf-8", WIZARD_HTML.to_string()),
        "/favicon.svg" => (200, "image/svg+xml", FAVICON.to_string()),
        "/api/status" => (200, "application/json", status_json(core)),
        "/api/retry" => {
            let _ = core.retry_tx.send(());
            (200, "application/json", r#"{"ok":true}"#.to_string())
        }
        "/api/open-app" => {
            crate::tray::open_browser(&core.engine_url);
            (200, "application/json", r#"{"ok":true}"#.to_string())
        }
        "/api/open-logs" => {
            crate::tray::open_dir(&runtime::logs_dir());
            (200, "application/json", r#"{"ok":true}"#.to_string())
        }
        _ => {
            // 壳管理页路由（/desktop、/desktop/api/status）；未命中 → 404
            match crate::status_page::serve(path) {
                Some((code, ct, body)) => (code, ct, body),
                None => (404, "text/plain; charset=utf-8", "not found".to_string()),
            }
        }
    };
    let header = match tiny_http::Header::from_bytes(b"Content-Type", content_type.as_bytes()) {
        Ok(h) => h,
        Err(_) => return,
    };
    let response = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

/// 状态轮询响应：phase/error 来自向导自身，stage/running/ready/port 直接透传引擎状态
fn status_json(core: &WizardCore) -> String {
    let st = supervisor::status();
    let w = core
        .state
        .lock()
        .map(|s| s.clone())
        .unwrap_or(WizardState {
            phase: WizardPhase::Installing,
            error: None,
        });
    serde_json::json!({
        "phase": w.phase,
        "stage": st.stage,
        "running": st.running,
        "ready": st.ready,
        "port": st.port,
        "error": w.error,
        "engine_url": core.engine_url,
    })
    .to_string()
}

// ---------- 引导 executor ----------

fn run_executor(cfg: AppConfig, core: Arc<WizardCore>, retry_rx: mpsc::Receiver<()>) {
    loop {
        set_phase(&core, WizardPhase::Installing, None);
        match crate::run_first_boot(&cfg) {
            Ok(()) => {
                // 引擎已 spawn（bootstrap 内 start 即返回；就绪在 supervisor 后台线程轮询）
                set_phase(&core, WizardPhase::Starting, None);
                let deadline =
                    std::time::Instant::now() + Duration::from_secs(cfg.startup_timeout_secs);
                loop {
                    if supervisor::status().ready {
                        // 页面收到 ready 自行跳转引擎页（同窗口导航，不新开窗口）；
                        // 向导窗口已关时才由托盘兜底打开（handed_off 检测）
                        set_phase(&core, WizardPhase::Ready, None);
                        supervisor::log("首次安装完成，向导页将跳转到引擎界面");
                        return;
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                fail(
                    &core,
                    "引擎在限定时间内未就绪（未见 HTTP 200）。可点重试，或查看日志确认 dsh 启动输出。",
                );
            }
            Err(e) => fail(&core, &diagnose(e)),
        }
        // 失败：阻塞等用户点「重试」（向导页 POST /api/retry）
        let _ = retry_rx.recv();
        supervisor::set_stage(""); // 清残留 stage，重跑时重新显示
    }
}

fn set_phase(core: &Arc<WizardCore>, phase: WizardPhase, error: Option<String>) {
    if let Ok(mut w) = core.state.lock() {
        w.phase = phase;
        w.error = error;
    }
}

fn fail(core: &Arc<WizardCore>, error: &str) {
    supervisor::log(&format!("首次安装失败: {error}"));
    set_phase(core, WizardPhase::Failed, Some(error.to_string()));
}

/// 失败原因 + 网络诊断：追加一次 registry 连通性探测，帮小白定位断网/代理
fn diagnose(mut err: String) -> String {
    let probe = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()
        .and_then(|c| c.get("https://registry.npmjs.org/@deepseek-ai/dsh").send().ok());
    match probe {
        Some(resp) if resp.status().is_success() => err,
        _ => {
            err.push_str("。无法访问 npm registry——请检查网络连接或代理设置后重试");
            err
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端口全空闲时绑定到起点 3177（不真绑，避免与测试环境冲突：只断言顺序正确）
    #[test]
    fn wizard_port_sequence_starts_at_3177() {
        assert_eq!(WIZARD_PORT_START, 3177);
        assert_eq!(WIZARD_PORT_ATTEMPTS, 5);
    }

    /// 绑定回退：先占住 3177，bind 应跳过它并成功绑定后续端口
    #[test]
    fn bind_skips_occupied_port() {
        let blocker = tiny_http::Server::http(("127.0.0.1", WIZARD_PORT_START))
            .expect("占住 3177 失败（测试环境端口被占）");
        let (server, port) = bind_wizard_server().expect("应回退到后续端口");
        assert_ne!(port, WIZARD_PORT_START, "占用的 3177 必须被跳过");
        drop(blocker);
        drop(server);
    }
}
