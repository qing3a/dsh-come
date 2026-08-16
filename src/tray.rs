//! 系统托盘（tray-icon + winit 事件循环，主线程）。
//!
//! 复用 md-agent 的已验证模式（main.rs 托盘部分）：winit 事件循环 + MenuEvent 转发 +
//! StartCause::Init 里建托盘（避免平台侧显示问题）。差异：
//! - 菜单按 DSH 伴侣语义精简：状态行 / 打开界面 / 插件市场 / 检查更新 / 日志目录 / 退出
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
    /// 远程市场清单（verified.json）拉取完成（重建菜单展示新条目）
    MarketDone,
}

struct TrayIds {
    open: MenuId,
    open_sys: MenuId,
    update: MenuId,
    apply: MenuId,
    restart: MenuId,
    autostart: MenuId,
    status: MenuId,
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
                    open_browser(&self.url); // 独立窗口优先
                } else if ev.id == self.ids.open_sys {
                    open_system_browser(&self.url); // 强制系统浏览器（DevTools/多标签）
                } else if ev.id == self.ids.logs {
                    open_dir(&runtime::logs_dir());
                } else if ev.id == self.ids.status {
                    // 壳管理页：独立窗口打开（跟随向导页的浏览器窗口方案）
                    crate::status_page::open();
                } else if ev.id == self.ids.update {
                    // 后台线程检查更新（可能联网下载耗时）；完成后回传事件重建菜单。
                    // 结果不自动切换：验证通过只存 pending，菜单出现「应用更新」由用户确认。
                    let cfg = config::load();
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        let r = updater::check_and_install(&cfg);
                        let msg = match &r {
                            updater::UpdateResult::UpToDate(v) => format!("已是最新版本 {v}"),
                            updater::UpdateResult::Pending(v) => format!("发现新版本 {v}（已验证），菜单「应用更新」确认后生效"),
                            updater::UpdateResult::Failed(e) => format!("更新失败: {e}"),
                        };
                        supervisor::log(&msg);
                        supervisor::set_flash(&msg);
                        let _ = proxy.send_event(UserEvent::UpdateDone);
                    });
                } else if ev.id == self.ids.apply {
                    // 用户确认应用待确认更新：切换版本 → 重启引擎立即生效
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        match updater::apply_pending() {
                            Ok(ver) => {
                                supervisor::log(&format!("应用更新 v{ver}，正在重启引擎…"));
                                supervisor::set_flash(&format!("应用更新 v{ver}，正在重启引擎…"));
                                let cfg = config::load();
                                match supervisor::restart(&cfg, &ver) {
                                    Ok(()) => supervisor::log("引擎已用新版本重启"),
                                    Err(e) => supervisor::log(&format!("引擎重启失败（重启后生效）: {e}")),
                                }
                            }
                            Err(e) => {
                                supervisor::log(&format!("应用更新失败: {e}"));
                                supervisor::set_flash(&format!("应用更新失败: {e}"));
                            }
                        }
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
                                Ok(()) => {
                                    supervisor::log("引擎已重启");
                                    supervisor::set_flash("引擎已重启");
                                }
                                Err(e) => {
                                    supervisor::log(&format!("引擎重启失败: {e}"));
                                    supervisor::set_flash(&format!("引擎重启失败: {e}"));
                                }
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
                    let id = plugin_id_from_menu(&ev.id.0, "plugin:install:");
                    self.run_plugin_op(&id, true);
                } else if ev.id.0.starts_with("plugin:uninstall:") {
                    let id = plugin_id_from_menu(&ev.id.0, "plugin:uninstall:");
                    self.run_plugin_op(&id, false);
                } else if ev.id.0.starts_with("workbench:open:") {
                    // 工作台「打开」：取 entry 与依赖服务，浏览器/独立窗口打开；
                    // requires 非空时先提示依赖需用户自启（壳不代启动外部服务）
                    let id = plugin_id_from_menu(&ev.id.0, "workbench:open:");
                    match plugins::workbench_open(&id) {
                        Some((url, requires)) => {
                            if !requires.is_empty() {
                                supervisor::set_flash(&format!("工作台依赖（请先启动）：{}", requires.join("、")));
                            }
                            open_browser(&url);
                        }
                        None => {
                            supervisor::log(&format!("工作台 {id} 未在市场清单中（或缺少 entry）"));
                            supervisor::set_flash(&format!("工作台 {id} 不可用：清单缺少打开入口"));
                        }
                    }
                }
            }
            UserEvent::Tray => {}
            UserEvent::UpdateDone => self.rebuild(),
            UserEvent::PluginDone => self.rebuild(),
            UserEvent::MarketDone => self.rebuild(),
        }
    }
}

