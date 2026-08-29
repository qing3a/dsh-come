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
use std::sync::atomic::{AtomicU64, Ordering};

/// 主线程最后活动时刻（unix 秒）：refresh() 每 1s 刷新；心跳线程据此检测主循环是否卡死
static MAIN_ACTIVITY: AtomicU64 = AtomicU64::new(0);

/// 定时兜底重建托盘的间隔（秒）：恢复 Explorer 重启检测之外的未知失效（睡眠唤醒/系统更新等）
const TRAY_REBUILD_INTERVAL_SECS: u64 = 600;

enum UserEvent {
    Tray,
    Menu(tray_icon::menu::MenuEvent),
    Refresh,
    /// 重建托盘（Explorer 重启检测 / 定时兜底 / 心跳恢复尝试触发）
    RefreshTray,
}

struct TrayIds {
    open: MenuId,
    open_admin: MenuId,
    restart: MenuId,
    exit_close: MenuId,
    logs: MenuId,
    check_update: MenuId,
    do_update: MenuId,
    quit: MenuId,
}

/// 持久持有的菜单项引用（创建一次，后续只就地更新文本/勾选/可用态，不重建）。
/// 只保留会被动态更新的项；其余菜单项（打开管理页/重启/日志/退出）append 进菜单后
/// 不再需要引用——事件处理只比对 id（TrayIds）。
struct MenuItems {
    status: MenuItem,
    open: MenuItem,
    exit_close: CheckMenuItem,
    /// 「更新到 vX」动态项：无可用更新时禁用占位，发现新版本后就地更新文本/启用
    update: MenuItem,
    ids: TrayIds,
}

struct App {
    /// dsh web 地址（引擎本体 UI）：http://127.0.0.1:<port>
    url: String,
    items: MenuItems,
    pending_menu: Option<Menu>,
    tray: Option<tray_icon::TrayIcon>,
    proxy: EventLoopProxy<UserEvent>,
    /// 当前 Windows 模式（true=浅色，false=深色），refresh 时比对变化切换托盘图标
    current_is_light: bool,
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
                    // 动态查实际管理页端口（固定端口被占时回退随机端口）
                    if let Some(p) = crate::status::admin_port() {
                        open_browser(&format!("http://127.0.0.1:{p}"));
                    } else {
                        supervisor::set_flash(crate::i18n::tr(
                            "管理页不可用（status_port=0 或启动失败）",
                            "Admin page unavailable (status_port=0 or failed to start)",
                        ));
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
                                supervisor::set_flash(crate::i18n::tr("引擎已重启", "Engine restarted"));
                            }
                            Err(e) => {
                                supervisor::log(&format!("引擎重启失败: {e}"));
                                supervisor::set_flash(&format!(
                                    "{}: {e}",
                                    crate::i18n::tr("引擎重启失败", "Engine restart failed")
                                ));
                            }
                        }
                        let _ = proxy.send_event(UserEvent::Refresh);
                    });
                } else if ev.id == ids.check_update {
                    // 手动检查更新（无视每日节流）；结果 flash + 刷新（更新项可用态）
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        match crate::updater::check(true) {
                            Ok(Some(info)) => {
                                supervisor::log(&format!("发现新版本 v{}", info.version));
                                supervisor::set_flash(&format!(
                                    "{} v{}",
                                    crate::i18n::tr("发现新版本", "New version available"),
                                    info.version
                                ));
                            }
                            Ok(None) => {
                                supervisor::set_flash(crate::i18n::tr(
                                    "已是最新版本",
                                    "Already up to date",
                                ));
                            }
                            Err(e) => {
                                supervisor::set_flash(&format!(
                                    "{}: {e}",
                                    crate::i18n::tr("检查更新失败", "Update check failed")
                                ));
                            }
                        }
                        let _ = proxy.send_event(UserEvent::Refresh);
                    });
                } else if ev.id == ids.do_update {
                    // 下载 + 校验 + 换装：成功后本进程退出，换装脚本接管替换/重启
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        let Some(info) = crate::updater::available() else {
                            supervisor::set_flash(crate::i18n::tr(
                                "暂无可用更新",
                                "No update available",
                            ));
                            let _ = proxy.send_event(UserEvent::Refresh);
                            return;
                        };
                        supervisor::set_flash(&format!(
                            "{} v{}…",
                            crate::i18n::tr("正在下载更新", "Downloading update"),
                            info.version
                        ));
                        match crate::updater::download_and_verify(&info) {
                            Ok(new_path) => {
                                supervisor::log(&format!(
                                    "更新 v{} 下载并校验通过，开始换装（备份 .bak）",
                                    info.version
                                ));
                                crate::notify::toast(
                                    crate::i18n::tr("DSH 伴侣", "DSH Companion"),
                                    &format!(
                                        "{} v{}",
                                        crate::i18n::tr(
                                            "更新就绪，即将重启完成安装",
                                            "Update ready; restarting to install"
                                        ),
                                        info.version
                                    ),
                                );
                                if let Err(e) = crate::updater::install(&new_path) {
                                    supervisor::set_flash(&format!(
                                        "{}: {e}",
                                        crate::i18n::tr("更新安装失败", "Update install failed")
                                    ));
                                    let _ = proxy.send_event(UserEvent::Refresh);
                                    return;
                                }
                                supervisor::shutdown();
                                std::process::exit(0);
                            }
                            Err(e) => {
                                supervisor::log(&format!("更新失败: {e}"));
                                supervisor::set_flash(&format!(
                                    "{}: {e}",
                                    crate::i18n::tr("更新失败", "Update failed")
                                ));
                                let _ = proxy.send_event(UserEvent::Refresh);
                            }
                        }
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
            UserEvent::RefreshTray => self.rebuild_tray(),
        }
    }
}

