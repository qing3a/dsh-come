//! 自动更新（方向 v4 P0，2026-08-27）：GitHub Releases 分发，**无代码签名**。
//!
//! 流程（**询问制**，不静默安装——沿用项目历史习惯）：
//! - `check()`：GET `releases/latest/download/update-{win|macos|linux}.json`（按平台取清单，
//!   见 release.yml 矩阵构建）→ 版本比较 → 有新版本则记录到
//!   AVAILABLE（托盘菜单「更新到 vX」据此出现）。启动时静默检查，**每日最多一次**
//!   （root/update-state.json 记 last_check；托盘「检查更新」force 无视节流）。
//! - `download_and_verify()`：下载到 `<exe>.new`（Windows 为 `<exe>.exe.new`），SHA256 与
//!   update-*.json 比对（sha2）。
//! - `install()`：写**平台化**换装脚本并拉起——Windows 用 `.ps1`（UTF-8 BOM，中文任务名
//!   不乱码），Unix 用 `.sh`（POSIX）。脚本负责：等本进程退出 → 校验 → 备份 `.bak`
//!   → 替换 → 重启新版本 → 自删；换装窗口期先停系统级看门狗（Windows schtasks／macOS
//!   launchd／Linux systemd，注册脚本见 scripts/install-watchdog.*），换完再拉起——
//!   否则 KeepAlive / Restart=always 会把旧版本立刻拉起来，pid 永不退出、换装死等。
//!   调用方返回后应立即退出（supervisor::shutdown + exit），脚本接管换装。
//!
//! ⚠️ 换装是**异步**的：本进程在脚本启动后即退出，无法回传脚本结果。因此
//! `install()` 只保证「脚本已成功启动」，并把能在启动前发现的问题（新文件缺失/为空、
//! 目标目录不可写）全部前移为显式错误；脚本内部的失败写入 `swap-update.log` 供排查。
//! 这不是偷懒——父进程已不存在，返回值无处可去，落盘留痕是唯一可行的失败可见化。
//!
//! 安全模型（无签名）：HTTPS + update.json 内 sha256 比对，防下载损坏/中间人篡改；
//! 防不了「GitHub 账号被攻破」——个人项目接受该风险（与 yt-dlp 等无签名分发一致）。
//! 未来企业包需要 SmartScreen 免警告时再评估 Azure Trusted Signing（约 $10/月）。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::Digest;

/// 更新清单（发布流水线生成的 update.json 资产，见 .github/workflows/release.yml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// 当前版本（Cargo.toml version；发布纪律：发版时 bump 并与 tag 一致）
pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 版本比较（纯函数，可测）：取 '-' 前的数字段逐位比（0.2.0 > 0.1.9；rc 后缀忽略）。
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let nums = |s: &str| -> Vec<u64> {
        s.split('-')
            .next()
            .unwrap_or(s)
            .split('.')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let (na, nb) = (nums(a), nums(b));
    for i in 0..na.len().max(nb.len()) {
        let (x, y) = (na.get(i).copied().unwrap_or(0), nb.get(i).copied().unwrap_or(0));
        match x.cmp(&y) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

/// 平台后缀：更新清单与发布资产按平台命名（update-win / update-macos / update-linux，
/// release.yml 矩阵构建）。旧版 Windows（<0.3）读无后缀 update.json，已附带兼容清单。
fn platform_suffix() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "win"
    }
}

// ---------- 最新已知更新（托盘「更新到 vX」菜单据此显示） ----------

static AVAILABLE: OnceLock<Mutex<Option<UpdateInfo>>> = OnceLock::new();

pub fn set_available(info: Option<UpdateInfo>) {
    if let Ok(mut g) = AVAILABLE.get_or_init(|| Mutex::new(None)).lock() {
        *g = info;
    }
}

pub fn available() -> Option<UpdateInfo> {
    AVAILABLE
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.clone())
}

// ---------- 检查（节流） ----------

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn update_state_path() -> PathBuf {
    crate::runtime::root_dir().join("update-state.json")
}