/// 从菜单项 id 提取插件 npm 包名：去掉 action 前缀与 `|组名` 后缀。
fn plugin_id_from_menu(item_id: &str, prefix: &str) -> String {
    item_id
        .trim_start_matches(prefix)
        .split('|')
        .next()
        .unwrap_or("")
        .to_string()
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
            supervisor::set_flash(&msg);
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
        // 首次向导成功时由向导接管打开引擎窗口（handed_off），托盘不再重复开（防双窗口）
        if st.ready && !self.auto_opened && !crate::wizard::handed_off() {
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
    // 定时只是兜底（15s 对状态行足够，参考 md-agent 用 30s）。
    {
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(15));
            let _ = proxy.send_event(UserEvent::UpdateDone);
        });
    }
    // 后台拉取远程市场清单（verified.json）：离线/失败静默回退内置清单，不打扰小白。
    {
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || {
            match plugins::refresh_market_catalog() {
                Ok(n) if n > 0 => {
                    supervisor::log(&format!("市场清单已更新（远程 {n} 个插件）"));
                    supervisor::set_flash(&format!("市场清单已更新（远程 {n} 个插件）"));
                }
                Ok(_) => supervisor::log("市场清单拉取成功（无远程条目）"),
                Err(e) => supervisor::log(&format!("市场清单拉取失败（用内置清单）: {e}")),
            }
            let _ = proxy.send_event(UserEvent::MarketDone);
        });
    }
    // 不做启动自动检查更新（加快启动速度，用户需要时手动「检查更新」）

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
    let open_sys_item = MenuItem::new("在浏览器中打开", st.ready, None);
    let update_item = MenuItem::new("检查更新", true, None);
    // 有已验证待应用的更新 → 菜单提示「应用更新」（更新前询问，用户确认才切换）
    let apply_item = state.pending.as_ref().map(|v| {
        MenuItem::with_id(
            "apply-update",
            format!("应用更新 → v{v}"),
            true,
            None,
        )
    });
    let restart_item = MenuItem::new("重启引擎", true, None);
    let status_item_page = MenuItem::new("运行状态（壳管理页）", true, None);
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
    let _ = menu.append(&open_sys_item);
    let _ = menu.append(&market_sub);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&update_item);
    if let Some(item) = &apply_item {
        let _ = menu.append(item);
    }
    let _ = menu.append(&restart_item);
    let _ = menu.append(&status_item_page);
    let _ = menu.append(&autostart_item);
    let _ = menu.append(&logs_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let ids = TrayIds {
        open: open_item.id().clone(),
        open_sys: open_sys_item.id().clone(),
        update: update_item.id().clone(),
        apply: apply_item.as_ref().map(|i| i.id().clone()).unwrap_or_else(|| MenuId::new("apply-update-none")),
        restart: restart_item.id().clone(),
        autostart: autostart_item.id().clone(),
        status: status_item_page.id().clone(),
        logs: logs_item.id().clone(),
        quit: quit_item.id().clone(),
    };
    (menu, ids)
}

/// 市场子菜单：工作台按场景分组优先 + 单件工具按标签分组 + 「全部」完整列表。
/// 工作台条目（kind=workbench，有 entry）点击 = 打开本地资产（不装/卸 npm 包）；
/// 单件工具 = 装/卸（后台执行）。分组子菜单的项 id 附 `|组名` 后缀，
/// 事件处理时剥离（见 plugin_id_from_menu）。
fn build_market_submenu() -> Submenu {
    let sub = Submenu::new("市场", true);
    let installed = plugins::installed_plugins();
    let catalog = plugins::market_catalog();
    if catalog.is_empty() {
        let _ = sub.append(&MenuItem::new("（暂无已验证插件）", false, None));
        return sub;
    }
    let groups = plugins::marketplace_groups(&catalog);
    if !groups.is_empty() {
        for (label, items) in &groups {
            let gsub = Submenu::new(label, true);
            for p in items {
                append_plugin_item(&gsub, p, &installed, label);
            }
            let _ = sub.append(&gsub);
        }
        let _ = sub.append(&PredefinedMenuItem::separator());
    }
    let all = Submenu::new("全部", true);
    for p in &catalog {
        append_plugin_item(&all, p, &installed, "");
    }
    let _ = sub.append(&all);
    let _ = sub.append(&PredefinedMenuItem::separator());
    let _ = sub.append(&MenuItem::new(format!("已安装 {} 个插件", installed.len()), false, None));
    sub
}