impl App {
    /// 定时（1s）刷新：就地更新状态行文本、打开项可用态、复选框勾选；不重建菜单。
    /// 同时检测 Windows 模式深浅切换，动态换托盘图标（黑线↔白线，见 is_light_theme）。
    /// 只换图标、不重建菜单（菜单内容不变，重建会重置复选框/菜单 id，属冗余）。
    fn refresh(&mut self) {
        // 主线程心跳：每轮刷新标记活动（心跳线程据此检测主循环是否卡死）
        MAIN_ACTIVITY.store(unix_secs(), Ordering::Relaxed);

        // 主题切换检测：读注册表，变化时只调 set_icon 换图标
        let is_light = is_light_theme();
        if is_light != self.current_is_light {
            self.current_is_light = is_light;
            supervisor::log(&format!(
                "检测到 Windows 模式切换（is_light={is_light}），切换托盘图标"
            ));
            if let Some(tray) = &self.tray {
                let _ = tray.set_icon(Some(load_tray_icon()));
            }
        }

        let st = supervisor::status();
        let status_text = status_line(&st);
        self.items.status.set_text(&status_text);
        self.items.open.set_enabled(st.ready);
        // 同步复选框（配置可能被其他路径修改，保持菜单与持久化一致）
        self.items.exit_close.set_checked(config::load().exit_close_engine);
        // 更新项：发现新版本 → 「更新到 vX」（启用）；否则禁用占位
        match crate::updater::available() {
            Some(info) => {
                self.items.update.set_text(&format!(
                    "{} v{}",
                    crate::i18n::tr("更新到", "Update to"),
                    info.version
                ));
                self.items.update.set_enabled(true);
            }
            None => {
                self.items
                    .update
                    .set_text(crate::i18n::tr("暂无可用更新", "No update available"));
                self.items.update.set_enabled(false);
            }
        }
    }

