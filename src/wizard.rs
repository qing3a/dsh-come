//! 引擎引导：后台启动引擎 → 轮询就绪 → 打开浏览器（自动打开的唯一路径）。
//! 浏览器只在引导线程就绪时打开一次（became_ready 分支 return 保证）；
//! 早期设计的「托盘侧 handed_off 防双开」已随托盘不再自动打开页面而移除。

use crate::config::AppConfig;
use crate::doctor::Mode;
use crate::supervisor;
use std::time::Duration;

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

/// 自动安装的组件（node / dsh 共用一套引导骨架，仅文案不同）。
#[derive(Clone, Copy)]
enum Kind {
    Node,
    Dsh,
}

/// 自动安装引导（node/dsh 共用骨架，此前两块复制粘贴）：提示三件套（日志/托盘/通知）
/// → 触发安装 → 等待 → 失败三件套。返回 false = 安装失败，调用方应终止引导；
/// 返回 true = 安装完成，或触发失败（按原逻辑放行，让下一步自检继续判断）。
fn ensure_installed(kind: Kind) -> bool {
    let (installer_kind, start_log, start_flash, start_toast, fail_log, fail_flash, fail_toast) =
        match kind {
            Kind::Node => (
                "node",
                "未检测到 Node.js/npm，自动安装 Node.js（winget，可能弹出权限确认）…",
                crate::i18n::tr("正在安装 Node.js…", "Installing Node.js…"),
                crate::i18n::tr(
                    "未检测到 Node.js，正在自动安装（可能弹出权限确认）…",
                    "Node.js not found; installing automatically (a permission prompt may appear)…",
                ),
                "Node.js 安装超时或失败，请到管理页重试",
                crate::i18n::tr(
                    "Node.js 安装失败：请在管理页重试",
                    "Node.js install failed: retry from the admin page",
                ),
                crate::i18n::tr(
                    "Node.js 安装失败，请打开管理页查看原因重试。",
                    "Node.js install failed; open the admin page to retry.",
                ),
            ),
            Kind::Dsh => (
                "dsh",
                "未检测到 dsh，自动安装（npm install -g @deepseek-ai/dsh）…",
                crate::i18n::tr("正在安装 dsh…", "Installing dsh…"),
                crate::i18n::tr(
                    "未检测到 dsh，正在自动安装…",
                    "dsh not found; installing automatically…",
                ),
                "dsh 安装超时或失败，请到管理页重试",
                crate::i18n::tr(
                    "dsh 安装失败：请在管理页重试",
                    "dsh install failed: retry from the admin page",
                ),
                crate::i18n::tr(
                    "dsh 安装失败，请打开管理页查看原因重试。",
                    "dsh install failed; open the admin page to retry.",
                ),
            ),
        };
    supervisor::log(start_log);
    supervisor::set_flash(start_flash);
    crate::notify::toast(crate::i18n::tr("DSH 伴侣", "DSH Companion"), start_toast);
    if let Err(e) = crate::installer::start_install(installer_kind) {
        supervisor::log(&format!("自动安装触发失败: {e}"));
        return true; // 触发失败不终止：原逻辑会继续走下一步自检
    }
    if !wait_install(480) {
        supervisor::log(fail_log);
        supervisor::set_flash(fail_flash);
        crate::notify::toast(crate::i18n::tr("DSH 伴侣", "DSH Companion"), fail_toast);
        return false;
    }
    true
}

/// 起引导线程：后台启动引擎，就绪后打开浏览器（只开一次）。
/// 每次重试逐级升级自检模式（处置→主治→急救），把「先检测→推荐执行→兜底急救」落到重试里。
/// 停止条件：运行器缺失（dsh 未安装，诊疗不可自愈）或尝试 3 次仍失败。
pub fn start(cfg: &AppConfig) {
    let exec_cfg = cfg.clone();
    std::thread::spawn(move || {
        let mut attempt: u32 = 0;
        loop {
            // 运行器缺失 → 自动走「正常安装」（2026-08-19 用户拍板，不做 npx 临时拉取）：
            // node 缺失 → winget 装 LTS；dsh 缺失 → npm install -g；装完 fall-through 重试启动。
            if crate::runtime::dsh_runner().is_none() {
                if !crate::installer::npm_installed() && !ensure_installed(Kind::Node) {
                    return;
                }
                if !crate::installer::dsh_installed() && !ensure_installed(Kind::Dsh) {
                    return;
                }
                // v1.4.0 P0-2：HLP 插件（LocalApp 生态）缺失时自动安装（node → dsh → hlp 第三步）。
                // 无随附插件源时报错并提示管理页——不阻塞引擎启动（HLP 缺席只影响 LocalApp）。
                if crate::hlp_plugin::detect_version().is_none() {
                    supervisor::log("未检测到 HLP 插件，自动安装（LocalApp 生态）…");
                    match crate::installer::spawn_task("hlp-plugin", || {
                        match crate::hlp_plugin::install() {
                            Ok(m) => (true, m),
                            Err(e) => (false, e),
                        }
                    }) {
                        Err(e) => {
                            supervisor::log(&format!(
                                "HLP 插件安装无法启动：{e}（管理页可手动安装）"
                            ));
                            crate::notify::toast(
                                crate::i18n::tr("DSH 伴侣", "DSH Companion"),
                                crate::i18n::tr(
                                    "HLP 插件安装失败，请打开管理页查看。",
                                    "HLP plugin install failed; open the admin page.",
                                ),
                            );
                        }
                        Ok(()) => {
                            if !wait_install(600) {
                                supervisor::log("HLP 插件安装超时/失败，请到管理页重试");
                            } else {
                                supervisor::log("HLP 插件安装完成");
                            }
                        }
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
                    let deadline = std::time::Instant::now()
                        + Duration::from_secs(exec_cfg.startup_timeout_secs);
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
                        // 打开 dsh 引擎本体界面：用 engine.log 里的带 token URL
                        // （dsh 0.1.2-rc.1 起 web 默认本地鉴权，裸 URL 会 401）。
                        // HTTP 验证等待（最多 8s）：新引擎刚就绪时 stdout 可能还没
                        // flush，日志里还是旧引擎的过期 token → 直接读会 401「打不开」
                        let dsh_url =
                            supervisor::ui_url_ready(Duration::from_secs(8));
                        supervisor::note_opened_url(&dsh_url);
                        crate::tray::open_browser(&dsh_url);
                        // 同时打开 dsh-come 管理页：动态读实际端口（固定端口被占时可能回退随机端口）
                        if let Some(p) = crate::status::admin_port() {
                            let admin_url = format!("http://127.0.0.1:{p}");
                            crate::tray::open_browser(&admin_url);
                        }
                        supervisor::log("引擎就绪，已打开 dsh 界面与管理页");
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