/// 单个商品行：●/○ 状态 + ✓已验证 + 动作。工作台 → [打开]（本地资产入口）；
/// 单件工具 → [安装/卸载]（npm 包）。group 非空时 id 附 `|group` 后缀。
fn append_plugin_item(sub: &Submenu, p: &plugins::PluginInfo, installed: &[String], group: &str) {
    let has = installed.iter().any(|i| i == &p.id);
    let marker = if p.verified { "✓已验证" } else { "未验证" };
    let suffix = if group.is_empty() { String::new() } else { format!("|{group}") };
    if p.is_workbench() {
        // 工作台：本地资产形态 → 直接打开入口（无 npm 包可装）；资产缺失标 ✗
        let present = p.entry.as_deref().map_or(false, local_asset_present);
        let dot = if present { "●" } else { "✗" };
        let id = format!("workbench:open:{}{suffix}", p.id);
        let label = format!("{dot} {} v{} {marker}　[打开]", p.name, p.version);
        let _ = sub.append(&MenuItem::with_id(id, label, true, None));
        return;
    }
    let dot = if has { "●" } else { "○" };
    let (id, action) = if has {
        (format!("plugin:uninstall:{}{suffix}", p.id), "卸载".to_string())
    } else {
        (format!("plugin:install:{}{suffix}", p.id), "安装".to_string())
    };
    let label = format!("{dot} {} v{} {marker}　[{action}]", p.name, p.version);
    let it = MenuItem::with_id(id, label, true, None);
    let _ = sub.append(&it);
}

/// 工作台本地资产是否存在：entry 为 file:// 本地路径时检查文件；URL 视为可用。
/// pub(crate)：status_page 渲染工作台状态复用同一判定。
pub(crate) fn local_asset_present(entry: &str) -> bool {
    if let Some(path) = entry.strip_prefix("file:///") {
        let sep = std::path::MAIN_SEPARATOR;
        std::path::Path::new(&path.replace('/', &sep.to_string())).is_file()
    } else {
        true
    }
}

