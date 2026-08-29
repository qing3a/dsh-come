//! DSH 伴侣入口：把 DeepSeek Harness 变成双击即用的 Windows 桌面 App。
//! release 构建自动隐藏控制台窗口（windows_subsystem）。
//!
//! 流程：参数解析 → 目录骨架 → 启动系统 dsh 引擎 → 托盘事件循环（或 --no-tray 无头）；
//! 退出时清理 dsh 子进程（Job Object / taskkill 整树）。
//!
//! 子命令（跨进程控制已运行的守护）：
//!   status        读 state.json，输出 dsh 运行状态 JSON
//!   stop          写 control.json，监测线程下一轮停掉 dsh（看门狗继续后台）
//!   config edit   打开配置文件
//!   doctor        独立诊断（不需要守护在跑）

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod doctor;
mod i18n;
mod installer;
mod job;
mod notify;
mod patchyml;
mod runtime;
mod status;
mod supervisor;
mod tray;
mod uninstall;
mod updater;
mod wizard;

use doctor::Mode;

/// 启动系统 dsh 引擎（直启 PATH 里的 dsh；缺失时由管理页/向导负责正常安装）。
/// 引导线程（wizard::start）调用；引擎就绪后由该线程统一打开浏览器。
/// 启动前先跑一次证据驱动自愈；config.doctor_mode 可覆盖调用方传入的模式
/// （缺省 None → 用调用方 mode，即 wizard 的逐级升级；崩溃自愈不受此配置影响）。
pub fn run_first_boot(cfg: &config::AppConfig, mode: Mode) -> Result<(), String> {
    let mode = cfg
        .doctor_mode
        .as_deref()
        .and_then(Mode::from_str)
        .unwrap_or(mode);
    doctor::heal(cfg, mode);
    supervisor::start(cfg)
}

/// 单实例保护：Windows 用 named mutex；Unix 用数据根下的 flock 锁文件。
/// 抢锁失败 → false（本实例应退出）。锁句柄静态持有到进程退出（drop 会释放锁，双开检查失效）。
#[cfg(target_os = "windows")]
fn acquire_single_instance() -> bool {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    static LOCK: OnceLock<OwnedHandle> = OnceLock::new();
    const NAME: &str = "Local\\dsh-come-single-instance";
    let wide: Vec<u16> = NAME.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: CreateMutexW 标准调用，无自定义属性；固定名字
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if h == 0 {
        return true; // 创建失败（权限等）：不阻塞启动，退化无锁
    }
    // SAFETY: h 是刚创建的合法 mutex 句柄，交由 OwnedHandle 托管生命周期
    let handle = unsafe { OwnedHandle::from_raw_handle(h as *mut _) };
    // SAFETY: GetLastError 紧随 CreateMutexW，读本线程最近错误
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let _ = LOCK.set(handle); // 静态持有到进程退出，防 drop 释放锁
    !already_exists
}

/// 单实例保护（Unix）：对 `<root>/dsh-come.lock` 加 flock（非阻塞）。
/// 成功持锁 → true 并静态持有文件到进程退出（close 自动释放）；已被别实例锁住 → false。
#[cfg(not(target_os = "windows"))]
fn acquire_single_instance() -> bool {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;
    use std::sync::OnceLock;

    static LOCK: OnceLock<File> = OnceLock::new();
    let p = runtime::root_dir().join("dsh-come.lock");
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(f) = File::options().create(true).write(true).open(&p) else {
        return true; // 打开失败（只读目录等）：不阻塞启动，退化无锁
    };
    // SAFETY: f 的 fd 有效且保持 open 期间锁有效；LOCK_NB 非阻塞，失败即已有实例
    let held = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 };
    if held {
        let _ = LOCK.set(f); // 静态持有到进程退出，防 close 释放锁
    }
    held
}

/// 无头/托盘降级时的阻塞：等待 Ctrl+C（前台）或进程被看门狗任务复活；守护线程继续工作。
/// 收到 Ctrl+C 后返回，main 在返回前调用 supervisor::shutdown() 清理 dsh。
fn headless_loop() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    // ctrlc::set_handler 进程内只能设置一次；失败（已设置过）则退化为无限睡。
    if ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .is_ok()
    {
        let _ = rx.recv();
        eprintln!(
            "{}",
            crate::i18n::tr("收到 Ctrl+C，清理 dsh 引擎…", "Ctrl+C received, shutting down the dsh engine…")
        );
    } else {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}

