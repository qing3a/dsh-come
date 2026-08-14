//! 系统托盘（tray-icon + winit 事件循环，主线程）。
//!
//! 复用 md-agent 的已验证模式（main.rs 托盘部分）：winit 事件循环 + MenuEvent 转发 +
//! StartCause::Init 里建托盘（避免平台侧显示问题）。差异：
//! - 菜单按 DSH 桌面版语义精简：状态行 / 打开界面 / 插件市场 / 检查更新 / 日志目录 / 退出
//! - 2s 定时重建菜单，让状态行（版本/就绪/错误）对小白实时可见
//! - 检查更新与插件安装都放后台线程，完成后事件回传触发菜单重建

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use winit::application::ApplicationHandler;
use winit::event::StartCause;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};

use crate::config;
use crate::plugins;
use crate::runtime;
use crate::supervisor;
use crate::updater;

enum UserEvent {
    Tray,
    Menu(tray_icon::menu::MenuEvent),
    /// 检查更新完成（重建菜单反映新状态）
    UpdateDone,
    /// 插件安装/卸载完成（重建菜单反映新状态）
    PluginDone,
}

struct TrayIds {
    open: MenuId,
    update: MenuId,
    restart: MenuId,
    autostart: MenuId,
    logs: MenuId,
    quit: MenuId,
}

struct App {
    url: String,
    ids: TrayIds,
    menu: Option<Menu>,
    tray: Option<tray_icon::TrayIcon>,
    /// 事件循环退出前重建菜单/转发后台线程完成信号（winit 0.30：proxy 须在外部创建）
    proxy: EventLoopProxy<UserEvent>,
    /// 已自动打开过浏览器（就绪后只自动开一次；之后用户自己点菜单）
    auto_opened: bool,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _el: &ActiveEventLoop,
        _id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        // 事件循环真正运行后再建托盘图标（避免平台侧显示问题）
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
                    // 退出前清理 dsh 引擎子进程（防残留 Node 占端口）
                    supervisor::shutdown();
                    std::process::exit(0);
                } else if ev.id == self.ids.open {
                    open_browser(&self.url);
                } else if ev.id == self.ids.logs {
                    open_dir(&runtime::logs_dir());
                } else if ev.id == self.ids.update {
                    // 后台线程跑更新（可能联网安装耗时）；完成后回传事件重建菜单
                    let cfg = config::load();
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        let r = updater::check_and_install(&cfg);
                        let msg = match &r {
                            updater::UpdateResult::UpToDate(v) => format!("已是最新版本 {v}"),
                            updater::UpdateResult::Installed(v) => format!("已更新到 {v}（重启 dsh 生效）"),
                            updater::UpdateResult::Failed(e) => format!("更新失败: {e}"),
                        };
                        supervisor::log(&msg);
                        let _ = proxy.send_event(UserEvent::UpdateDone);
                    });
                } else if ev.id == self.ids.restart {
                    // 重启引擎：更新安装后一键立即生效（v2 更新流程的配套操作）
                    let cfg = config::load();
                    let ver = runtime::load_state().current.unwrap_or_default();
                    if ver.is_empty() {
                        supervisor::log("尚未安装 DSH 版本，无法重启");
                    } else {
                        let proxy = self.proxy.clone();
                        std::thread::spawn(move || {
                            let r = supervisor::restart(&cfg, &ver);
                            match r {
                                Ok(()) => supervisor::log("引擎已重启"),
                                Err(e) => supervisor::log(&format!("引擎重启失败: {e}")),
                            }
                            let _ = proxy.send_event(UserEvent::UpdateDone);
                        });
                    }
                } else if ev.id == self.ids.autostart {
                    // 开机自启勾选翻转（注册表 HKCU Run；无需管理员权限）
                    let on = !autostart_enabled();
                    match set_autostart(on) {
                        Ok(()) => supervisor::log(&format!("开机自启：{}", if on { "已开启" } else { "已关闭" })),
                        Err(e) => supervisor::log(&format!("设置开机自启失败: {e}")),
                    }
                    self.rebuild();
                } else if ev.id.0.starts_with("plugin:install:") {
                    let id = ev.id.0.trim_start_matches("plugin:install:").to_string();
                    self.run_plugin_op(&id, true);
                } else if ev.id.0.starts_with("plugin:uninstall:") {
                    let id = ev.id.0.trim_start_matches("plugin:uninstall:").to_string();
                    self.run_plugin_op(&id, false);
                }
            }
            UserEvent::Tray => {}
            UserEvent::UpdateDone => self.rebuild(),
            UserEvent::PluginDone => self.rebuild(),
        }
    }
}