    /// 重建托盘（图标/菜单整体重建）：恢复「幽灵图标」——Explorer 重启 / 睡眠唤醒等导致
    /// Shell_NotifyIcon 注册失效、图标可见但点击/右键无响应。主线程调用（user_event 分发）；
    /// 旧 TrayIcon drop 时向旧窗口发 NIM_DELETE（对已死的 Explorer 无害），新 TrayIcon
    /// 重新注册到当前 Explorer。
    fn rebuild_tray(&mut self) {
        let (menu, items) = build_ui();
        self.items = items;
        self.tray = build_tray(menu);
        self.current_is_light = is_light_theme();
        supervisor::log("托盘已重建（Explorer 重启检测或定时兜底）");
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

    let (menu, items) = build_ui();
    let mut app = App {
        url: url.to_string(),
        items,
        pending_menu: Some(menu),
        tray: None,
        proxy: app_proxy,
        current_is_light: is_light_theme(),
    };

    // 1s 定时刷新（状态行/菜单可用性；主题切换由 watcher 事件驱动，此处兜底）；
    // 每 TRAY_REBUILD_INTERVAL_SECS 发一次 RefreshTray（定时兜底重建托盘）
    {
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || {
            let mut ticks: u64 = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                ticks += 1;
                if ticks % TRAY_REBUILD_INTERVAL_SECS == 0 {
                    let _ = proxy.send_event(UserEvent::RefreshTray);
                }
                let _ = proxy.send_event(UserEvent::Refresh);
            }
        });
    }

    // 事件驱动：注册表 Personalize 键变化（主题切换）时立即刷新托盘图标
    spawn_theme_watcher(event_loop.create_proxy());

    // Explorer 重启检测：Shell_TrayWnd 的属主 PID 变化 → 重建托盘（根治幽灵图标）
    spawn_explorer_watcher(event_loop.create_proxy());

    // 主线程心跳：主循环疑似卡死时记日志 + 尝试重建托盘（诊断与自愈）
    spawn_main_heartbeat(event_loop.create_proxy());

    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("托盘事件循环错误: {e}");
    }
    Ok(())
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 当前 Explorer（任务栏宿主）PID：查 Shell_TrayWnd 顶层窗口的属主进程。
/// None = 任务栏窗口不存在（Explorer 崩溃/注销中/无桌面会话）。
#[cfg(target_os = "windows")]
fn explorer_pid() -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    let name: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: FindWindowW 标准调用，只读系统窗口表
    let hwnd = unsafe { FindWindowW(name.as_ptr(), std::ptr::null()) };
    if hwnd == 0 {
        return None;
    }
    let mut pid: u32 = 0;
    // SAFETY: hwnd 为刚查到的合法窗口句柄；pid 为输出缓冲区
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    (pid != 0).then_some(pid)
}

#[cfg(not(target_os = "windows"))]
fn explorer_pid() -> Option<u32> {
    None
}

/// 后台线程：每 2s 探测 Explorer 任务栏窗口的属主 PID；变化（Explorer 重启）→ 发
/// RefreshTray 重建托盘。覆盖「Explorer 崩溃→重建」（PID 短暂 None 后回来）与
/// 「PID 直接更换」两种情况，避免幽灵图标（图标可见但点击/菜单无响应）。
#[cfg(target_os = "windows")]
fn spawn_explorer_watcher(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let mut last: Option<u32> = explorer_pid();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let cur = explorer_pid();
            let restarted = match (last, cur) {
                (Some(prev), Some(now)) if prev != now => true,
                (None, Some(_)) => true, // Explorer 曾消失，回来即视为重启
                _ => false,
            };
            if restarted {
                supervisor::log(&format!(
                    "检测到 Explorer 重启（PID {last:?} → {cur:?}），重建托盘图标"
                ));
                let _ = proxy.send_event(UserEvent::RefreshTray);
            }
            last = cur;
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn spawn_explorer_watcher(_proxy: EventLoopProxy<UserEvent>) {}

/// 主线程心跳：主循环每 1s 经 refresh() 刷新 MAIN_ACTIVITY；超过 30s 未活动 →
/// 记日志（诊断「托盘点不出菜单是否主线程卡死」）并尝试发一次 RefreshTray 自愈。
fn spawn_main_heartbeat(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let mut warned = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let now = unix_secs();
            let last = MAIN_ACTIVITY.load(Ordering::Relaxed);
            if last != 0 && now.saturating_sub(last) > 30 {
                if !warned {
                    supervisor::log(&format!(
                        "⚠️ 主线程事件循环疑似卡死（{last} 后未活动）——托盘菜单可能无响应；已尝试重建托盘"
                    ));
                    warned = true;
                    // 每次卡死事件只尝试一次重建（主循环若真死，事件也发不出去；若只是
                    // 短暂卡顿，一次重建后活动恢复，避免反复重建）
                    let _ = proxy.send_event(UserEvent::RefreshTray);
                }
            } else {
                warned = false;
            }
        }
    });
}

