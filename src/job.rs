//! Windows Job Object 作业对象：把 dsh 引擎整树纳入作业，给「进程外 supervisor」补齐最后一块拼图。
//!
//! 解决的真实脆弱点（见 supervisor.rs / main.rs 的崩溃模式）：
//! 1. **守护进程崩溃 → dsh 变孤儿占端口**：`KILL_ON_JOB_CLOSE` 让 dsh-come（父进程）退出/崩溃时，
//!    OS 强制杀掉作业内整树（cmd → dsh.cmd → node 全杀），不再留下占 3080 的孤儿 dsh。
//! 2. **Windows 进程树杀不可靠**：`taskkill /T` 对脱离的孙进程会漏杀；`terminate_job()` 由 OS 一次性
//!    杀掉作业内全部进程，比 `taskkill /T` 可靠。
//!
//! 仅 Windows 编译；非 Windows 提供空实现（return false / None），调用方回退 `taskkill`。
//!
//! 注意：作业句柄**故意不 `CloseHandle`**——句柄存活期间整树受控，进程退出时 OS 关句柄即触发
//! `KILL_ON_JOB_CLOSE`。若主动 CloseHandle 反而会在正常运行中误杀引擎。

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    // windows-sys 0.52 的 HANDLE 即 isize 别名；用 0 表示「未创建/失败」。
    /// 进程级作业句柄（整树受 KILL_ON_JOB_CLOSE 约束），存活到 dsh-come 进程退出。
    static JOB: OnceLock<HANDLE> = OnceLock::new();

    /// 创建带 `KILL_ON_JOB_CLOSE` 的作业；失败返回 0（NULL，调用方降级 taskkill）。
    fn create() -> HANDLE {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h == 0 {
                return 0;
            }
            // 全零初始化（POD 结构），仅打开「父进程退出即杀整树」开关。
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return 0;
            }
            // 故意不 CloseHandle：见模块文档说明。
            h
        }
    }

    /// 创建或取已存在的作业句柄（幂等）；未创建成功返回 None。
    pub fn ensure_job() -> Option<HANDLE> {
        let h = *JOB.get_or_init(create);
        if h == 0 {
            None
        } else {
            Some(h)
        }
    }

    /// 把刚 spawn 的 immediate 子进程（cmd.exe）纳入作业；其后代（dsh.cmd → node）默认继承作业，
    /// 整树一并受 `KILL_ON_JOB_CLOSE` 约束。失败返回 false（调用方降级 taskkill）。
    ///
    /// 必须在 spawn 后尽快调用——dsh 启动链前几百毫秒内 node 尚未拉起，此时 assign 可覆盖整树；
    /// 即使个别已存在的孙进程未被纳入，停止时 `terminate_job` 也会因作业内进程退出而结束，
    /// 且命令行强杀路径（dsh-come 崩溃）由 OS 兜底。
    pub fn assign_child(pid: u32) -> bool {
        let Some(h) = ensure_job() else { return false };
        unsafe {
            let hp = OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION,
                0,
                pid,
            );
            if hp == 0 {
                return false;
            }
            let ok = AssignProcessToJobObject(h, hp);
            CloseHandle(hp);
            ok != 0
        }
    }

    /// 杀掉作业内全部进程（stop/重启用），比 `taskkill /T` 可靠（不漏杀已脱离的孙进程）。
    pub fn terminate_job() -> bool {
        let Some(h) = JOB.get() else { return false };
        unsafe { TerminateJobObject(*h, 1) != 0 }
    }
}

#[cfg(target_os = "windows")]
pub use imp::*;

// ---------- 非 Windows 占位（调用方回退 taskkill） ----------

#[cfg(not(target_os = "windows"))]
pub fn ensure_job() -> Option<()> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn assign_child(_pid: u32) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn terminate_job() -> bool {
    false
}
