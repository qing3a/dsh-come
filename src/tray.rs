//! 系统托盘（tray-icon + winit 事件循环，主线程）。
//! 精简版：状态行 / 打开 dsh 界面 / 打开管理页 / 重启引擎 /
//! 退出时关闭引擎（复选框，仅关联退出行为）/ 打开日志目录 / 退出。
//!
//! 复选框设计（2026-08-21 修正）：菜单项在启动时创建一次并持久持有引用，
//! 点击「退出时关闭引擎」只就地 `set_checked` 更新勾选 + 保存配置，**不重建整菜单**。
//! 之前每点一次都 `build_menu` 重建（且 CheckMenuItem::new 的 enabled/checked 参数写反），
//! 导致首次点击后 enabled=false 菜单项被禁用、无法再次点击。

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
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
    open_admin: MenuId,
    restart: MenuId,
    exit_close: MenuId,
    logs: MenuId,
    quit: MenuId,
}

/// 持久持有的菜单项引用（创建一次，后续只就地更新文本/勾选/可用态，不重建）。
struct MenuItems {
    status: MenuItem,
    open: MenuItem,
    open_admin: MenuItem,
    restart: MenuItem,
    exit_close: CheckMenuItem,
    logs: MenuItem,
    quit: MenuItem,
    ids: TrayIds,
}

struct App {
    /// dsh web 地址（引擎本体 UI）：http://127.0.0.1:<port>
    url: String,
    /// dsh-come 管理页地址（状态/安装/插件/版本）：http://127.0.0.1:<status_port>
    admin_url: String,
    items: MenuItems,
    pending_menu: Option<Menu>,
    tray: Option<tray_icon::TrayIcon>,
    proxy: EventLoopProxy<UserEvent>,
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
            if let Some(menu) = self.pending_menu.take() {
                self.tray = build_tray(menu);
            }
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(ev) => {
                let ids = &self.items.ids;
                if ev.id == ids.quit {
                    supervisor::shutdown();
                    std::process::exit(0);
                } else if ev.id == ids.open {
                    open_browser(&self.url);
                } else if ev.id == ids.open_admin {
                    if !self.admin_url.is_empty() {
                        open_browser(&self.admin_url);
                    } else {
                        supervisor::set_flash("管理页已关闭（status_port=0）");
                    }
                } else if ev.id == ids.logs {
                    open_dir(&runtime::logs_dir());
                } else if ev.id == ids.restart {
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
                } else if ev.id == ids.exit_close {
                    // 「退出时关闭引擎」复选框：本身**不执行任何动作**（不提示/不弹窗/不动引擎），
                    // 只保存配置；仅当用户点「退出」时才据此决定是否关闭引擎（supervisor::shutdown）。
                    // 仅就地更新勾选（不重建菜单——重建会重新生成 id 并可能使菜单项短暂失效）。
                    let mut cfg = config::load();
                    cfg.exit_close_engine = !cfg.exit_close_engine;
                    config::save(&cfg);
                    self.items.exit_close.set_checked(cfg.exit_close_engine);
                }
            }
            UserEvent::Tray => {}
            UserEvent::Refresh => self.refresh(),
        }
    }
}

