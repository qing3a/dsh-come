//! dsh 纯净卸载：只卸载系统 dsh（npm 全局包），可选清理其数据/残留 shim。
//! 壳（dsh-come 自身）不在卸载范围——用户保留壳，只是不要 dsh 了。
//!
//! 流程（幂等，全部同步）：
//!   1. 停引擎（supervisor::stop()，防监测线程/Job Object 把引擎拉回）；
//!   2. `npm uninstall -g @deepseek-ai/dsh`（用与 dsh 同目录的 npm，保证落到 PATH 解析的那个全局目录）；
//!   3. 可选清 `%USERPROFILE%\.dsh` 数据目录（keep_data=false，默认保留——里面是凭据/配置/工作台数据）；
//!   4. 可选清 PATH 残留 shim（clean_shim=true，默认关——删另一套 node 全局目录里的 dsh*，危险项）。
//! 装完失效探测/版本缓存（invalidate_cache），返回 JSON 报告。

use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallReport {
    /// 是否成功执行了 npm uninstall（dsh 未安装也算成功——幂等）
    pub ok: bool,
    /// 人性化结果文案
    pub msg: String,
    /// 卸载前是否正在运行（调用方已停掉）
    pub was_running: bool,
    /// npm 全局目录（uninstall 的落点）
    pub npm_prefix: Option<String>,
    /// 数据目录 %USERPROFILE%\.dsh 是否被清除
    pub data_cleared: bool,
    /// PATH 残留 shim 是否被清除
    pub shim_cleared: bool,
    /// 卸载后 dsh 是否仍在 PATH 可解析（应 false）
    pub still_installed: bool,
    /// 各步骤明细（含 tail 输出），便于 UI 展示
    pub steps: Vec<String>,
}

/// 执行纯净卸载。keep_data=false → 连数据目录一起删；clean_shim=true → 连 PATH 残留 shim 一起删。
/// 返回报告（不抛错——局部失败也尽量收进 steps，让用户看到完整情况）。
pub fn uninstall_dsh(keep_data: bool, clean_shim: bool) -> UninstallReport {
    let mut report = UninstallReport {
        ok: false,
        msg: String::new(),
        was_running: crate::supervisor::status().running,
        npm_prefix: None,
        data_cleared: false,
        shim_cleared: false,
        still_installed: true,
        steps: Vec::new(),
    };

    // 1. 停引擎（幂等：没跑也没关系）。auto_restart 置 false，监测线程不会把它拉回。
    report.steps.push("停止 dsh 引擎…".to_string());
    if report.was_running {
        match crate::supervisor::stop() {
            Ok(()) => report.steps.push("  已停止 dsh 引擎".to_string()),
            Err(e) => report.steps.push(format!("  停止引擎失败（继续卸载）：{e}")),
        }
    } else {
        report.steps.push("  dsh 引擎未在运行".to_string());
    }

    // 2. npm uninstall -g。用与 dsh 同目录的 npm，确保卸载的就是 PATH 解析到的那个全局包。
    //    npm 不存在 → 视为 dsh 也不可能由 npm 装（幂等空操作，仍算成功）。
    report.steps.push("卸载 npm 全局包 @deepseek-ai/dsh…".to_string());
    let installed_before = crate::installer::dsh_installed();
    if !installed_before {
        report.steps.push("  dsh 未安装（无需卸载）".to_string());
        report.ok = true;
        report.msg = "dsh 未安装".to_string();
        report.still_installed = false;
        report.npm_prefix = crate::installer::npm_prefix().map(|p| p.display().to_string());
        finish(&mut report);
        return report;
    }
    let npm = crate::installer::npm_for_dsh().or_else(|| {
        // npm_for_dsh 是「与 dsh 同目录的 npm」；找不到时退回 PATH 里任意 npm
        crate::installer::which("npm")
    });
    let Some(npm) = npm else {
        report.steps.push("  未找到 npm 命令，无法卸载（请先安装 Node.js 或手动执行 npm uninstall -g @deepseek-ai/dsh）".to_string());
        report.msg = "卸载失败：未找到 npm".to_string();
        report.ok = false;
        finish(&mut report);
        return report;
    };
    report.npm_prefix = crate::installer::npm_prefix().map(|p| p.display().to_string());
    let mut cmd = Command::new(&npm);
    cmd.args(["uninstall", "-g", "@deepseek-ai/dsh"]);
    crate::supervisor::hide_window(&mut cmd);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            report.steps.push(format!("  无法启动 npm: {e}"));
            report.msg = format!("卸载失败：无法启动 npm（{}）", npm.display());
            finish(&mut report);
            return report;
        }
    };
    let tail = crate::installer::tail_text(&out.stdout, &out.stderr);
    if !out.status.success() {
        report.steps.push(format!("  npm uninstall 退出码 {:?}。{tail}", out.status.code()));
        report.msg = format!("卸载失败（退出码 {:?}）。{tail}", out.status.code());
        report.ok = false;
        finish(&mut report);
        return report;
    }
    // npm 退出成功，但需验证 npm 全局包是否真的删掉了（npm 偶发 EPERM 部分失败但退出码 0——
    // 2026-08-23 实测：依赖被占用导致 uninstall 只删了部分，退出码仍 0，PATH 里 dsh 还能解析）。
    let pkg_dir = crate::installer::npm_prefix()
        .map(|p| p.join("node_modules").join("@deepseek-ai").join("dsh"));
    let pkg_gone = pkg_dir
        .as_ref()
        .map(|p| !p.exists())
        .unwrap_or(true);
    report.steps.push(format!("  npm uninstall 完成。{tail}"));
    if !pkg_gone {
        report.steps.push("  ⚠️ npm 全局包 @deepseek-ai/dsh 仍存在（可能被占用导致卸载不完整），请关闭占用 dsh 的进程后重试。".to_string());
        report.msg = format!("dsh 卸载不完整：npm 全局包仍存在（{tail}）").to_string();
        report.ok = false;
        finish(&mut report);
        return report;
    }
    report.ok = true;

    // 3. 可选清数据目录（%USERPROFILE%\.dsh，除非 DSH_HOME 另有设置）。
    let home = crate::runtime::system_home_dir();
    if !keep_data {
        report.steps.push(format!("清除数据目录 {}…", home.display()));
        match remove_all(&home) {
            Ok(()) => {
                report.data_cleared = true;
                report.steps.push("  已清除数据目录".to_string());
            }
            Err(e) => report.steps.push(format!("  清除数据目录失败（跳过）：{e}")),
        }
    } else {
        report.steps.push(format!("保留数据目录 {}（keep_data）", home.display()));
    }

    // 4. 可选清 PATH 残留 shim（另一套 node 全局目录里的 dsh*——npm uninstall 只清 npm 全局那份）。
    //    默认关：这属于「危险项」，删的是 C:\tools\nodejs 这类目录里的文件，误删风险高。
    if clean_shim {
        report.steps.push("清除 PATH 残留 dsh shim…".to_string());
        match clean_shims() {
            Ok(n) => {
                if n > 0 {
                    report.shim_cleared = true;
                    report.steps.push(format!("  已删除 {n} 个残留 shim"));
                } else {
                    report.steps.push("  未发现残留 shim".to_string());
                }
            }
            Err(e) => report.steps.push(format!("  清理 shim 失败（跳过）：{e}")),
        }
    } else {
        report.steps.push("保留 PATH 残留 shim（clean_shim=false）".to_string());
    }

    finish(&mut report);
    report
}