fn last_check_ts() -> u64 {
    std::fs::read_to_string(update_state_path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["last_check"].as_u64())
        .unwrap_or(0)
}

fn touch_check() {
    if let Ok(s) = serde_json::to_string(&serde_json::json!({ "last_check": now_ts() })) {
        let _ = std::fs::write(update_state_path(), s);
    }
}

/// 检查更新：force=false 时每日最多自动检查一次（上次结果返回）；有新版本 → 记录到
/// AVAILABLE 并返回。失败返回 Err（网络/解析），调用方自行提示，不阻塞守护。
pub fn check(force: bool) -> Result<Option<UpdateInfo>, String> {
    if !force {
        let last = last_check_ts();
        if last != 0 && now_ts().saturating_sub(last) < 24 * 3600 {
            return Ok(available()); // 今日已查过：返回上次结果（可能 None）
        }
    }
    touch_check();
    let url = format!(
        "https://github.com/qing3a/dsh-come/releases/latest/download/update-{}.json",
        platform_suffix()
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("检查更新失败（网络）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("检查更新失败（HTTP {}）", resp.status()));
    }
    let info: UpdateInfo = resp
        .json()
        .map_err(|e| format!("更新清单解析失败: {e}"))?;
    let newer = compare_versions(&info.version, &current_version()) == Ordering::Greater;
    let result = newer.then_some(info);
    set_available(result.clone());
    Ok(result)
}

// ---------- 下载 + 校验 ----------

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 下载并校验新版本到 `<exe>.new.exe`；返回新文件路径。校验失败删除残留并报错。
pub fn download_and_verify(info: &UpdateInfo) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前 exe: {e}"))?;
    // Unix 产物无 .exe 扩展名（dsh-come → dsh-come.new）；Windows 保持 dsh-come.exe.new
    #[cfg(target_os = "windows")]
    let new_path = exe.with_extension("exe.new");
    #[cfg(not(target_os = "windows"))]
    let new_path = exe.with_extension("new");
    let _ = std::fs::remove_file(&new_path); // 清上次失败的半成品
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;
    let mut resp = client
        .get(&info.url)
        .send()
        .map_err(|e| format!("下载失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败（HTTP {}）", resp.status()));
    }
    let mut bytes = Vec::new();
    resp.copy_to(&mut bytes)
        .map_err(|e| format!("下载中断: {e}"))?;
    let hash = hex_lower(&sha2::Sha256::digest(&bytes));
    if !hash.eq_ignore_ascii_case(&info.sha256) {
        let _ = std::fs::remove_file(&new_path);
        return Err(format!("校验失败：期望 {}，实际 {}", info.sha256, hash));
    }
    std::fs::write(&new_path, bytes).map_err(|e| format!("写入 {} 失败: {e}", new_path.display()))?;
    Ok(new_path)
}

// ---------- 安装（换装脚本接管） ----------

