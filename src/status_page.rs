//! 壳管理页：`http://127.0.0.1:3177/desktop` 独立窗口，可视化版本/插件/日志。
//!
//! 复刻首次向导的本地服务模式（wizard.rs）：同一 3177 端口、同一浏览器窗口载体，
//! 不新增监听端口。状态 JSON 拼装（status_json）用 `pub(crate)` 供 wizard.rs 复用，
//! 保证「引擎状态」在页面层只有一份来源。

use crate::runtime;
use crate::supervisor;
use std::sync::OnceLock;
use std::time::Duration;

const STATUS_HTML: &str = include_str!("../assets/status.html");

static PORT: OnceLock<u16> = OnceLock::new();

/// 记录向导本地服务端口（由 wizard::start 启动服务后调用）。
/// 托盘菜单「运行状态」在服务不可达时（已装环境无向导、端口被占）由浏览器直接访问引擎端口。
pub fn set_port(port: u16) {
    let _ = PORT.set(port);
}

fn serve_port() -> Option<u16> {
    PORT.get().copied()
}

/// 打开壳管理页：优先向导本地服务端口；无（已装环境未起向导）→ 浏览器直接访问引擎端口。
/// 两者都打开 dsh 壳/引擎的 Web UI，托盘无重定向依赖。
pub fn open() {
    let url = match serve_port() {
        Some(p) => format!("http://127.0.0.1:{p}/desktop"),
        None => format!("http://127.0.0.1:{}/desktop", runtime_engine_port()),
    };
    crate::tray::open_browser(&url);
}

fn runtime_engine_port() -> u16 {
    crate::config::load().port
}

/// 壳管理页状态 JSON（页面轮询）：引擎状态 + 版本管理 + 插件清单 + 工作台
/// 复用 supervisor 的 flash/stage/version + plugins::installed_plugins，单一事实来源。
pub(crate) fn status_json() -> String {
    let st = supervisor::status();
    let state = runtime::load_state();
    let installed = crate::plugins::installed_plugins();
    let catalog = crate::plugins::market_catalog();
    let workbenches: Vec<_> = catalog
        .iter()
        .filter(|p| p.is_workbench())
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "version": p.version,
                "scenario": p.scenario,
                "entry": p.entry,
                "requires": p.requires,
                "verify_evidence": p.verify_evidence,
                "present": p.entry.as_deref().map_or(false, crate::tray::local_asset_present),
            })
        })
        .collect();
    serde_json::json!({
        "engine": {
            "running": st.running,
            "ready": st.ready,
            "port": st.port,
            "pid": st.pid,
            "stage": st.stage,
            "flash": supervisor::flash(),
            "restarts": st.restarts,
            "last_error": st.last_error,
        },
        "version": {
            "current": state.current,
            "previous": state.previous,
            "pending": state.pending,
            "known_bad": state.known_bad,
        },
        "plugins": installed,
        "market_installed": crate::plugins::market_installed(),
        "workbenches": workbenches,
        "dirs": {
            "root": runtime::root_dir().display().to_string(),
            "home": runtime::home_dir().display().to_string(),
            "logs": runtime::logs_dir().display().to_string(),
        },
        "engine_url": format!("http://127.0.0.1:{}", st.port),
        "logs": log_tail(40),
    })
    .to_string()
}

/// 壳管理页路由处理（wizard.rs 的 HTTP 服务里按 path 分发到本函数）
pub(crate) fn serve(path: &str) -> Option<(u16, &'static str, String)> {
    match path {
        "/desktop" => Some((
            200,
            "text/html; charset=utf-8",
            STATUS_HTML.to_string(),
        )),
        "/desktop/api/status" => Some((
            200,
            "application/json",
            status_json(),
        )),
        _ => None,
    }
}

// ---------- 日志尾部 ----------

/// engine.log 尾部最近 max_lines 行（页面展示；读失败返回空）
pub(crate) fn log_tail(max_lines: usize) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(runtime::engine_log()) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = s.lines().map(|l| l.to_string()).collect();
    let start = lines.len().saturating_sub(max_lines);
    lines.split_off(start)
}

/// 状态轮询间隔（页面端 JS 也写死 1500ms；此常量仅用于注释一致性）
#[allow(dead_code)]
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
