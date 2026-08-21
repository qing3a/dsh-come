//! 引擎引导：后台启动引擎 → 轮询就绪 → 打开浏览器（自动打开的唯一路径）。
//! tray 侧通过 handed_off() 得知引导线程是否已打开过页面，避免双开；
//! 兜底：引导线程超时未打开（引擎很久才就绪）时，托盘仍会自动打开一次。

use crate::config::AppConfig;
use crate::doctor::Mode;
use crate::supervisor;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static HANDOFF: OnceLock<Mutex<bool>> = OnceLock::new();

/// 等待安装任务完成（轮询 installer 状态）。返回是否成功安装。
fn wait_install(timeout_secs: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let st = crate::installer::install_state();
        if !st.running {
            return st.ok.unwrap_or(false);
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
}

/// 引导线程是否已打开过浏览器（托盘读取：true 则不再自动打开，防双开）。
pub fn handed_off() -> bool {
    HANDOFF
        .get()
        .map(|m| m.lock().map(|g| *g).unwrap_or(false))
        .unwrap_or(false)
}

/// 起引导线程：后台启动引擎，就绪后打开浏览器（只开一次）。
/// 每次重试逐级升级自检模式（处置→主治→急救），把「先检测→推荐执行→兜底急救」落到重试里。
/// 停止条件：运行器缺失（dsh 未安装，诊疗不可自愈）或尝试 3 次仍失败。
pub fn start(cfg: &AppConfig) {
    let _ = HANDOFF.get_or_init(|| Mutex::new(false));

    let exec_cfg = cfg.clone();
    std::thread::spawn(move || {
        let mut attempt: u32 = 0;
        loop {
            // 运行器缺失 → 自动走「正常安装」（2026-08-19 用户拍板，不做 npx 临时拉取）：
            // node 缺失 → winget 装 LTS；dsh 缺失 → npm install -g；装完 fall-through 重试启动。
            if crate::runtime::dsh_runner().is_none() {
                let need_node = !crate::installer::npm_installed();
                if need_node {
                    supervisor::log("未检测到 Node.js/npm，自动安装 Node.js（winget，可能弹出权限确认）…");
                    supervisor::set_flash("正在安装 Node.js…");
                    crate::notify::toast("DSH 伴侣", "未检测到 Node.js，正在自动安装（可能弹出权限确认）…");
                    if let Err(e) = crate::installer::start_install("node") {
                        supervisor::log(&format!("自动安装 Node.js 触发失败: {e}"));
                    } else if !wait_install(480) {
                        supervisor::log("Node.js 安装超时或失败，请到管理页重试");
                        supervisor::set_flash("Node.js 安装失败：请在管理页重试");
                        crate::notify::toast("DSH 伴侣", "Node.js 安装失败，请打开管理页查看原因重试。");
                        return;
                    }
                }
                if !crate::installer::dsh_installed() {
                    supervisor::log("未检测到 dsh，自动安装（npm install -g @deepseek-ai/dsh）…");
                    supervisor::set_flash("正在安装 dsh…");
                    crate::notify::toast("DSH 伴侣", "未检测到 dsh，正在自动安装…");
                    if let Err(e) = crate::installer::start_install("dsh") {
                        supervisor::log(&format!("自动安装 dsh 触发失败: {e}"));
                    } else if !wait_install(480) {
                        supervisor::log("dsh 安装超时或失败，请到管理页重试");
                        supervisor::set_flash("dsh 安装失败：请在管理页重试");
                        crate::notify::toast("DSH 伴侣", "dsh 安装失败，请打开管理页查看原因重试。");
                        return;
                    }
                }
                // 安装完成：fall-through 走下面的 start 重试（attempt 上限仍兜底防死循环）
            }
            if attempt >= 3 {
                supervisor::log("引擎多次尝试后仍无法启动，已停止自动重试（托盘菜单可手动重启）");
                return;
            }
            // 升级阶梯与 monitor 崩溃自愈共用同一函数（1→处置/2→主治/≥3→急救）
            let mode = Mode::for_restart(attempt + 1);
            match crate::run_first_boot(&exec_cfg, mode) {
                Ok(()) => {
                    // 引擎已 spawn，等就绪
                    let deadline = std::time::Instant::now() + Duration::from_secs(exec_cfg.startup_timeout_secs);
                    let mut became_ready = false;
                    loop {
                        if supervisor::status().ready {
                            became_ready = true;
                            break;
                        }
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    if became_ready {
                        // 打开 dsh 引擎本体界面（默认端口 3080）
                        let dsh_url = format!("http://127.0.0.1:{}", exec_cfg.port);
                        crate::tray::open_browser(&dsh_url);
                        // 同时打开 dsh-come 管理页（状态/安装/插件/版本，默认 3081）
                        if exec_cfg.status_port != 0 {
                            let admin_url =
                                format!("http://127.0.0.1:{}", exec_cfg.status_port);
                            crate::tray::open_browser(&admin_url);
                        }
                        supervisor::log("引擎就绪，已打开 dsh 界面与管理页");
                        if let Some(m) = HANDOFF.get() {
                            if let Ok(mut g) = m.lock() {
                                *g = true;
                            }
                        }
                        return;
                    }
                    // spawn 成功但超时未就绪（端口冲突 / 卡在启动）——同样算一次失败：
                    // 先停掉卡住的引擎（否则下次 start() 幂等返回同一个进程，永远等不到升级），
                    // 升级模式重试。
                    let _ = supervisor::stop();
                    supervisor::log(&format!(
                        "引擎在 {} 秒内未就绪（尝试 {}/3），已停止本次并升级诊疗模式重试",
                        exec_cfg.startup_timeout_secs,
                        attempt + 1
                    ));
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(3));
                }
                Err(e) => {
                    supervisor::log(&format!("引擎启动失败: {e}"));
                    // 失败后等待 10s 重试（下次尝试自愈模式升级一级）
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(10));
                }
            }
        }
    });
}