/// POSIX sh 单引号包裹：单引号内不做任何展开，路径中的 `'` 用 `'\''` 闭合再开（标准转义）。
/// 不转义的话，含空格或特殊字符的路径会被 shell 拆成多个参数，换装必然失败。
#[cfg(not(target_os = "windows"))]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Unix 换装脚本（POSIX sh），与 Windows 版同构：
/// 停看门狗 → 等本进程退出 → 前置校验 → 备份 `.bak` → 替换 → 恢复看门狗 → 拉起 → 自删。
///
/// 平台差异是**有意为之**：
/// 1. **必须停看门狗**：macOS launchd `KeepAlive` / Linux systemd `Restart=always` 会在
///    主进程退出瞬间把旧版本拉起来 → pid 永不退出、换装死等 30s 超时。停/复命令由
///    `watchdog_control()` 按平台给出（对应 scripts/install-watchdog.sh 注册的 unit）。
///    看门狗操作失败不阻断换装（无看门狗的环境也允许更新），仅留日志。
/// 2. **必须补 `chmod +x`**：下载得到的文件不保证带可执行位，`mv` 保留的是新文件自身的权限。
///
/// 失败写入 `swap-update.log`：脚本异步执行（父进程随即退出让出 exe 文件），
/// 无法把错误码回传给已经不存在的进程，只能落盘留痕。
#[cfg(not(target_os = "windows"))]
fn unix_swap_script(
    pid: u32,
    exe: &Path,
    new_path: &Path,
    script: &Path,
    log: &Path,
    watchdog_stop: &str,
    watchdog_start: &str,
) -> String {
    let q = sh_quote;
    format!(
        "#!/bin/sh\n\
         # dsh-come 自更新换装脚本（自动生成，勿手工编辑）\n\
         # 异步执行：父进程脚本启动后即退出，故失败只能写日志供排查。\n\
         set -u\n\
         LOG={log}\n\
         note() {{ echo \"$(date '+%Y-%m-%d %H:%M:%S') $1\" >> \"$LOG\" 2>/dev/null; }}\n\
         fail() {{ echo \"$(date '+%Y-%m-%d %H:%M:%S') FAIL: $1\" >> \"$LOG\" 2>/dev/null; exit 1; }}\n\
         note \"换装开始 pid={pid}\"\n\
         # 0) 停系统级看门狗：launchd KeepAlive / systemd Restart 会在主进程退出瞬间\n\
         #    拉起旧版本 → pid 永不退出、换装死等。失败不阻断（无看门狗环境也允许更新）。\n\
         {watchdog_stop}\n\
         # 1) 等主进程退出（最长 30s）：exe 仍被占用时替换会失败\n\
         i=0\n\
         while [ $i -lt 30 ]; do\n\
         \x20   kill -0 {pid} 2>/dev/null || break\n\
         \x20   sleep 1\n\
         \x20   i=$((i + 1))\n\
         done\n\
         if kill -0 {pid} 2>/dev/null; then fail \"主进程 {pid} 未在 30s 内退出\"; fi\n\
         # 2) 前置校验（父进程已校验一次，这里再兜一层）\n\
         [ -f {new} ] || fail \"新版本文件不存在: {new}\"\n\
         [ -s {new} ] || fail \"新版本文件为空: {new}\"\n\
         # 3) 备份 → 替换 → 补可执行位\n\
         cp -f {exe} {exe_bak} || fail \"备份旧版本失败\"\n\
         mv -f {new} {exe} || fail \"替换 exe 失败\"\n\
         chmod +x {exe} || fail \"设置可执行位失败\"\n\
         note \"已替换为新版本\"\n\
         # 4) 恢复看门狗（unit/plist 仍指向同一路径，直接重启即可）\n\
         {watchdog_start}\n\
         # 5) 拉起新版本（nohup 脱离本脚本，避免随脚本退出被带走）\n\
         nohup {exe} >/dev/null 2>&1 &\n\
         note \"已拉起新版本\"\n\
         # 6) 自删\n\
         rm -f {script}\n\
         exit 0\n",
        log = q(&log.to_string_lossy()),
        pid = pid,
        watchdog_stop = watchdog_stop,
        new = q(&new_path.to_string_lossy()),
        exe = q(&exe.to_string_lossy()),
        exe_bak = q(&format!("{}.bak", exe.to_string_lossy())),
        watchdog_start = watchdog_start,
        script = q(&script.to_string_lossy()),
    )
}

/// 换装窗口期的看门狗停/复命令（平台对应物；与 scripts/install-watchdog.sh 注册的
/// launchd LaunchAgent / systemd user unit 配套）。命令均带 `|| true` 兜底：失败只留痕
/// 不阻断换装。返回空串 = 该平台无看门狗实现（其他 Unix），脚本里留空行不执行。
#[cfg(target_os = "macos")]
fn watchdog_control() -> (&'static str, &'static str) {
    (
        // bootout 失败（未注册/重复卸载）无碍；$HOME 由脚本运行时环境提供
        "launchctl bootout gui/$(id -u)/com.qing3a.dsh-come 2>/dev/null || true",
        // Catalina 前无 bootstrap：回落 load -w
        "launchctl bootstrap gui/$(id -u) \"$HOME/Library/LaunchAgents/com.qing3a.dsh-come.plist\" 2>/dev/null || launchctl load -w \"$HOME/Library/LaunchAgents/com.qing3a.dsh-come.plist\" 2>/dev/null || true",
    )
}