impl App {
    /// 后台执行插件安装/卸载（首次可能触发 pnpm 安装，耗时）；完成后回传事件重建菜单
    fn run_plugin_op(&self, id: &str, install: bool) {
        let cfg = config::load();
        let proxy = self.proxy.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            let r = if install {
                plugins::install_plugin(&cfg, &id)
            } else {
                plugins::uninstall_plugin(&cfg, &id)
            };
            let msg = match r {
                Ok(m) => m,
                Err(e) => format!("插件操作失败: {e}"),
            };
            supervisor::log(&msg);
            let _ = proxy.send_event(UserEvent::PluginDone);
        });
    }

    /// 重建托盘菜单；首次就绪后自动打开浏览器（小白双击后不需要知道去哪）
    fn rebuild(&mut self) {
        if let Some(tray) = &self.tray {
            let (menu, ids) = build_menu();
            let _ = tray.set_menu(Some(Box::new(menu)));
            self.ids = ids;
        }
        let st = supervisor::status();
        if st.ready && !self.auto_opened {
            self.auto_opened = true;
            open_browser(&self.url);
        }
    }
}

pub fn run_tray(url: &str) {
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("创建事件循环失败: {e}");
            return;
        }
    };
    // 托盘事件转发到 winit 用户事件（各自独立的 proxy，被闭包 move）
    let proxy = event_loop.create_proxy();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |_e| {
        let _ = proxy.send_event(UserEvent::Tray);
    }));
    let proxy = event_loop.create_proxy();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));
    // App 持有的 proxy：后台线程（检查更新）完成信号回传
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

    // 兜底定时重建（15s）：刷新状态行。⚠️ 不能太频繁——之前 2s 重建导致鼠标悬停菜单时
    // 菜单被 set_menu 替换而消失/闪烁（用户实测反馈）。重建主要靠事件驱动（启动/更新/插件完成），
    // 定时只是兜底（15s 对小白状态行足够，参考 md-agent 用 30s）。
    {
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(15));
            let _ = proxy.send_event(UserEvent::UpdateDone);
        });
    }

    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("托盘事件循环错误: {e}");
    }
}

/// 构建托盘菜单（状态行 disabled 纯展示；「打开界面」在未就绪时禁用）
fn build_menu() -> (Menu, TrayIds) {
    let st = supervisor::status();
    let state = runtime::load_state();

    let status_text = status_line(&st, &state);
    let status_item = MenuItem::new(&status_text, false, None);

    let open_item = MenuItem::new("打开界面", st.ready, None);
    let update_item = MenuItem::new("检查更新", true, None);
    let restart_item = MenuItem::new("重启引擎", true, None);
    let autostart_item = CheckMenuItem::with_id(
        "autostart",
        if autostart_enabled() { "开机自启：开" } else { "开机自启：关" },
        true,
        autostart_enabled(),
        None,
    );
    let logs_item = MenuItem::new("打开日志目录", true, None);
    let quit_item = MenuItem::new("退出", true, None);

    let market_sub = build_market_submenu();

    let menu = Menu::new();
    let _ = menu.append(&status_item);
    let _ = menu.append(&open_item);
    let _ = menu.append(&market_sub);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&update_item);
    let _ = menu.append(&restart_item);
    let _ = menu.append(&autostart_item);
    let _ = menu.append(&logs_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let ids = TrayIds {
        open: open_item.id().clone(),
        update: update_item.id().clone(),
        restart: restart_item.id().clone(),
        autostart: autostart_item.id().clone(),
        logs: logs_item.id().clone(),
        quit: quit_item.id().clone(),
    };
    (menu, ids)
}

