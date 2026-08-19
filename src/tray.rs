//! 系统托盘（tray-icon + winit 事件循环，主线程）。
//! 精简版：状态行 / 打开界面 / 重启引擎 / 关闭引擎 / 退出。

use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use winit::application::ApplicationHandler;
use winit::event::StartCause;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};

use crate::config;
use crate::runtime;
use crate::supervisor;

enum UserEvent {
    Tray,
    Menu(tray_icon::menu::MenuEvent),
    Refresh,
}

struct TrayIds {
    open: MenuId,
    restart: MenuId,
    stop: MenuId,
    logs: MenuId,
    quit: MenuId,
}

struct App {
    url: String,
    ids: TrayIds,
    menu: Option<Menu>,
    tray: Option<tray_icon::TrayIcon>,
    proxy: EventLoopProxy<UserEvent>,
    auto_opened: bool,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _el: &ActiveEventLoop,
        _id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {}

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::Init) {
            if let Some(menu) = self.menu.take() {
                self.tray = build_tray(menu);
            }
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(ev) => {
                if ev.id == self.ids.quit {
                    supervisor::shutdown();
                    std::process::exit(0);
                } else if ev.id == self.ids.open {
                    open_browser(&self.url);
                } else if ev.id == self.ids.logs {
                    open_dir(&runtime::logs_dir());
                } else if ev.id == self.ids.restart {
                    let cfg = config::load();
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        match supervisor::restart(&cfg) {
                            Ok(()) => {
                                supervisor::log("引擎已重启");
                                supervisor::set_flash("引擎已重启");
                            }
                            Err(e) => {
                                supervisor::log(&format!("引擎重启失败: {e}"));
                                supervisor::set_flash(&format!("引擎重启失败: {e}"));
                            }
                        }
                        let _ = proxy.send_event(UserEvent::Refresh);
                    });
                } else if ev.id == self.ids.stop {
                    // 关闭引擎省内存：auto_restart=false，监测线程不再自动拉起；
                    // 看门狗继续后台，要用时点「重启引擎」恢复。
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        supervisor::set_flash("正在关闭引擎…");
                        match supervisor::stop() {
                            Ok(()) => {
                                supervisor::log("引擎已关闭（省内存模式，可随时重启）");
                                supervisor::set_flash("引擎已关闭，内存已释放");
                            }
                            Err(e) => {
                                supervisor::log(&format!("关闭引擎失败: {e}"));
                                supervisor::set_flash(&format!("关闭引擎失败: {e}"));
                            }
                        }
                        let _ = proxy.send_event(UserEvent::Refresh);
                    });
                }
            }
            UserEvent::Tray => {}
            UserEvent::Refresh => self.rebuild(),
        }
    }
}

impl App {
    fn rebuild(&mut self) {
        if let Some(tray) = &self.tray {
            let (menu, ids) = build_menu();
            let _ = tray.set_menu(Some(Box::new(menu)));
            self.ids = ids;
        }
        let st = supervisor::status();
        if st.ready && !self.auto_opened && !crate::wizard::handed_off() {
            self.auto_opened = true;
            open_browser(&self.url);
        }
    }
}

/// 运行托盘事件循环。返回：
/// - `Ok(())`：托盘正常退出（用户点「退出」）
/// - `Err(_)`：托盘不可用（典型：无桌面会话 / 创建事件循环失败）→ 调用方降级无头模式
pub fn run_tray(url: &str) -> Result<(), String> {
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            return Err(format!("创建托盘事件循环失败: {e}"));
        }
    };
    let proxy = event_loop.create_proxy();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |_e| {
        let _ = proxy.send_event(UserEvent::Tray);
    }));
    let proxy = event_loop.create_proxy();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));
    let app_proxy = event_loop.create_proxy();

    let (menu, ids) = build_menu();
    let mut app = App {
        url: url.to_string(),
        ids,
        menu: Some(menu),
        tray: None,
        proxy: app_proxy,
        auto_opened: false,
    };

    // 3s 定时重建（刷新状态行/菜单可用性——用户反馈状态更新太慢，15s→3s）
    {
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let _ = proxy.send_event(UserEvent::Refresh);
        });
    }

    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("托盘事件循环错误: {e}");
    }
    Ok(())
}

