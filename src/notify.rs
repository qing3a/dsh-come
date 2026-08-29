//! 桌面通知（Windows 走 WinRT toast，失败静默降级，不阻塞守护）。
//! 仅用于「引擎崩溃/重启/达上限」等关键事件提示——通知是锦上添花，绝不能影响守护逻辑。

/// 弹出系统通知；任何失败都静默吞掉（调用方不依赖返回值）。
/// Windows 上 toast 需要 AppUserModelID 才能保证弹出；未注册 AUMID 时可能不显示
/// （退回通知中心），属已知的 Win10+ 平台限制，不影响功能。
pub fn toast(title: &str, body: &str) {
    #[cfg(target_os = "windows")]
    {
        use notify_rust::Notification;
        let _ = Notification::new()
            .app_id("dsh-come")
            .summary(title)
            .body(body)
            .timeout(notify_rust::Timeout::Milliseconds(8000))
            .show();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // notify-rust 在 Unix 上同样是 summary（Linux 走 libnotify、macOS 走 osascript），
        // 与 Windows 一致；失败静默降级（通知是锦上添花，不阻塞守护）。
        use notify_rust::Notification;
        let _ = Notification::new()
            .summary(title)
            .body(body)
            .show();
    }
}
