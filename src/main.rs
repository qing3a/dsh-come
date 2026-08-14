//! DSH 桌面版入口：把 DeepSeek Harness 变成双击即用的 Windows 桌面 App。
//! release 构建自动隐藏控制台窗口（windows_subsystem）。
//!
//! 流程：参数解析 → 目录骨架 → 后台首次引导（自动安装 latest + 启动引擎）→ 托盘事件循环；
//! 托盘退出后清理 dsh 子进程（杀整棵树，防残留 Node 占端口）。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod plugins;
mod runtime;
mod supervisor;
mod tray;
mod updater;

fn main() {
    let mut port: u16 = std::env::var("DSH_DESKTOP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3080);
    let mut no_tray = std::env::var("DSH_DESKTOP_NO_TRAY").is_ok_and(|v| v == "1");

    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => {
                if let Some(v) = args.next() {
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
            _ => {}
        }
    }

    if let Err(e) = runtime::ensure_layout() {
        eprintln!("初始化运行时目录失败: {e}");
        std::process::exit(1);
    }

    let cfg = config::load();
    // CLI --port 覆盖 config.json（config 以 CLI 为准）
    let mut cfg = cfg;
    cfg.port = port;
    config::save(&cfg);

    let url = format!("http://127.0.0.1:{}", cfg.port);
    eprintln!("DSH 桌面版已启动: {url}  根目录: {}", runtime::root_dir().display());

    // 后台首次引导：自举安装 Node → 无已装版本则自动装 latest 并启动引擎（托盘状态行反映进度）
    {
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            if let Err(e) = runtime::ensure_node() {
                eprintln!("Node 自举安装失败: {e}");
                return;
            }
            if !updater::bootstrap(&cfg) {
                eprintln!("首次引导失败（详见 {}）", runtime::engine_log().display());
            }
        });
    }

    if no_tray {
        println!("[--no-tray] 开发模式，Ctrl+C 退出。");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    tray::run_tray(&url);
    // 托盘事件循环退出 → 清理 dsh 引擎子进程
    supervisor::shutdown();
}

fn print_help() {
    println!(
        "DSH 桌面版 — 双击即用的 DeepSeek Harness\n\
         \n\
         用法: dsh-desktop [--port <端口>] [--no-tray]\n\
         \n\
         环境变量:\n\
           DSH_DESKTOP_HOME   数据根目录（默认 %LOCALAPPDATA%\\dsh-desktop）\n\
           DSH_DESKTOP_PORT   引擎端口（默认 3080）\n\
           DSH_DESKTOP_NO_TRAY  1 时等同 --no-tray"
    );
}