/// 打开配置文件：确保文件存在（缺失落默认），再用系统默认程序打开。
fn open_config_editor() {
    let cfg = config::load();
    config::save(&cfg);
    let p = config::config_path();
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", &p.display().to_string()]);
        supervisor::hide_window(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&p).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&p).spawn();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");

    // 旧数据目录迁移（dsh-desktop → dsh-come，审计 P1-5，发布前最后窗口）。
    // 必须在 ensure_layout 之前：后者会在新目录建 logs，导致 new_root.exists() 而跳过迁移。
    runtime::migrate_legacy_dir();

    // 目录骨架提前（config edit 等子命令需要）
    if let Err(e) = runtime::ensure_layout() {
        eprintln!(
            "{}: {e}",
            crate::i18n::tr("初始化运行时目录失败", "Failed to initialize runtime directories")
        );
        std::process::exit(1);
    }

    // 修正 PATH：dsh 0.1.1+ 依赖 Node 22+ API（Promise.withResolvers / stripTypeScriptTypes
    // / createZstdDecompress），部分环境（IDE 沙箱等）会把旧 node 注入 PATH 最前面导致
    // dsh 用低版本启动崩溃。把第一个 >=22 的 node 目录提升到 PATH 最前面，所有子进程继承。
    if runtime::prioritize_compatible_node(22) {
        eprintln!(
            "{}",
            crate::i18n::tr(
                "已修正 PATH：优先使用 Node 22+（dsh 依赖）",
                "PATH adjusted: Node 22+ prioritized (required by dsh)"
            )
        );
    }

    // ---- 单实例 + 子命令分发 ----
    let acquired = acquire_single_instance();

    // doctor：独立诊断，不需要守护在跑
    if sub == "doctor" {
        let mode = args
            .iter()
            .position(|a| a == "--mode")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| Mode::from_str(s))
            .unwrap_or(Mode::Inspect);
        let mut cfg = config::load();
        if let Ok(p) = std::env::var("DSH_DESKTOP_PORT") {
            if let Ok(v) = p.parse() {
                cfg.port = v;
            }
        }
        doctor::run_cli(&cfg, mode);
        return;
    }

    // status：跨进程读守护状态（state.json）。存活判定看 state.json 心跳（mtime，
    // 监测线程每轮重写），不依赖单实例锁——锁创建失败（权限等）会退化无锁，
    // acquired==true 会把「守护其实在跑」误报成「未运行」（审计 P2-4）。
    // 心跳窗口 10s >> 监测轮询间隔（~1s），崩溃后最多 1s 即判死。
    if sub == "status" {
        if supervisor::state_stale_secs().map(|s| s < 10).unwrap_or(false) {
            println!("{}", supervisor::read_state_json());
        } else {
            println!(
                "{{\"running\":false,\"message\":\"{}\"}}",
                crate::i18n::tr("守护未运行", "daemon not running")
            );
        }
        return;
    }

    // stop：写 control.json，监测线程消费后停引擎（看门狗继续后台）。
    // 等待引擎确认停止（轮询 state.json ≤2.5s，覆盖监测线程 1s 轮询间隔），
    // 避免脚本里紧跟 `status` 读到「仍在运行」的旧值（审计 P2-1）。
    if sub == "stop" {
        if acquired {
            println!(
                "{}",
                crate::i18n::tr("守护未运行，无需停止", "daemon not running, nothing to stop")
            );
        } else {
            let (stopped, _) = supervisor::request_stop_and_wait();
            if stopped {
                println!(
                    "{}",
                    crate::i18n::tr(
                        "已停止 dsh 引擎（看门狗继续后台）",
                        "dsh engine stopped (the watchdog keeps running)"
                    )
                );
            } else {
                println!(
                    "{}",
                    crate::i18n::tr(
                        "停止请求已发送，但未能确认引擎停止（守护可能无响应）",
                        "Stop request sent, but engine stop could not be confirmed (daemon may be unresponsive)"
                    )
                );
            }
        }
        return;
    }

    // config edit：打开配置文件
    if sub == "config" && args.get(2).map(|s| s.as_str()) == Some("edit") {
        open_config_editor();
        return;
    }

    // update：检查更新（force，无视每日节流；不下载不安装，安装走托盘「更新到 vX」）
    if sub == "update" {
        match updater::check(true) {
            Ok(Some(info)) => println!(
                "{}",
                serde_json::json!({
                    "current": updater::current_version(),
                    "latest": info.version,
                    "available": true,
                })
            ),
            Ok(None) => println!(
                "{}",
                serde_json::json!({
                    "current": updater::current_version(),
                    "latest": null,
                    "available": false,
                })
            ),
            Err(e) => {
                println!(
                    "{}",
                    serde_json::json!({ "current": updater::current_version(), "error": e })
                );
                std::process::exit(1);
            }
        }
        return;
    }

    // dsh-uninstall：纯净卸载系统 dsh（不卸载壳自身）。
    // 默认保数据（keep_data=true）——.dsh 里是凭据/配置/工作台数据，不明确要求不清。
    // --clean-shim 才清 PATH 残留 shim（危险项，默认关）。
    if sub == "dsh-uninstall" {
        let mut keep_data = true;
        let mut clean_shim = false;
        for a in args.iter().skip(2) {
            match a.as_str() {
                "--keep-data" | "--keep-data=true" => keep_data = true,
                "--keep-data=false" | "--no-keep-data" => keep_data = false,
                "--clean-shim" => clean_shim = true,
                _ => {}
            }
        }
        let report = uninstall::run_uninstall(keep_data, clean_shim);
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.msg.clone())
        );
        if !report.ok {
            std::process::exit(1);
        }
        return;
    }

    // 改名迁移后，Unix 新旧锁文件路径不同：旧版守护若仍在旧目录跑，新路径的 flock
    // 挡不住本实例 → 必须显式检查旧锁并退出（否则双守护并存）。Windows 由进程级
    // named mutex 天然防双开（与路径无关），无需此检查。
    #[cfg(unix)]
    if runtime::legacy_daemon_running() {
        eprintln!(
            "{}",
            crate::i18n::tr(
                "检测到旧版 dsh-come 仍在运行（旧数据目录），请先退出旧版再启动",
                "An older dsh-come instance is still running (legacy data dir); quit it first"
            )
        );
        std::process::exit(1);
    }

    if !acquired {
        // 已有守护在跑，且不是上述控制命令 → 双开，静默退出（release 无控制台无需提示）
        return;
    }

    // ---- 以下是守护启动流程 ----
    // 作业对象：让 dsh-come 退出/崩溃时 OS 强杀整棵 dsh 进程树（KILL_ON_JOB_CLOSE），
    // 消除「守护进程崩→dsh 变孤儿占端口」。失败仅日志降级（仍可用 taskkill /T）。
    #[cfg(target_os = "windows")]
    if crate::job::ensure_job().is_none() {
        eprintln!("⚠️ 无法创建 Job Object（崩溃兜底降级为 taskkill /T）");
    }

    let mut port: u16 = std::env::var("DSH_DESKTOP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3080);
    let mut no_tray = std::env::var("DSH_DESKTOP_NO_TRAY").is_ok_and(|v| v == "1");
    let mut args_iter = args.iter().skip(1).peekable();
    while let Some(a) = args_iter.next() {
        match a.as_str() {
            "--port" => {
                if let Some(v) = args_iter.next() {
                    if let Ok(p) = v.parse() {
                        port = p;
                    }
                }
            }
            "--no-tray" => no_tray = true,
            "-h" | "--help" => {
                print_help();
                return;
            }
            // start 子命令：落到这里即正常启动（已在跑会被上方 acquired 拦下）
            "start" => {}
            _ => {}
        }
    }

    let mut cfg = config::load();
    cfg.port = port;
    config::save(&cfg);

    let url = format!("http://127.0.0.1:{}", cfg.port);
    eprintln!(
        "{}: {url}   {}: {}",
        crate::i18n::tr("DSH 伴侣已启动", "DSH Companion started"),
        crate::i18n::tr("根目录", "root dir"),
        runtime::root_dir().display()
    );

    // 管理页（状态管理·网页形态）：http://127.0.0.1:<status_port> —— 状态展示 + 安装/启停。
    // 0 = 关闭。固定端口被占时自动回退随机端口（防与其他应用冲突导致管理页不可用），
    // 实际端口写入 status::admin_port()，托盘菜单/向导动态读取；bind 双双失败才静默降级。
    if cfg.status_port != 0 {
        let expect = cfg.status_port;
        match crate::status::bind_any(expect) {
            Ok((listener, actual)) => {
                crate::status::set_admin_port(Some(actual));
                if actual != expect {
                    crate::supervisor::log(&format!(
                        "管理页端口 {expect} 被占用，已回退到随机端口 {actual}"
                    ));
                }
                let cfg2 = cfg.clone();
                std::thread::spawn(move || crate::status::serve_listener(listener, cfg2));
            }
            Err(e) => {
                crate::supervisor::log(&format!("管理页启动失败（端口 {expect}）: {e}"));
            }
        }
        // 探测预热：后台先跑一次环境探测填充缓存（管理页首屏不卡）
        std::thread::spawn(|| {
            let _ = crate::installer::probe();
        });
    }

    // 启动向导：后台启动引擎 + 就绪后打开浏览器
    {
        let cfg = cfg.clone();
        std::thread::spawn(move || wizard::start(&cfg));
    }

    // 静默检查更新（每日最多一次）：有新版本 → 托盘菜单出现「更新到 vX」+ 桌面通知。
    // 失败静默（只记日志，更新是锦上添花，不影响守护）。
    std::thread::spawn(|| match updater::check(false) {
        Ok(Some(info)) => {
            supervisor::log(&format!(
                "发现新版本 v{}（当前 v{}），托盘菜单可更新",
                info.version,
                updater::current_version()
            ));
            crate::notify::toast(
                crate::i18n::tr("DSH 伴侣", "DSH Companion"),
                &format!(
                    "{} v{}（{} v{}）",
                    crate::i18n::tr("发现新版本", "New version available"),
                    info.version,
                    crate::i18n::tr("当前", "current"),
                    updater::current_version()
                ),
            );
        }
        Ok(None) => {}
        Err(e) => supervisor::log(&format!("检查更新失败（静默）: {e}")),
    });

    if no_tray {
        eprintln!(
            "{}",
            crate::i18n::tr("[--no-tray] 无头模式，Ctrl+C 退出。", "[--no-tray] headless mode, Ctrl+C to exit.")
        );
        headless_loop();
    } else {
        match tray::run_tray(&url) {
            Ok(()) => {}
            Err(e) => {
                // 托盘不可用（无桌面会话/创建事件循环失败）：降级无头模式，守护继续跑
                eprintln!(
                    "{}（{e}），{}",
                    crate::i18n::tr("托盘不可用", "Tray unavailable"),
                    crate::i18n::tr(
                        "降级无头模式继续守护 dsh",
                        "falling back to headless mode, dsh stays guarded"
                    )
                );
                supervisor::log(&format!("托盘不可用，降级无头模式: {e}"));
                headless_loop();
            }
        }
    }
    // 托盘事件循环退出（用户点退出）或 Ctrl+C → 清理 dsh 引擎子进程
    supervisor::shutdown();
}