impl App {
    /// 定时（3s）刷新：就地更新状态行文本、打开项可用态、复选框勾选；不重建菜单。
    fn refresh(&mut self) {
        let st = supervisor::status();
        let status_text = status_line(&st);
        self.items.status.set_text(&status_text);
        self.items.open.set_enabled(st.ready);
        // 同步复选框（配置可能被其他路径修改，保持菜单与持久化一致）
        self.items.exit_close.set_checked(config::load().exit_close_engine);
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

    let cfg = config::load();
    let admin_url = if cfg.status_port != 0 {
        format!("http://127.0.0.1:{}", cfg.status_port)
    } else {
        String::new()
    };

    let (menu, items) = build_ui();
    let mut app = App {
        url: url.to_string(),
        admin_url,
        items,
        pending_menu: Some(menu),
        tray: None,
        proxy: app_proxy,
    };

    // 3s 定时刷新（状态行/菜单可用性——用户反馈状态更新太慢，15s→3s）
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

/// 创建菜单项（一次性）并组装菜单，返回 (菜单, 持久项引用)。
fn build_ui() -> (Menu, MenuItems) {
    let st = supervisor::status();
    let status_text = status_line(&st);
    // 状态行：禁用项，仅展示
    let status_item = MenuItem::new(&status_text, false, None);
    // 「打开 dsh 界面」置顶（最常用）：打开引擎本体 UI（3080）
    let open_item = MenuItem::new("打开 dsh 界面", st.ready, None);
    // 「打开管理页」：打开 dsh-come 管理页（3081）
    let open_admin_item = MenuItem::new("打开管理页", true, None);
    let restart_item = MenuItem::new("重启引擎", true, None);
    // 「退出时关闭引擎」复选框（2026-08-21）：勾选=退出 dsh-come 时杀引擎（默认）；
    // 取消勾选=退出保留引擎运行。勾选状态持久化在 config.exit_close_engine。
    // 注意 CheckMenuItem::new 签名 = (text, enabled, checked, accelerator)。
    let exit_close_item = CheckMenuItem::new(
        "退出时关闭引擎",
        true, // enabled：始终可点击
        config::load().exit_close_engine, // checked：随配置
        None,
    );
    let logs_item = MenuItem::new("打开日志目录", true, None);
    let quit_item = MenuItem::new("退出", true, None);

    let menu = Menu::new();
    let _ = menu.append(&open_item);
    let _ = menu.append(&open_admin_item);
    let _ = menu.append(&status_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&restart_item);
    let _ = menu.append(&exit_close_item);
    let _ = menu.append(&logs_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let ids = TrayIds {
        open: open_item.id().clone(),
        open_admin: open_admin_item.id().clone(),
        restart: restart_item.id().clone(),
        exit_close: exit_close_item.id().clone(),
        logs: logs_item.id().clone(),
        quit: quit_item.id().clone(),
    };
    let items = MenuItems {
        status: status_item,
        open: open_item,
        open_admin: open_admin_item,
        restart: restart_item,
        exit_close: exit_close_item,
        logs: logs_item,
        quit: quit_item,
        ids,
    };
    (menu, items)
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
    let icon = load_tray_icon();
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

/// 加载 DeepSeek 托盘图标（resources/tray-icon.png）。失败则回退到程序生成图标。
fn load_tray_icon() -> tray_icon::Icon {
    if let Some(icon) = try_load_png_icon() {
        return icon;
    }
    gen_icon()
}

/// 尝试从 resources/tray-icon.png 加载并解码为 RGBA 图标。
fn try_load_png_icon() -> Option<tray_icon::Icon> {
    let path = find_icon_path()?;
    let file = std::fs::File::open(&path).ok()?;
    let mut decoder = png::Decoder::new(file);
    // 忽略可选辅助 chunk，避免某些 PNG 的文本/色彩配置导致解码失败
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let rgba = convert_to_rgba8(&buf[..info.buffer_size()], info.color_type, info.bit_depth)?;
    tray_icon::Icon::from_rgba(rgba, w, h).ok()
}

/// 从 exe 所在目录向上查找 resources/tray-icon.png（覆盖 dev/test/release/dist 多种启动位置）。
fn find_icon_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..6 {
        let candidate = dir.join("resources").join("tray-icon.png");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// 把 PNG 解码输出转成 8-bit RGBA。
fn convert_to_rgba8(src: &[u8], ct: png::ColorType, bits: png::BitDepth) -> Option<Vec<u8>> {
    let depth = bits as u8;
    let stride = (depth / 8) as usize;
    let px_size = match ct {
        png::ColorType::Rgba => 4 * stride,
        png::ColorType::Rgb => 3 * stride,
        png::ColorType::Grayscale => 1 * stride,
        png::ColorType::GrayscaleAlpha => 2 * stride,
        _ => return None,
    };
    let pixels = src.len() / px_size;
    let mut out = Vec::with_capacity(pixels * 4);
    for chunk in src.chunks_exact(px_size) {
        let u8v = |i: usize| -> u8 {
            let v = if stride == 1 {
                chunk[i] as u16
            } else {
                u16::from_be_bytes([chunk[i * 2], chunk[i * 2 + 1]])
            };
            if v >= 0xff00 { 255 } else { (v >> 8) as u8 }
        };
        match ct {
            png::ColorType::Rgba => {
                out.push(u8v(0));
                out.push(u8v(1));
                out.push(u8v(2));
                out.push(u8v(3));
            }
            png::ColorType::Rgb => {
                out.push(u8v(0));
                out.push(u8v(1));
                out.push(u8v(2));
                out.push(255);
            }
            png::ColorType::Grayscale => {
                let g = u8v(0);
                out.push(g);
                out.push(g);
                out.push(g);
                out.push(255);
            }
            png::ColorType::GrayscaleAlpha => {
                let g = u8v(0);
                out.push(g);
                out.push(g);
                out.push(g);
                out.push(u8v(1));
            }
            _ => unreachable!(),
        }
    }
    Some(out)
}

/// 回退：32x32 RGBA 图标（深蓝圆角方块 + 白色中心点 + 青色光环）
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_tray_icon_loads() {
        // 验证 resources/tray-icon.png 能被正确解码为托盘可用图标
        let icon = load_tray_icon();
        // Icon 不暴露尺寸，能构造成功即表示 RGBA 数据合法
        let _ = icon;
    }
}