/// 状态行文案：DSH 伴侣 v0.1.0-rc.6｜运行中 ✓ / 启动中… / 已停止 ✗（附错误）
/// 优先级：安装 stage > 瞬时提示（插件/更新结果）> 引擎状态；有更新待确认时追加 ⚑ 提示
fn status_line(st: &supervisor::SuperStatus, state: &runtime::State) -> String {
    let ver = st
        .version
        .clone()
        .or_else(|| state.current.clone())
        .unwrap_or_else(|| "未安装".to_string());
    let body = if !st.stage.is_empty() {
        st.stage.clone() // 阶段提示（首次安装/下载/启动中）优先
    } else if let Some(f) = supervisor::flash() {
        f // 瞬时提示（插件装/卸、更新结果），12s 后消失
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
    let mut line = format!("DSH 伴侣 v{ver}｜{body}");
    if let Some(p) = &state.pending {
        if st.stage.is_empty() {
            line = format!("{line} ⚑ 有更新 v{p}（菜单应用更新）");
        }
    }
    line
}

fn build_tray(menu: Menu) -> Option<tray_icon::TrayIcon> {
    let icon = gen_tray_icon();
    match tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("DSH 伴侣")
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
    // 32x32 输出：Windows 托盘图标标准尺寸（tray-icon 文档建议；64 传系统缩放反而可能显小/发糊）
    let (w, h) = (32u32, 32u32);
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
        assert_eq!(pm.width(), 32, "Windows 托盘标准尺寸 32x32");
        assert_eq!(pm.height(), 32);
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

    /// --app 独立窗口参数格式（Edge/Chrome 规范）
    #[test]
    fn app_window_arg_format() {
        assert_eq!(app_browser_args("http://127.0.0.1:3080"), vec!["--app=http://127.0.0.1:3080"]);
    }

    /// 窗口几何参数：未记录过 → 空（浏览器默认位置/大小）；记录过 → 带 --window-position/--window-size
    #[test]
    fn window_flags_from_config() {
        // 隔离 home，避免污染真实 config.json
        let tmp = std::env::temp_dir().join(format!("dsh-wflags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("DSH_DESKTOP_HOME", &tmp);

        // 未记录：空参数
        assert!(window_flags().is_empty());

        // 记录位置 + 大小后：按 DIP 整数格式输出
        let mut cfg = config::load();
        cfg.window_pos = Some((120, 80));
        cfg.window_size = Some((1440, 900));
        config::save(&cfg);
        let flags = window_flags();
        assert!(flags.contains(&"--window-position=120,80".to_string()));
        assert!(flags.contains(&"--window-size=1440,900".to_string()));

        // 只有位置：只输出 position
        let mut cfg = config::load();
        cfg.window_pos = Some((10, 20));
        cfg.window_size = None;
        config::save(&cfg);
        let flags = window_flags();
        assert_eq!(flags, vec!["--window-position=10,20".to_string()]);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("DSH_DESKTOP_HOME");
    }

    /// DIP 换算：96 DPI（1x 缩放）物理像素 == DIP；150% 缩放时物理 1920x1080 → DIP 1280x720
    #[test]
    fn dip_scale_at_150_percent() {
        let scale: f64 = 144.0 / 96.0;
        assert_eq!(((1920.0 / scale).round() as i32, (1080.0 / scale).round() as i32), (1280, 720));
        assert_eq!(((96.0 / scale).round() as i32, (96.0 / scale).round() as i32), (64, 64));
    }

    /// 浏览器探测：命中的路径必须真实存在；未命中合法（调用方回退系统浏览器）
    #[test]
    fn browser_probe_exists_when_hit() {
        if let Some(p) = find_app_browser() {
            assert!(p.is_file(), "探测到的浏览器必须存在: {}", p.display());
        }
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

// ---------- 打开界面：优先独立窗口（--app），回退系统浏览器 ----------

/// Chromium 系浏览器候选（Edge 优先——Win10/11 系统自带，无需用户安装）
fn find_app_browser() -> Option<std::path::PathBuf> {
    const CANDIDATES: [&str; 4] = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    CANDIDATES.iter().map(std::path::PathBuf::from).find(|p| p.is_file())
}

/// --app 参数（独立窗口：无地址栏 + 任务栏图标，让官方 Web UI 看起来就是桌面 App）
fn app_browser_args(url: &str) -> Vec<String> {
    vec![format!("--app={url}")]
}

/// 从 config 读用户最后使用的窗口几何（DIP），拼 Chromium --window-position/--window-size 参数。
/// 只对 --app 窗口生效（引擎/向导/壳管理页统一用此路径打开）；没记录过 → 空参数，浏览器默认。
fn window_flags() -> Vec<String> {
    let cfg = config::load();
    let mut args = Vec::new();
    if let Some((x, y)) = cfg.window_pos {
        args.push(format!("--window-position={x},{y}"));
    }
    if let Some((w, h)) = cfg.window_size {
        args.push(format!("--window-size={w},{h}"));
    }
    args
}

/// 以独立窗口打开 URL（Edge/Chrome --app）；找不到浏览器则 false（调用方回退）。
/// 打开成功后后台记录窗口几何（位置/大小，下次启动恢复——见 record_window_geometry）。
pub fn open_app_window(url: &str) -> bool {
    if let Some(browser) = find_app_browser() {
        let mut cmd = std::process::Command::new(&browser);
        cmd.args(app_browser_args(url)).args(window_flags());
        supervisor::hide_window(&mut cmd);
        if let Ok(child) = cmd.spawn() {
            record_window_geometry(child.id());
            return true;
        }
    }
    false
}

/// 打开界面：优先独立桌面窗口（--app）；无 Edge/Chrome 时回退系统默认浏览器
pub fn open_browser(url: &str) {
    if open_app_window(url) {
        return;
    }
    open_system_browser(url);
}

/// 强制用系统默认浏览器打开（调试/高级用户：可看 DevTools、多标签）
pub fn open_system_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

pub fn open_dir(dir: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &dir.display().to_string()])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

// ---------- 开机自启（HKCU Run，无需管理员权限） ----------

const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_NAME: &str = "DSH Come";

/// 当前 exe 路径（自启项目标；带引号防路径空格）
fn autostart_target() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "dsh-come.exe".to_string());
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
        // 清理旧名（dsh-desktop / dsh-companion 时代）的注册表残留，避免多自启项指向旧 exe
        let _ = key.delete_value("DSH Desktop");
        let _ = key.delete_value("DSH Companion");
        key.set_value(AUTOSTART_NAME, &autostart_target())?;
    } else {
        let _ = key.delete_value(AUTOSTART_NAME);
    }
    Ok(())
}

// ---------- 窗口几何记录（--app 窗口位置/大小持久化） ----------
//
// 浏览器 --app 窗口的几何由 Chromium 自持，壳无法直接读取/设置其内存状态；
// 用 Win32 枚举顶层窗口，按 pid 匹配该浏览器进程的可见主窗口，把矩形（物理像素）
// 按该窗口 DPI 换算成 DIP 存进 config。下次打开 --app 时经 window_flags 恢复。
// 每 5s 采样一次，直到窗口消失（连续 6 次找不到）或超时 5 分钟（浏览器冷启动慢）。

struct GeometryState {
    target_pid: u32,
    rect: Option<(i32, i32, i32, i32)>, // left, top, width, height（物理像素）
    dpi: u32,
}

unsafe extern "system" fn enum_window_proc(hwnd: windows_sys::Win32::Foundation::HWND, lparam: windows_sys::Win32::Foundation::LPARAM) -> windows_sys::Win32::Foundation::BOOL {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    };
    let state = &mut *(lparam as *mut GeometryState);
    if IsWindowVisible(hwnd) == 0 {
        return 1; // 继续枚举
    }
    let mut wpid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut wpid);
    if wpid != state.target_pid {
        return 1;
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if GetWindowRect(hwnd, &mut r) == 0 {
        return 1;
    }
    state.dpi = GetDpiForWindow(hwnd);
    state.rect = Some((r.left, r.top, r.right - r.left, r.bottom - r.top));
    0 // 找到目标窗口，停止枚举
}