fn build_menu() -> (Menu, TrayIds) {
    let st = supervisor::status();
    let status_text = status_line(&st);
    let status_item = MenuItem::new(&status_text, false, None);
    let open_item = MenuItem::new("打开界面", st.ready, None);
    let restart_item = MenuItem::new("重启引擎", true, None);
    // 关闭引擎（省内存）：不区分是否本壳启动（2026-08-19 用户要求），运行中即可点
    let stop_item = MenuItem::new("关闭引擎", st.running, None);
    let logs_item = MenuItem::new("打开日志目录", true, None);
    let quit_item = MenuItem::new("退出", true, None);

    let menu = Menu::new();
    // 「打开界面」置顶（最常用）
    let _ = menu.append(&open_item);
    let _ = menu.append(&status_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&restart_item);
    let _ = menu.append(&stop_item);
    let _ = menu.append(&logs_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let ids = TrayIds {
        open: open_item.id().clone(),
        restart: restart_item.id().clone(),
        stop: stop_item.id().clone(),
        logs: logs_item.id().clone(),
        quit: quit_item.id().clone(),
    };
    (menu, ids)
}

fn status_line(st: &supervisor::SuperStatus) -> String {
    let ver = st.version.clone().unwrap_or_else(|| "系统 dsh".to_string());
    let body = if !st.stage.is_empty() {
        let mut s = st.stage.clone();
        if let Some(secs) = st.stage_elapsed {
            if secs >= 30 {
                s.push_str(&format!("（已 {}）", crate::supervisor::fmt_elapsed(secs)));
            }
        }
        s
    } else if let Some(f) = supervisor::flash() {
        f
    } else if st.running {
        if st.ready {
            format!("运行中 ✓ http://127.0.0.1:{}", st.port)
        } else {
            "启动中…".to_string()
        }
    } else if let Some(e) = &st.last_error {
        format!("已停止 ✗（{e}）")
    } else {
        "已停止 ✗".to_string()
    };
    format!("DSH 伴侣｜{ver}｜{body}")
}

fn build_tray(menu: Menu) -> Option<tray_icon::TrayIcon> {
    let icon = gen_icon();
    match tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("DSH 伴侣")
        .with_icon(icon)
        .build()
    {
        Ok(t) => Some(t),
        Err(e) => {
            supervisor::log(&format!("创建托盘图标失败: {e}"));
            None
        }
    }
}

/// 32x32 RGBA 图标：深蓝圆角方块 + 白色中心点 + 青色光环
fn gen_icon() -> tray_icon::Icon {
    let (w, h) = (32u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let blue = [34u8, 74u8, 160u8, 255u8];
    let white = [238u8, 242u8, 255u8, 255u8];
    let cyan = [64u8, 192u8, 190u8, 255u8];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let body = x >= 6 && x <= 27 && y >= 6 && y <= 27;
            let corner = (x <= 9 && y <= 9) || (x >= 24 && y <= 9) || (x <= 9 && y >= 24) || (x >= 24 && y >= 24);
            if body && !corner {
                rgba[i..i + 4].copy_from_slice(&blue);
            } else {
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            if body && !corner {
                let dx = x as i32 - 16;
                let dy = y as i32 - 16;
                if dx * dx + dy * dy <= 16 {
                    rgba[i..i + 4].copy_from_slice(&white);
                }
                if dx * dx + dy * dy > 45 && dx * dx + dy * dy <= 64 {
                    rgba[i..i + 4].copy_from_slice(&cyan);
                }
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, w, h).expect("生成图标失败")
}

pub fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        crate::supervisor::hide_window(&mut cmd); // 防弹 cmd 黑框
        let _ = cmd.spawn();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

pub fn open_dir(dir: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", &dir.display().to_string()]);
        crate::supervisor::hide_window(&mut cmd); // 防弹 cmd 黑框
        let _ = cmd.spawn();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}