/// 创建菜单项（一次性）并组装菜单，返回 (菜单, 持久项引用)。
fn build_ui() -> (Menu, MenuItems) {
    let st = supervisor::status();
    let status_text = status_line(&st);
    // 状态行：禁用项，仅展示
    let status_item = MenuItem::new(&status_text, false, None);
    // 「打开 dsh 界面」置顶（最常用）：打开引擎本体 UI（3080）
    let open_item = MenuItem::new(crate::i18n::tr("打开 dsh 界面", "Open dsh UI"), st.ready, None);
    // 「打开管理页」：打开 dsh-come 管理页（3081）
    let open_admin_item = MenuItem::new(crate::i18n::tr("打开管理页", "Open admin page"), true, None);
    let restart_item = MenuItem::new(crate::i18n::tr("重启引擎", "Restart engine"), true, None);
    // 「退出时关闭引擎」复选框（2026-08-21）：勾选=退出 dsh-come 时杀引擎（默认）；
    // 取消勾选=退出保留引擎运行。勾选状态持久化在 config.exit_close_engine。
    // 注意 CheckMenuItem::new 签名 = (text, enabled, checked, accelerator)。
    let exit_close_item = CheckMenuItem::new(
        crate::i18n::tr("退出时关闭引擎", "Close engine on exit"),
        true, // enabled：始终可点击
        config::load().exit_close_engine, // checked：随配置
        None,
    );
    let logs_item = MenuItem::new(crate::i18n::tr("打开日志目录", "Open log folder"), true, None);
    // 「检查更新」：手动检查（无视每日节流）；「更新到 vX」：有可用更新时启用
    let check_update_item = MenuItem::new(
        crate::i18n::tr("检查更新", "Check for updates"),
        true,
        None,
    );
    let update_item = MenuItem::new(
        crate::i18n::tr("暂无可用更新", "No update available"),
        false,
        None,
    );
    let quit_item = MenuItem::new(crate::i18n::tr("退出", "Exit"), true, None);

    let menu = Menu::new();
    let _ = menu.append(&open_item);
    let _ = menu.append(&open_admin_item);
    let _ = menu.append(&status_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&restart_item);
    let _ = menu.append(&exit_close_item);
    let _ = menu.append(&logs_item);
    let _ = menu.append(&check_update_item);
    let _ = menu.append(&update_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let ids = TrayIds {
        open: open_item.id().clone(),
        open_admin: open_admin_item.id().clone(),
        restart: restart_item.id().clone(),
        exit_close: exit_close_item.id().clone(),
        logs: logs_item.id().clone(),
        check_update: check_update_item.id().clone(),
        do_update: update_item.id().clone(),
        quit: quit_item.id().clone(),
    };
    // 只保留会被就地更新的项（状态行/打开项可用态/退出复选框/更新项）；
    // 其余项 append 进 menu 后由菜单自身持有，无需在此保存引用。
    let items = MenuItems {
        status: status_item,
        open: open_item,
        exit_close: exit_close_item,
        update: update_item,
        ids,
    };
    (menu, items)
}

fn status_line(st: &supervisor::SuperStatus) -> String {
    let ver = st
        .version
        .clone()
        .unwrap_or_else(|| crate::i18n::tr("系统 dsh", "system dsh").to_string());
    let body = if !st.stage.is_empty() {
        let mut s = st.stage.clone();
        if let Some(secs) = st.stage_elapsed {
            if secs >= 30 {
                if crate::i18n::is_en() {
                    s.push_str(&format!(" (elapsed {})", crate::supervisor::fmt_elapsed(secs)));
                } else {
                    s.push_str(&format!("（已 {}）", crate::supervisor::fmt_elapsed(secs)));
                }
            }
        }
        s
    } else if let Some(f) = supervisor::flash() {
        f
    } else if st.running {
        if st.ready {
            format!(
                "{} http://127.0.0.1:{}",
                crate::i18n::tr("运行中 ✓", "Running ✓"),
                st.port
            )
        } else {
            crate::i18n::tr("启动中…", "Starting…").to_string()
        }
    } else if let Some(e) = &st.last_error {
        format!("{}（{e}）", crate::i18n::tr("已停止 ✗", "Stopped ✗"))
    } else {
        crate::i18n::tr("已停止 ✗", "Stopped ✗").to_string()
    };
    format!(
        "{}｜{ver}｜{body}",
        crate::i18n::tr("DSH 伴侣", "DSH Companion")
    )
}

fn build_tray(menu: Menu) -> Option<tray_icon::TrayIcon> {
    let icon = load_tray_icon();
    match tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(crate::i18n::tr("DSH 伴侣", "DSH Companion"))
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

/// 加载托盘图标（随 Windows 模式切换，见 `is_light_theme`）：
/// - 浅色：4 条黑色竖线（透明底）→ 浅色任务栏上清晰可见
/// - 深色：4 条白色竖线（透明底）→ 深色任务栏上清晰可见
/// 按托盘实际显示尺寸原生生成（避免 Windows 把图标缩小时比例失真）。
fn load_tray_icon() -> tray_icon::Icon {
    let is_light = is_light_theme();
    let size = tray_icon_size();
    supervisor::log(&format!(
        "托盘图标加载：{}（{}x{}，is_light={is_light}）",
        if is_light { "4条黑线" } else { "4条白线" },
        size, size
    ));
    gen_tray_icon(is_light, size)
}

/// 查询 Windows 小图标尺寸（托盘图标在当前 DPI 下的实际显示像素）。
/// 96 DPI → 16px；120 DPI → 20px；144 DPI → 24px。查询失败回退 16。
#[cfg(target_os = "windows")]
fn tray_icon_size() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    // SAFETY: 纯指标查询，无副作用
    let s = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if s > 0 { s as u32 } else { 16 }
}

#[cfg(not(target_os = "windows"))]
fn tray_icon_size() -> u32 {
    16
}

/// 计算 4 条竖线的几何：(线宽, 4 条线起始 x)。
/// 设计（2026-08-29 修正）：4 条线在图标内**居中、四周留白 1px、不贴边**，
/// 避免首尾两条线紧贴图标边缘、在 16px 托盘下被感知为「图标边框」而数成 3 条。
/// 线宽最大化 + 间距固定 2px（2026-08-29 二次修正）：
/// 由 `4*line + 3*2 = area` 得 `line = (area-6)/4`，使**3 个间距严格相等（2px）**，
/// 且线宽随尺寸加粗：16px→2、20px→3、24px→4、28px→5、32px→6。
fn tray_line_geometry(size: u32) -> (u32, [u32; 4]) {
    let pad = 1u32;              // 四周留白（不贴边）
    let area = size.saturating_sub(2 * pad);
    // 线宽最大化；area<10（size<12）时退化为 area/4
    let line = if area >= 10 {
        (area - 6) / 4
    } else {
        (area / 4).max(1)
    };
    let total = 4 * line;
    let gap = area.saturating_sub(total) / 3;
    let extra = area.saturating_sub(total) % 3;

    // 4 条线的起始 x，均匀分布在 [pad, size-pad]，余数补到前几个间隙
    let mut starts = [0u32; 4];
    let mut x = pad;
    for i in 0u32..4 {
        starts[i as usize] = x;
        x += line;
        if i < 3 {
            x += gap + if i < extra { 1 } else { 0 };
        }
    }
    (line, starts)
}

/// 按指定尺寸生成托盘图标：透明背景 + 4 条竖直条纹（跟随系统明暗切换）。
/// - is_light=true（浅色任务栏）：4 条黑色竖线
/// - is_light=false（深色任务栏）：4 条白色竖线
/// 背景透明（无白/黑底贴纸感），线条颜色随系统明暗反转。
fn gen_tray_icon(is_light: bool, size: u32) -> tray_icon::Icon {
    let size = size.max(8);
    let (line, line_starts) = tray_line_geometry(size);

    // 线条颜色：浅色=黑，深色=白；其余像素保持全透明 (0,0,0,0)
    let fg: [u8; 4] = if is_light {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    };

    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            if line_starts.iter().any(|&s| x >= s && x < s + line) {
                rgba[i..i + 4].copy_from_slice(&fg);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, size, size).expect("生成托盘图标失败")
}

/// 检测 Windows 当前是否为暗色模式（决定托盘图标：深色→白线，浅色→黑线）。
/// 同时读两个注册表值，**任一为深色(0) 即视为暗色模式**：
/// - `SystemUsesLightTheme`：Windows 模式（任务栏/系统 UI）
/// - `AppsUseLightTheme`：应用模式
/// 读取失败默认浅色（兼容旧版 Windows）。
#[cfg(target_os = "windows")]
fn is_light_theme() -> bool {
    use std::ptr;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_DWORD,
    };

    let sub_key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();

    // 读单个 DWORD 注册表值，返回 (是否读取成功, 值是否为浅色/非0)
    fn read_light_flag(hkey: HKEY, name: &str) -> Option<bool> {
        let value_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut data: u32 = 0;
        let mut data_size = std::mem::size_of::<u32>() as u32;
        let mut dtype: u32 = 0;
        // SAFETY: 标准 RegQueryValueExW 调用，value_name 以 NUL 结尾，data 为合法 u32 缓冲区
        let ok = unsafe {
            RegQueryValueExW(
                hkey,
                value_name.as_ptr(),
                ptr::null(),
                &mut dtype,
                &mut data as *mut u32 as *mut u8,
                &mut data_size,
            )
        } == 0;
        if ok && dtype == REG_DWORD {
            Some(data != 0)
        } else {
            None
        }
    }

    unsafe {
        let mut hkey: HKEY = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub_key.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return true;
        }
        let system = read_light_flag(hkey, "SystemUsesLightTheme");
        let apps = read_light_flag(hkey, "AppsUseLightTheme");
        RegCloseKey(hkey);
        // 任一为深色(0) → 非浅色 → false（深色图标）。两个都读不到才默认浅色。
        match (system, apps) {
            (Some(s), Some(a)) => s && a,
            (Some(s), None) => s,
            (None, Some(a)) => a,
            (None, None) => true,
        }
    }
}