/// 插件市场子菜单：内置可信清单（✓已验证）+ 当前已装状态；点击即装/卸（后台执行）
fn build_market_submenu() -> Submenu {
    let sub = Submenu::new("插件市场", true);
    let installed = plugins::installed_plugins();
    let catalog = plugins::builtin_marketplace();
    if catalog.is_empty() {
        let _ = sub.append(&MenuItem::new("（暂无已验证插件）", false, None));
        return sub;
    }
    for p in &catalog {
        let has = installed.iter().any(|i| i == &p.id);
        let dot = if has { "●" } else { "○" };
        let marker = if p.verified { "✓已验证" } else { "未验证" };
        let action = if has { "卸载" } else { "安装" };
        let id = if has { format!("plugin:uninstall:{}", p.id) } else { format!("plugin:install:{}", p.id) };
        let label = format!("{dot} {} v{} {marker}　[{}]", p.name, p.version, action);
        let it = MenuItem::with_id(id, label, true, None);
        let _ = sub.append(&it);
    }
    let _ = sub.append(&PredefinedMenuItem::separator());
    let _ = sub.append(&MenuItem::new(format!("已安装 {} 个插件", installed.len()), false, None));
    sub
}

/// 状态行文案：DSH 桌面版 v0.1.0-rc.6｜运行中 ✓ / 启动中… / 已停止 ✗（附错误）
fn status_line(st: &supervisor::SuperStatus, state: &runtime::State) -> String {
    let ver = st
        .version
        .clone()
        .or_else(|| state.current.clone())
        .unwrap_or_else(|| "未安装".to_string());
    let body = if !st.stage.is_empty() {
        st.stage.clone() // 阶段提示（首次安装/下载/启动中）优先
    } else if st.running {
        if st.ready {
            format!("运行中 ✓ http://127.0.0.1:{}", st.port)
        } else {
            "启动中…".to_string()
        }
    } else if let Some(e) = &st.last_error {
        format!("已停止 ✗（{e}）")
    } else if ver == "未安装" {
        "首次运行：正在安装 DSH…".to_string()
    } else {
        "已停止 ✗".to_string()
    };
    format!("DSH 桌面版 v{ver}｜{body}")
}

fn build_tray(menu: Menu) -> Option<tray_icon::TrayIcon> {
    let icon = gen_tray_icon();
    match tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("DSH 桌面版")
        .with_icon(icon)
        .build()
    {
        Ok(t) => {
            supervisor::log("托盘图标已创建（官方 DSH logo）");
            Some(t)
        }
        Err(e) => {
            // 托盘创建失败不 panic：记日志继续跑（浏览器界面不受影响）
            supervisor::log(&format!("创建托盘图标失败（无托盘，界面仍可用）: {e}"));
            None
        }
    }
}

/// 托盘图标：优先用 DeepSeek Harness 官方 favicon（内嵌 SVG → resvg 光栅化）；
/// 渲染失败回退程序画图标。**颜色跟随系统主题**：
/// - 深色任务栏（AppsUseLightTheme=0）→ 白色 logo
/// - 浅色任务栏（AppsUseLightTheme=1）→ 黑色 logo（原图）
/// 否则浅色主题下白 logo 完全隐形（用户实测「没看到图标」）。
/// 出处：`apps/web/public/favicon.svg`（deepseek-harness，MIT 仓库资产）。
/// 图标为 DeepSeek AI 商标，仅作引用，不暗示官方联名。
fn gen_tray_icon() -> tray_icon::Icon {
    if let Some(pm) = rasterize_official() {
        let (w, h) = (pm.width(), pm.height());
        if let Ok(icon) = tray_icon::Icon::from_rgba(pm.data().to_vec(), w, h) {
            return icon;
        }
    }
    gen_icon()
}

/// 系统是否为浅色主题（浅色任务栏）：HKCU\...\Themes\Personalize\AppsUseLightTheme
/// 读不到时按浅色处理（Windows 默认浅色任务栏，白色 logo 会隐形）
fn system_uses_light_theme() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .ok();
    let v: Option<u32> = key.and_then(|k| k.get_value("AppsUseLightTheme").ok());
    v.unwrap_or(1) != 0
}