/// 收尾：失效缓存 + 重探测 + 汇总文案。
fn finish(report: &mut UninstallReport) {
    crate::installer::invalidate_cache();
    report.still_installed = crate::installer::dsh_installed();
    if report.ok {
        if report.still_installed {
            report.msg = format!(
                "dsh 已从 npm 卸载，但 PATH 仍可解析到 dsh（{}）。残留 shim 未清理（保持默认），如需完全清掉请用 --clean-shim。",
                crate::installer::which("dsh").map(|p| p.display().to_string()).unwrap_or_default()
            );
        } else {
            report.msg = "dsh 已完全卸载".to_string();
        }
    }
    if report.data_cleared {
        report.msg.push_str("，数据目录已清除");
    } else if !report.ok && report.still_installed {
        report.msg.push_str("，dsh 仍可用（卸载未完成）");
    }
}

/// 递归删除目录/文件（尽力而为，单个失败继续；只删目标本身，不删父目录）。
fn remove_all(p: &std::path::Path) -> std::io::Result<()> {
    if !p.exists() {
        return Ok(());
    }
    // 只删目标本身（.dsh 目录），不删其父级
    if p.is_dir() {
        std::fs::remove_dir_all(p)?;
    } else {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

/// 清 PATH 残留 shim：在合并 PATH 目录（进程 PATH + 注册表 PATH + npm 全局）里找
/// dsh / dsh.cmd / dsh.ps1 / dsh.exe / dsh.bat，但**跳过 npm 全局目录**（那是 npm uninstall
/// 已经清过的、也是合法落点）。返回删除的文件数。只删文件名精确匹配 dsh 的 shim，
/// 不碰 dsh-market 等其他可执行。
fn clean_shims() -> Result<usize, String> {
    let npm_dir = crate::installer::npm_prefix();
    let mut removed = 0usize;
    for dir in crate::installer::env_dirs() {
        if let Some(n) = &npm_dir {
            if same_path(&dir, n) {
                continue;
            }
        }
        for name in ["dsh", "dsh.cmd", "dsh.ps1", "dsh.exe", "dsh.bat"] {
            let p = dir.join(name);
            if p.is_file() {
                let _ = std::fs::remove_file(&p); // 单个删不掉继续（被占用/无权限），整体不失败
                if !p.exists() {
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

/// 判断两个路径是否指向同一位置（先 canonicalize 消除 `..`/尾分隔符/符号链接差异）。
///
/// **大小写敏感性必须分平台**，这是本函数唯一的复杂点：
/// - Windows（NTFS）文件系统大小写不敏感 → 比较时忽略大小写，否则 `C:\Users`
///   与 `C:\users` 会被判成两个路径。
/// - Unix（ext4 / APFS 默认）大小写**敏感** → 必须严格比较。沿用 `to_lowercase()`
///   会把 `/usr/local/bin/Dsh` 与 `/usr/local/bin/dsh` 误判为同一路径；
///   在 `clean_shims` 里表现为：真实的 shim 目录被误认成 npm 全局目录而 `continue` 跳过，
///   **该清理的残留 shim 就留在了 PATH 上**。
fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    #[cfg(target_os = "windows")]
    {
        ca.to_string_lossy().to_lowercase() == cb.to_string_lossy().to_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        // PathBuf 的 == 是分组件比较，比字符串比较更严格也更正确
        ca == cb
    }
}

/// 供管理页/CLI 调用的同步卸载（带线程 panic 兜底；npm 卡住时由调用方所在线程负责超时，
/// 这里不额外设超时——uninstall 正常几秒内返回）。
pub fn run_uninstall(keep_data: bool, clean_shim: bool) -> UninstallReport {
    let handle = std::thread::spawn(move || uninstall_dsh(keep_data, clean_shim));
    match handle.join() {
        Ok(r) => r,
        Err(_) => {
            // 线程 panic → 当作失败返回（不阻塞管理页线程）
            UninstallReport {
                ok: false,
                msg: "卸载线程异常退出".to_string(),
                was_running: crate::supervisor::status().running,
                npm_prefix: None,
                data_cleared: false,
                shim_cleared: false,
                still_installed: crate::installer::dsh_installed(),
                steps: vec!["卸载线程异常退出".to_string()],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// same_path：不同写法/分隔符的同一路径视为相同（用于跳过 npm 全局目录）。
    #[test]
    fn same_path_ignores_sep() {
        // 用真实存在的目录：Windows 上 canonicalize 会规范化分隔符，
        // 但带不带尾斜杠、/ vs \ 应判为同一路径。
        let dir = std::env::temp_dir();
        let with_trailing = {
            let mut s = dir.to_string_lossy().to_string();
            if !s.ends_with(std::path::MAIN_SEPARATOR) {
                s.push(std::path::MAIN_SEPARATOR);
            }
            std::path::PathBuf::from(s)
        };
        // temp_dir 一定存在，canonicalize 会成功并去掉尾分隔符差异 → 应相等
        assert!(same_path(&dir, &with_trailing));
    }

    /// same_path：不存在/无法 canonicalize 的路径退回原始比较（不会 panic）。
    #[test]
    fn same_path_missing_falls_back() {
        let a = std::path::Path::new("C:\\nonexistent\\dsh-dir");
        let b = std::path::Path::new("C:\\nonexistent\\dsh-dir");
        assert!(same_path(a, b));
        let c = std::path::Path::new("C:\\nonexistent\\other");
        assert!(!same_path(a, c));
    }

    /// Unix 大小写敏感：仅大小写不同的路径必须判为**不同**。
    /// 修复前两边都 to_lowercase 后相等 → clean_shims 会误把待清理目录认成
    /// npm 全局目录而跳过，残留 shim 就留在了 PATH 上。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn same_path_is_case_sensitive_on_unix() {
        let a = std::path::Path::new("/usr/local/bin/Dsh");
        let b = std::path::Path::new("/usr/local/bin/dsh");
        assert!(!same_path(a, b), "Unix 下大小写不同的路径必须判为不同");
        // 完全一致仍应判等（防止改过头）
        assert!(same_path(a, a));
    }

    /// Windows 大小写不敏感（NTFS）：仅大小写不同的路径视为同一路径。
    #[cfg(target_os = "windows")]
    #[test]
    fn same_path_is_case_insensitive_on_windows() {
        let a = std::path::Path::new("C:\\Users");
        let b = std::path::Path::new("C:\\users");
        assert!(same_path(a, b), "Windows 下应忽略大小写");
    }
}