/// 非 Windows 主题探测（决定托盘图标黑线/白线）：
/// - macOS：`defaults read -g AppleInterfaceStyle` → `Dark` → 深色。无需 objc FFI。
/// - Linux：`gsettings get org.gnome.desktop.interface gtk-theme` 命中 `dark` 关键词；
///   再退 `GTK_THEME` 环境变量（轻量桌面/无 gsettings 时的唯一信号）。
/// 读不到 → 回落浅色（true），与 Windows 分支的默认一致。
#[cfg(not(target_os = "windows"))]
fn is_light_theme() -> bool {
    fn output_contains(args: &[&str], keyword: &str) -> Option<bool> {
        let out = std::process::Command::new(args[0]).args(&args[1..]).output().ok()?;
        if !out.status.success() {
            return None; // 命令存在但查询失败（如 gsettings 无此键）→ 不算命中
        }
        let s = String::from_utf8_lossy(&out.stdout);
        Some(s.to_ascii_lowercase().contains(keyword))
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(dark) = output_contains(&["defaults", "read", "-g", "AppleInterfaceStyle"], "dark") {
            return !dark; // "Dark" → 深色 → false
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(dark) = output_contains(
            &["gsettings", "get", "org.gnome.desktop.interface", "gtk-theme"],
            "dark",
        ) {
            return !dark;
        }
        if let Ok(theme) = std::env::var("GTK_THEME") {
            if theme.to_ascii_lowercase().contains("dark") {
                return false;
            }
        }
    }
    true
}

/// 后台线程：监听注册表 Personalize 键变化，主题切换时立即发 Refresh 事件。
/// 配合 1s 定时刷新（状态行）兜底，确保不遗漏。
#[cfg(target_os = "windows")]
fn spawn_theme_watcher(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            CreateEventW, WaitForSingleObject, INFINITE,
        };
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
            KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
        };

        let sub_key: Vec<u16> =
            "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
                .encode_utf16()
                .collect();

        unsafe {
            let mut hkey: HKEY = 0;
            if RegOpenKeyExW(HKEY_CURRENT_USER, sub_key.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
                return;
            }
            loop {
                // 创建自动重置事件（bManualReset=FALSE, bInitialState=FALSE）
                let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
                if event == 0 {
                    break;
                }
                // 异步监听：键内任意值变化时信号 event
                // SAFETY: hkey 已打开，event 是合法句柄
                let ok = RegNotifyChangeKeyValue(
                    hkey,
                    0,             // bWatchSubtree=FALSE
                    REG_NOTIFY_CHANGE_LAST_SET,
                    event,
                    1,             // fAsynchronous=TRUE
                );
                if ok != 0 {
                    CloseHandle(event);
                    break;
                }
                // 阻塞等待变化（INFINITE）
                WaitForSingleObject(event, INFINITE);
                CloseHandle(event);
                // 通知主线程刷新（refresh 内部会比对主题是否真的变了）
                let _ = proxy.send_event(UserEvent::Refresh);
            }
            RegCloseKey(hkey);
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn spawn_theme_watcher(_proxy: EventLoopProxy<UserEvent>) {}

pub fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        crate::supervisor::hide_window(&mut cmd); // 防弹 cmd 黑框
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_generates_both_themes() {
        // 两套主题都能生成有效图标
        let _dark = gen_tray_icon(false, 16);
        let _light = gen_tray_icon(true, 16);
        let _ = load_tray_icon();
    }

    #[test]
    fn tray_icon_four_lines_visible_and_centered() {
        // 关键回归：4 条竖线都必须可见（不贴边），浅色/深色共用同一几何。
        // 16px: line=2, gap=[2,2,2] → 线起始 [1,5,9,13]，左右各留 1px
        // 20px: line=2, 24px: line=3, 32px: line=4
        for &size in &[16u32, 20, 24, 28, 32] {
            let (line, starts) = tray_line_geometry(size);
            // 恰好 4 条线
            assert_eq!(starts.len(), 4, "size={size} 应有 4 条竖线");
            // 线宽至少 2px
            assert!(line >= 2, "size={size} 线宽应 >=2，实际 {line}");
            // 首条线不贴左边缘（x>=1）
            assert!(starts[0] >= 1, "size={size} 首条线不应贴左边缘");
            // 末条线不贴右边缘（starts[3]+line <= size-1）
            assert!(
                starts[3] + line <= size - 1,
                "size={size} 末条线不应贴右边缘（{}+{}>{})",
                starts[3],
                line,
                size
            );
            // 线之间不重叠、顺序正确
            for i in 0..3 {
                assert!(
                    starts[i] + line < starts[i + 1],
                    "size={size} 线 {i} 与 {} 重叠",
                    i + 1
                );
            }
        }
        // 16px 精确值回归（100% 缩放）
        let (line, starts) = tray_line_geometry(16);
        assert_eq!(line, 2);
        assert_eq!(starts, [1, 5, 9, 13]);
        // 28px 精确值回归（175% DPI）：线宽 5px、间距 2px 严格一致
        let (line, starts) = tray_line_geometry(28);
        assert_eq!(line, 5);
        assert_eq!(starts, [1, 8, 15, 22]);
    }
}