/// 官方 favicon.svg 光栅化（颜色随系统主题：深色任务栏白、浅色任务栏黑）。返回非预乘 RGBA Pixmap。
fn rasterize_official() -> Option<resvg::tiny_skia::Pixmap> {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::{Options, Tree};
    const SVG: &str = include_str!("../assets/favicon.svg");
    let light = system_uses_light_theme();
    // 浅色主题用原图（黑 logo 在浅色任务栏可见）；深色主题黑→白（原 svg 的
    // prefers-color-scheme 媒体查询被整体替换，逻辑归我们管）
    let svg = if light {
        SVG.to_string()
    } else {
        SVG.replace("fill=\"#000\"", "fill=\"#fff\"")
    };
    let tree = Tree::from_str(&svg, &Options::default()).ok()?;
    let (w, h) = (64u32, 64u32); // 64 渲染，HiDPI 更清晰
    let mut pixmap = Pixmap::new(w, h)?;
    resvg::render(&tree, Transform::default(), &mut pixmap.as_mut());
    Some(pixmap) // 纯黑/白形状 alpha 仅 0/255，无半透明 → 无需 unpremultiply
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    /// 官方图标光栅化成功且存在非透明像素（黑或白，取决于系统主题；任务栏可见的前提）
    #[test]
    fn official_icon_renders() {
        let pm = rasterize_official().expect("SVG 应能光栅化");
        assert_eq!(pm.width(), 64);
        assert_eq!(pm.height(), 64);
        let has_opaque = pm.data().chunks_exact(4).any(|px| px[3] > 0);
        assert!(has_opaque, "应存在非透明像素（浅色主题黑 logo / 深色主题白 logo）");
    }

    /// 兜底程序画图标仍可用（像素非空 + 存在非透明像素）
    #[test]
    fn fallback_icon_builds() {
        let rgba = gen_icon_rgba(32, 32);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        assert!(rgba.chunks_exact(4).any(|px| px[3] > 0), "应存在非透明像素");
    }
}

/// 32x32 RGBA 图标：深蓝圆角方块 + 白色中心点 + 青色光环（「盒子/桌面」意象，象征打开即用）
fn gen_icon() -> tray_icon::Icon {
    let (w, h) = (32u32, 32u32);
    let rgba = gen_icon_rgba(w, h);
    tray_icon::Icon::from_rgba(rgba, w, h).expect("生成图标失败")
}

fn gen_icon_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let blue = [34u8, 74u8, 160u8, 255u8];
    let white = [238u8, 242u8, 255u8, 255u8];
    let cyan = [64u8, 192u8, 190u8, 255u8];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            // 圆角方块主体（深蓝）
            let body = x >= 6 && x <= 27 && y >= 6 && y <= 27;
            let corner = (x <= 9 && y <= 9) || (x >= 24 && y <= 9) || (x <= 9 && y >= 24) || (x >= 24 && y >= 24);
            if body && !corner {
                rgba[i..i + 4].copy_from_slice(&blue);
            } else {
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            if body && !corner {
                // 中心点（白）
                let dx = x as i32 - 16;
                let dy = y as i32 - 16;
                if dx * dx + dy * dy <= 16 {
                    rgba[i..i + 4].copy_from_slice(&white);
                }
                // 外圈光环（青）
                if dx * dx + dy * dy > 45 && dx * dx + dy * dy <= 64 {
                    rgba[i..i + 4].copy_from_slice(&cyan);
                }
            }
        }
    }
    rgba
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn open_dir(dir: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &dir.display().to_string()])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

// ---------- 开机自启（HKCU Run，无需管理员权限） ----------

const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_NAME: &str = "DSH Desktop";

/// 当前 exe 路径（自启项目标；带引号防路径空格）
fn autostart_target() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "dsh-desktop.exe".to_string());
    format!("\"{exe}\"")
}

/// 开机自启是否已开启（注册表 Run 键下存在本程序）
pub fn autostart_enabled() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(AUTOSTART_KEY, winreg::enums::KEY_READ) {
        let v: Option<String> = key.get_value(AUTOSTART_NAME).ok();
        v.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    } else {
        false
    }
}

/// 设置开机自启（true=写入 Run 键，false=删除）
pub fn set_autostart(on: bool) -> std::io::Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(AUTOSTART_KEY, KEY_WRITE)?;
    if on {
        key.set_value(AUTOSTART_NAME, &autostart_target())?;
    } else {
        let _ = key.delete_value(AUTOSTART_NAME);
    }
    Ok(())
}