fn print_help() {
    let name = crate::i18n::tr("DSH 伴侣", "DSH Companion");
    let tagline = crate::i18n::tr("双击即用的 DeepSeek Harness", "one-click DeepSeek Harness desktop app");
    let usage = crate::i18n::tr("用法", "Usage");
    let sub = crate::i18n::tr("子命令", "Subcommands");
    let env = crate::i18n::tr("环境变量", "Environment variables");
    println!(
        "{name} — {tagline}\n\
         \n\
         {usage}: dsh-come [start] [--port <port>] [--no-tray]\n\
         \n\
         {sub}:\n\
         \x20  status            {st}\n\
         \x20  stop              {sp}\n\
         \x20  config edit      {ce}\n\
         \x20  doctor [--mode inspect|treat|attend|emergency]  {dc}\n\
         \x20  update            {up}\n\
         \x20  dsh-uninstall [--keep-data=false] [--clean-shim]  {un}\n\
         \n\
         {env}:\n\
         \x20  DSH_COME_HOME         {e0}\n\
         \x20  DSH_DESKTOP_HOME      {e1}（{legacy}）\n\
         \x20  DSH_DESKTOP_PORT      {e2}\n\
         \x20  DSH_DESKTOP_NO_TRAY   {e3}",
        st = crate::i18n::tr("查询 dsh 运行状态（JSON）", "query dsh status (JSON)"),
        sp = crate::i18n::tr("停止 dsh 引擎（看门狗继续后台）", "stop the dsh engine (watchdog keeps running)"),
        ce = crate::i18n::tr("打开配置文件", "open config file"),
        dc = crate::i18n::tr("独立诊断", "standalone diagnostics"),
        up = crate::i18n::tr("检查更新（输出 JSON）", "check for updates (prints JSON)"),
        un = crate::i18n::tr(
            "纯净卸载系统 dsh（默认保数据、不删 shim）",
            "cleanly uninstall system dsh (keeps data and shim by default)"
        ),
        e0 = crate::i18n::tr("数据根目录（默认 %LOCALAPPDATA%\\dsh-come）", "data root (default %LOCALAPPDATA%\\dsh-come)"),
        e1 = crate::i18n::tr("数据根目录（旧名，兼容）", "data root (legacy name, kept for compatibility)"),
        legacy = crate::i18n::tr("旧名，兼容", "legacy, compatible"),
        e2 = crate::i18n::tr("引擎端口（默认 3080）", "engine port (default 3080)"),
        e3 = crate::i18n::tr("1 时等同 --no-tray", "equivalent to --no-tray when 1"),
    );
}
