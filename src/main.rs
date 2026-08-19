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
mod installer;
mod job;
mod notify;
mod runtime;
mod status;
mod supervisor;
mod tray;
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

/// 单实例保护：named mutex 抢锁。已有实例在跑 → false（本实例应退出）。
/// 锁句柄静态持有到进程退出（drop 会释放锁，双开检查失效）。
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

#[cfg(not(target_os = "windows"))]
fn acquire_single_instance() -> bool {
    true
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
        eprintln!("收到 Ctrl+C，清理 dsh 引擎…");
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
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(&p).spawn();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");

    // 目录骨架提前（config edit 等子命令需要）
    if let Err(e) = runtime::ensure_layout() {
        eprintln!("初始化运行时目录失败: {e}");
        std::process::exit(1);
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

    // status：跨进程读守护状态（state.json）
    if sub == "status" {
        if acquired {
            println!("{{\"running\":false,\"message\":\"守护未运行\"}}");
        } else {
            println!("{}", supervisor::read_state_json());
        }
        return;
    }

    // stop：写 control.json，监测线程下一轮停掉 dsh（看门狗继续后台）
    if sub == "stop" {
        if acquired {
            println!("守护未运行，无需停止");
        } else {
            supervisor::request_stop();
            println!("已发送停止请求（守护将在数秒内停止 dsh 引擎，看门狗继续后台）");
        }
        return;
    }

    // config edit：打开配置文件
    if sub == "config" && args.get(2).map(|s| s.as_str()) == Some("edit") {
        open_config_editor();
        return;
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
    eprintln!("DSH 伴侣已启动: {url}  根目录: {}", runtime::root_dir().display());

    // 管理页（状态管理·网页形态）：http://127.0.0.1:<status_port> —— 状态展示 + 安装/启停。
    // 0 = 关闭；bind 失败（端口被占）静默降级，不影响主流程。
    if cfg.status_port != 0 {
        let p = cfg.status_port;
        let cfg2 = cfg.clone();
        std::thread::spawn(move || {
            if let Err(e) = crate::status::serve(p, cfg2) {
                crate::supervisor::log(&format!("管理页启动失败（端口 {p}）: {e}"));
            }
        });
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

    if no_tray {
        eprintln!("[--no-tray] 无头模式，Ctrl+C 退出。");
        headless_loop();
    } else {
        match tray::run_tray(&url) {
            Ok(()) => {}
            Err(e) => {
                // 托盘不可用（无桌面会话/创建事件循环失败）：降级无头模式，守护继续跑
                eprintln!("托盘不可用（{e}），降级无头模式继续守护 dsh");
                supervisor::log(&format!("托盘不可用，降级无头模式: {e}"));
                headless_loop();
            }
        }
    }
    // 托盘事件循环退出（用户点退出）或 Ctrl+C → 清理 dsh 引擎子进程
    supervisor::shutdown();
}

fn print_help() {
    println!(
        "DSH 伴侣 — 双击即用的 DeepSeek Harness\n\
         \n\
         用法: dsh-come [start] [--port <端口>] [--no-tray]\n\
         \n\
         子命令:\n\
           status            查询 dsh 运行状态（JSON）\n\
           stop              停止 dsh 引擎（看门狗继续后台）\n\
           config edit      打开配置文件\n\
           doctor [--mode inspect|treat|attend|emergency]  独立诊断\n\
         \n\
         环境变量:\n\
           DSH_DESKTOP_HOME      数据根目录（默认 %LOCALAPPDATA%\\dsh-desktop）\n\
           DSH_DESKTOP_PORT      引擎端口（默认 3080）\n\
           DSH_DESKTOP_NO_TRAY   1 时等同 --no-tray"
    );
}