/// 枚举所有顶层窗口，找属于 pid 的可见窗口矩形，换算为 DIP 的 (pos, size)
fn find_window_geometry(pid: u32) -> Option<((i32, i32), (i32, i32))> {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
    let mut state = GeometryState { target_pid: pid, rect: None, dpi: 96 };
    let ptr = &mut state as *mut GeometryState;
    unsafe {
        EnumWindows(Some(enum_window_proc), ptr as windows_sys::Win32::Foundation::LPARAM);
    }
    let (l, t, w, h) = state.rect?;
    // 粗校验：过小的窗口是通知/工具窗而非主窗口
    if w < 200 || h < 120 {
        return None;
    }
    let scale = state.dpi.max(1) as f64 / 96.0;
    Some((
        (((l as f64) / scale).round() as i32, ((t as f64) / scale).round() as i32),
        (((w as f64) / scale).round() as i32, ((h as f64) / scale).round() as i32),
    ))
}

fn save_window_geometry(pos: (i32, i32), size: (i32, i32)) {
    let mut cfg = config::load();
    cfg.window_pos = Some(pos);
    cfg.window_size = Some(size);
    config::save(&cfg);
}

/// 后台记录 --app 窗口几何：每 5s 采样一次；窗口消失（连续 6 次）或超时即停。
/// 位置/大小变化时写 config（下次启动恢复），写不成功静默（不影响主流程）。
fn record_window_geometry(pid: u32) {
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut misses = 0u32;
        while std::time::Instant::now() < deadline {
            match find_window_geometry(pid) {
                Some((pos, size)) => {
                    misses = 0;
                    save_window_geometry(pos, size);
                }
                None => {
                    misses += 1;
                    if misses >= 6 {
                        break; // 连续 ~30s 找不到 → 窗口已关闭
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}