#[cfg(target_os = "linux")]
fn watchdog_control() -> (&'static str, &'static str) {
    (
        "systemctl --user stop dsh-come.service 2>/dev/null || true",
        "systemctl --user start dsh-come.service 2>/dev/null || true",
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn watchdog_control() -> (&'static str, &'static str) {
    ("", "")
}

/// 写换装脚本并拉起，返回后调用方应立即退出（supervisor::shutdown + exit）。
pub fn install(new_path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前 exe: {e}"))?;
    let pid = std::process::id();

    // 前置校验：换装脚本异步执行（本进程随即退出，无法回传结果）。
    // 这些问题若在这里不拦下，就会变成「下载成功、SHA256 校验通过，但更新其实没生效」，
    // 而 UI 只看 spawn 是否成功，会显示「更新成功」——典型的静默失败。
    if !new_path.is_file() {
        return Err(format!("新版本文件不存在: {}", new_path.display()));
    }
    let new_len = std::fs::metadata(new_path).map(|m| m.len()).unwrap_or(0);
    if new_len == 0 {
        return Err(format!("新版本文件为空: {}", new_path.display()));
    }
    let Some(exe_dir) = exe.parent() else {
        return Err(format!("无法定位 exe 所在目录: {}", exe.display()));
    };
    if !exe_dir.is_dir() {
        return Err(format!("exe 所在目录不存在: {}", exe_dir.display()));
    }

    #[cfg(not(target_os = "windows"))]
    {
        let script = crate::runtime::root_dir().join("swap-update.sh");
        let log = crate::runtime::root_dir().join("swap-update.log");
        let (wd_stop, wd_start) = watchdog_control();
        let content = unix_swap_script(pid, &exe, new_path, &script, &log, wd_stop, wd_start);
        std::fs::write(&script, content).map_err(|e| format!("写入换装脚本失败: {e}"))?;
        // 用 sh 显式执行：不依赖脚本自身的可执行位，也不依赖 shebang 支持
        std::process::Command::new("sh")
            .arg(&script)
            .spawn()
            .map_err(|e| format!("启动换装脚本失败: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let script = crate::runtime::root_dir().join("swap-update.ps1");
        // 脚本流程：禁用看门狗 → 等本进程退出（≤30s）→ 备份 `.bak` → 替换 → 重启 → 恢复看门狗 → 自删。
        let content = format!(
            "$ErrorActionPreference = 'SilentlyContinue'\n\
             # 1) 禁用看门狗：换 exe 窗口期防旧版本被拉起锁住文件\n\
             schtasks /Change /TN 'DSH伴侣守护' /Disable | Out-Null\n\
             # 2) 等 dsh-come 退出（最长 30s）\n\
             $deadline = (Get-Date).AddSeconds(30)\n\
             while ((Get-Date) -lt $deadline) {{\n\
                 if (-not (Get-Process -Id {pid} -ErrorAction SilentlyContinue)) {{ break }}\n\
                 Start-Sleep -Milliseconds 400\n\
             }}\n\
             # 3) 备份旧 exe（回滚用）→ 替换\n\
             Copy-Item -LiteralPath '{exe}' -Destination '{exe}.bak' -Force\n\
             Move-Item -LiteralPath '{new}' -Destination '{exe}' -Force\n\
             # 4) 拉起新版本 → 恢复看门狗 → 自删脚本\n\
             Start-Process -FilePath '{exe}'\n\
             schtasks /Change /TN 'DSH伴侣守护' /Enable | Out-Null\n\
             Remove-Item -LiteralPath '{script}' -Force\n",
            pid = pid,
            exe = exe.display(),
            new = new_path.display(),
            script = script.display(),
        );
        // UTF-8 BOM：PowerShell 5.1 对无 BOM 的 UTF-8 中文按 ANSI 解析
        //（任务名「DSH伴侣守护」会乱码）
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(content.as_bytes());
        std::fs::write(&script, bytes).map_err(|e| format!("写入换装脚本失败: {e}"))?;
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script);
        crate::supervisor::hide_window(&mut cmd);
        cmd.spawn()
            .map_err(|e| format!("启动换装脚本失败: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_numeric() {
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.2.0", "0.1.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.9", "0.2.0"), Ordering::Less);
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ordering::Greater);
        // 位数不同：0.2 > 0.2.0 视为相等；1.0 > 0.9.9
        assert_eq!(compare_versions("0.2", "0.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "0.9.9"), Ordering::Greater);
    }

    #[test]
    fn version_compare_ignores_suffix() {
        // rc 后缀（dsh 版本形态 0.1.1-rc.2）→ 取数字段比较
        assert_eq!(compare_versions("0.1.1-rc.2", "0.1.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.2.0", "0.1.1-rc.2"), Ordering::Greater);
    }

    #[test]
    fn hex_lower_formats() {
        assert_eq!(hex_lower(&[0xde, 0xad]), "dead");
        assert_eq!(hex_lower(&[0x00, 0xff, 0x10]), "00ff10");
        assert_eq!(hex_lower(&[]), "");
    }

    /// 换装脚本模板锁：看门狗停/复、chmod +x、备份/替换/拉起/自删必须都在——
    /// 换装是异步执行（父进程随即退出），模板少一行就是静默失败，只能靠单测兜住。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_swap_script_contains_watchdog_and_swap_steps() {
        let script = unix_swap_script(
            1234,
            Path::new("/opt/dsh-come/dsh-come"),
            Path::new("/opt/dsh-come/dsh-come.new"),
            Path::new("/opt/dsh-come/swap-update.sh"),
            Path::new("/opt/dsh-come/swap-update.log"),
            "systemctl --user stop dsh-come.service 2>/dev/null || true",
            "systemctl --user start dsh-come.service 2>/dev/null || true",
        );
        // 看门狗停/复命令原样进入脚本（换装窗口期防 KeepAlive/Restart 拉起旧版本）
        assert!(script.contains("systemctl --user stop dsh-come.service"), "缺少停看门狗步骤");
        assert!(script.contains("systemctl --user start dsh-come.service"), "缺少恢复看门狗步骤");
        // 顺序：停看门狗在等退出之前，恢复在替换之后（顺序错 = 换装必死等或误拉旧版）
        let stop_idx = script.find("systemctl --user stop").unwrap();
        let wait_idx = script.find("kill -0 1234").unwrap();
        let swap_idx = script.find("mv -f").unwrap();
        let start_idx = script.find("systemctl --user start").unwrap();
        assert!(stop_idx < wait_idx, "停看门狗必须在等退出之前");
        assert!(swap_idx < start_idx, "恢复看门狗必须在替换之后");
        // Unix 必补可执行位（下载文件不保证带 +x，mv 保留新文件自身权限）
        assert!(script.contains("chmod +x"), "缺少 chmod +x");
        // 路径被单引号包裹（含空格/特殊字符路径安全）
        assert!(script.contains("'/opt/dsh-come/dsh-come'"), "exe 路径必须 sh_quote");
        assert!(script.contains("'/opt/dsh-come/dsh-come.new'"), "new 路径必须 sh_quote");
        // 备份/拉起/自删
        assert!(script.contains("cp -f"), "缺少备份步骤");
        assert!(script.contains("nohup"), "缺少拉起新版本步骤");
        assert!(script.contains("rm -f"), "缺少自删步骤");
    }

    /// 无看门狗平台（命令为空串）：脚本留空行不执行、不阻断换装，主流程完整。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_swap_script_empty_watchdog_cmds_are_noop() {
        let script = unix_swap_script(
            1,
            Path::new("/tmp/dsh-come"),
            Path::new("/tmp/dsh-come.new"),
            Path::new("/tmp/swap-update.sh"),
            Path::new("/tmp/swap-update.log"),
            "",
            "",
        );
        assert!(script.contains("kill -0 1"), "空看门狗命令不应影响等退出");
        assert!(script.contains("mv -f"), "空看门狗命令不应影响替换");
    }
}
