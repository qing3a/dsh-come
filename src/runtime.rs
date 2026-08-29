//! 运行时目录布局与系统 dsh/node 定位。
//!
//! ```text
//! %LOCALAPPDATA%\dsh-come\       # 启动器自己的数据（可被 DSH_COME_HOME 覆盖）
//! ├── config.json                # 启动器配置（端口/重启上限/状态端口等）
//! ├── logs\                      # 滚动日志
//! └── come.patch.yml             # dsh CLI --patch overlay（禁用 dsh-market detached 重启）
//! ```
//!
//! dsh 本体与数据**不由本启动器隔离**，直接走系统 dsh（正常设计逻辑）：
//! - 运行器：PATH 中的系统 `dsh` 命令直启（**无 npx 临时拉取回退**——2026-08-19 用户拍板：
//!   dsh 缺失就走正常安装，见 `src/installer.rs`；探测合并进程 PATH + 注册表 PATH + `npm prefix -g`）
//! - 数据：不设置 DSH_HOME，dsh 用其默认目录（`%USERPROFILE%\.dsh`），与终端里正常用法一致
//!
//! # 数据目录命名史
//!
//! `dsh-desktop`（初名）→ `dsh-companion` → `dsh-come`（定名）。仓库/二进制/自启项都改了，
//! 唯独运行时数据目录 `%LOCALAPPDATA%\dsh-desktop` 与 `DSH_DESKTOP_HOME` 环境变量一直没动
//! （历次更名时为免迁移用户数据）。2026-08-29 审计 P1-5：跨平台正式发布前是改名的
//! **最后窗口**（发布后用户数据铺开就改不动了）。因此：
//! - 新默认目录 = `dsh-come`；新环境变量 = `DSH_COME_HOME`（旧 `DSH_DESKTOP_HOME` 仍兼容读取）。
//! - 启动时尽力迁移旧默认目录（`migrate_legacy_dir`）：仅当旧目录存在、新目录不存在、
//!   且没有显式设置任何 env 时才执行；rename 失败只记日志，绝不 copy 半写状态。

use std::path::PathBuf;

/// 启动器数据根目录：env DSH_COME_HOME > env DSH_DESKTOP_HOME（兼容旧名）>
/// 平台默认（Windows %LOCALAPPDATA%\dsh-come；Unix $XDG_DATA_HOME/dsh-come
/// 或 ~/.local/share/dsh-come）。
pub fn root_dir() -> PathBuf {
    if let Ok(h) = std::env::var("DSH_COME_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(h) = std::env::var("DSH_DESKTOP_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("dsh-come"))
            .unwrap_or_else(|| PathBuf::from(".dsh-come"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(x).join("dsh-come");
        }
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/share/dsh-come"))
            .unwrap_or_else(|| PathBuf::from(".dsh-come"))
    }
}

/// 旧数据目录（改名前的默认布局，仅迁移用）：Windows %LOCALAPPDATA%\dsh-desktop；
/// Unix $XDG_DATA_HOME/dsh-desktop 或 ~/.local/share/dsh-desktop。
/// 返回 None 表示平台路径无法确定（无 LOCALAPPDATA/HOME）。
fn legacy_root_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("dsh-desktop"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            Some(PathBuf::from(x).join("dsh-desktop"))
        } else {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/dsh-desktop"))
        }
    }
}

/// 旧目录是否仍被旧版守护占用（Unix 锁文件探测）。
/// 改名后新旧锁文件路径不同，mutex 类机制挡不住双实例——只有 flock 能如实反映
/// 「旧版守护是否还活着」。返回 true = 旧版守护在跑，调用方应阻止本实例继续启动。
/// Windows 由进程级 named mutex 天然防双开（与路径无关），无需此探测。
#[cfg(unix)]
pub fn legacy_daemon_running() -> bool {
    if std::env::var_os("DSH_COME_HOME").is_some() || std::env::var_os("DSH_DESKTOP_HOME").is_some() {
        return false; // 显式指路：不迁移也就无所谓旧守护
    }
    let Some(old_root) = legacy_root_dir() else { return false };
    old_root.is_dir() && !try_lock_file(&old_root.join("dsh-come.lock"))
}

#[cfg(windows)]
#[allow(dead_code)] // 仅 Unix 语义有意义；Windows 构建下 main 的调用点被 cfg 裁掉，此桩恒无人调用
pub fn legacy_daemon_running() -> bool {
    false
}

/// 尝试对锁文件加非阻塞 flock；失败（被持有 / 打不开）返回 false。
#[cfg(unix)]
fn try_lock_file(p: &std::path::Path) -> bool {
    use std::os::unix::io::AsRawFd;
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(p)
    {
        Ok(f) => {
            // SAFETY: flock 是标准锁调用；fd 合法
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
        }
        Err(_) => false,
    }
}

/// 迁移旧默认数据目录（dsh-desktop → dsh-come）。
///
/// 条件（全部满足才动）：未设置任何显式 env（DSH_COME_HOME/DSH_DESKTOP_HOME）、
/// 旧目录存在、新目录不存在。Unix 下还要求旧目录**没有被旧版守护占用**
/// （占用了就跳过，避免把正在写的目录搬走）。
///
/// 迁移 = 同卷 `rename`（原子、快）。rename 失败只记日志跳过（下次启动重试），
/// **绝不 copy**——rename 失败基本意味着目录被占用或权限问题，copy 半写状态比不迁更糟。
pub fn migrate_legacy_dir() {
    if std::env::var_os("DSH_COME_HOME").is_some() || std::env::var_os("DSH_DESKTOP_HOME").is_some() {
        return; // 显式指路：不迁移
    }
    let new_root = root_dir();
    let Some(old_root) = legacy_root_dir() else { return };
    if !old_root.is_dir() || new_root.exists() {
        return; // 没有旧目录 / 新目录已存在 → 无事可做
    }
    #[cfg(unix)]
    if !try_lock_file(&old_root.join("dsh-come.lock")) {
        crate::supervisor::log(&format!(
            "旧版守护仍在使用旧数据目录 {}，迁移推迟到下次启动",
            old_root.display()
        ));
        return;
    }
    match migrate_dir(&old_root, &new_root) {
        Ok(()) => crate::supervisor::log(&format!(
            "旧数据目录 {} 已迁移到 {}",
            old_root.display(),
            new_root.display()
        )),
        Err(e) => crate::supervisor::log(&format!(
            "旧数据目录迁移失败（跳过，下次启动重试）：{e}"
        )),
    }
}

/// 纯迁移动作：旧目录存在且新目录不存在时 rename（同卷、原子）。
/// 其余情况视为「无事可做」返回 Ok。rename 失败返回 Err（调用方记日志）。
/// 单独成函数以便单测（不依赖真实数据目录/env）。
fn migrate_dir(old: &std::path::Path, new: &std::path::Path) -> Result<(), String> {
    if !old.is_dir() || new.exists() {
        return Ok(());
    }
    std::fs::rename(old, new)
        .map_err(|e| format!("rename {} → {}: {e}", old.display(), new.display()))
}

pub fn logs_dir() -> PathBuf {
    root_dir().join("logs")
}

/// 守护状态快照（CLI `status` 跨进程读取）：监测线程每轮写入。
pub fn state_path() -> PathBuf {
    root_dir().join("state.json")
}

/// 控制请求文件（CLI `stop` 写入，监测线程下一轮消费）：存在即「停止 dsh」。
pub fn control_path() -> PathBuf {
    root_dir().join("control.json")
}

/// 引擎滚动日志文件（stdout/stderr 重定向，防管道阻塞 + 留诊断）
pub fn engine_log() -> PathBuf {
    logs_dir().join("engine.log")
}

/// dsh 的数据根（$DSH_HOME）：优先环境变量 DSH_HOME，缺省平台默认（Windows %USERPROFILE%\.dsh；
/// Unix $HOME/.dsh，dsh 官方默认）。启动器不设置该变量、也不写这里——保持与终端里正常使用 dsh 完全一致。
pub fn system_home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("DSH_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|p| PathBuf::from(p).join(".dsh"))
        .unwrap_or_else(|| PathBuf::from(".dsh"))
}

// ---------- Node 版本兼容 ----------

/// dsh 0.1.1-rc.2+ 依赖的 Node API（Promise.withResolvers / stripTypeScriptTypes /
/// createZstdDecompress）需要 Node 22+。部分环境（IDE 沙箱 / 豆包工作环境等）会把
/// 旧版 node 注入到 PATH 最前面，导致 dsh 用低版本 node 启动而崩溃。这里探测 PATH
/// 中第一个 >= min_major 的 node，把其目录提升到 PATH 最前面，确保 dsh / npm 子进程用对版本。

/// 解析 `node --version` 输出的主版本号（"v22.5.1" → 22）。失败 → None。
fn node_major_version(node_exe: &std::path::Path) -> Option<u32> {
    let out = std::process::Command::new(node_exe)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout);
    let ver = v.trim().strip_prefix('v')?;
    ver.split('.').next()?.parse::<u32>().ok()
}

/// 遍历 PATH，找到第一个 node 主版本 >= min_major 的目录。
/// 找不到（PATH 无 node / 全部低于要求）→ None。
pub fn find_node_dir_at_least(min_major: u32) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) { "node.exe" } else { "node" });
        if candidate.is_file() {
            if let Some(major) = node_major_version(&candidate) {
                if major >= min_major {
                    return Some(dir);
                }
            }
        }
    }
    None
}

/// 修正当前进程 PATH：若存在 >= min_major 的 node 且不在最前面，则把它的目录移到最前面。
/// 返回是否做了修正（供日志）。找不到兼容 node 或已在最前面 → false。
pub fn prioritize_compatible_node(min_major: u32) -> bool {
    let Some(dir) = find_node_dir_at_least(min_major) else {
        return false;
    };
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    let mut paths: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if paths.first() == Some(&dir) {
        return false; // 已在最前面
    }
    paths.retain(|p| p != &dir);
    paths.insert(0, dir);
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
        return true;
    }
    false
}

// ---------- 系统 dsh 定位 ----------

/// dsh 运行器：PATH（含注册表与 npm 全局目录）中的系统 dsh 命令。
/// 无 npx 回退——缺失即走安装流程（src/installer.rs）。
#[derive(Debug, Clone, PartialEq)]
pub struct DshRunner(pub PathBuf);

impl DshRunner {
    pub fn describe(&self) -> String {
        format!("系统 dsh（{}）", self.0.display())
    }
}

/// 探测系统 dsh：PATH 直启路径（合并进程 PATH / 注册表 PATH / `npm prefix -g`）。
pub fn dsh_runner() -> Option<DshRunner> {
    crate::installer::which("dsh").map(DshRunner)
}

/// 构造 dsh 命令（spawn 用）。
/// - Windows：dsh 是 .cmd 包装，CreateProcess 不能直接执行 .cmd → `cmd /C <dsh> <args…>`
///   （进程树由 supervisor 的 Job Object / taskkill /T 整树清理）。
/// - Unix：dsh 是可执行脚本（shebang 指向 node），直接 spawn；supervisor 在 spawn 时
///   `process_group(0)` 建独立进程组，杀树用 `kill -pgid`。
pub fn dsh_command(runner: &DshRunner, args: &[String]) -> std::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/C");
        cmd.arg(&runner.0);
        cmd.args(args);
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = std::process::Command::new(&runner.0);
        cmd.args(args);
        cmd
    }
}

/// 查询系统 dsh 版本（`dsh --version`）。失败/不可得 → None。
pub fn dsh_version() -> Option<String> {
    let runner = dsh_runner()?;
    let args: Vec<String> = vec!["--version".to_string()];
    let mut cmd = dsh_command(&runner, &args);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 引擎实际运行的 dsh 版本（状态行展示）。cfg 保留签名兼容，实际不依赖配置。
pub fn resolved_version(_cfg: &crate::config::AppConfig) -> Option<String> {
    dsh_version()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dsh_command 构造形态（平台化）：
    /// - Windows：`cmd /C dsh <args>`（.cmd 包装需 cmd 直启）
    /// - Unix：直接 `dsh <args>`（可执行脚本，shebang 指向 node）
    #[test]
    fn dsh_command_direct_shape() {
        let cmd = dsh_command(&DshRunner(PathBuf::from("dsh")), &["web".to_string()]);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        #[cfg(target_os = "windows")]
        {
            assert!(args.iter().any(|a| a == "dsh"), "直启 dsh: {args:?}");
        }
        #[cfg(not(target_os = "windows"))]
        {
            // 直启形态：无 cmd /C 包装
            assert!(!args.iter().any(|a| a == "/C"), "Unix 不应有 cmd /C: {args:?}");
            assert!(args.iter().any(|a| a == "web"), "透传 web: {args:?}");
        }
        assert!(args.iter().any(|a| a == "web"), "透传 web: {args:?}");
        assert!(!args.iter().any(|a| a.starts_with("@deepseek-ai")), "无 npm 包名: {args:?}");
    }

    // ---------- 数据目录迁移（P1-5：dsh-desktop → dsh-come） ----------

    fn temp_base(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dsh-come-mig-{tag}-{}", std::process::id()))
    }

    /// 旧目录存在、新目录不存在 → rename 成功：旧消失、新出现且内容完整。
    #[test]
    fn migrate_dir_moves_old_to_new() {
        let base = temp_base("t1");
        let old = base.join("old");
        let new = base.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("state.json"), "{}").unwrap();
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::write(old.join("logs").join("engine.log"), "x").unwrap();

        assert!(migrate_dir(&old, &new).is_ok());
        assert!(!old.exists(), "旧目录应消失");
        assert!(new.join("state.json").is_file(), "文件应随目录迁走");
        assert!(new.join("logs").join("engine.log").is_file(), "子目录应完整迁移");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 新目录已存在 → 绝不动（防覆盖新数据）。
    #[test]
    fn migrate_dir_skips_when_new_exists() {
        let base = temp_base("t2");
        let old = base.join("old");
        let new = base.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("fresh.json"), "{}").unwrap();

        assert!(migrate_dir(&old, &new).is_ok());
        assert!(old.is_dir(), "新目录已存在时不应动旧目录");
        assert!(new.join("fresh.json").is_file(), "新目录内容不得被覆盖");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 旧目录不存在 → 无事可做，Ok。
    #[test]
    fn migrate_dir_noop_when_old_missing() {
        let base = temp_base("t3");
        let old = base.join("old");
        let new = base.join("new");
        assert!(migrate_dir(&old, &new).is_ok());
        assert!(!new.exists());
    }
}

// ---------- 壳 patch overlay ----------

/// 壳维护的 patch overlay 路径（root\come.patch.yml）：spawn dsh web 时经 `--patch`
/// 传入（CLI 契约面用法，不写 profile 内部文件）。当前内容：dsh-market 安装后
/// 禁用其 detached 一键重启（重启归 supervisor 管，防止绕过崩溃自愈/退避/日志）。
pub fn come_patch_path() -> PathBuf {
    root_dir().join("come.patch.yml")
}

/// 幂等写入 come.patch.yml（内容固定；已存在则跳过）。
/// dsh-market 未安装时该覆盖条目在加载期仅 warn 一条（applyEntryPatches 对
/// 未找到的 entry 报 warning 后跳过），无副作用。
pub fn ensure_come_patch() -> std::io::Result<()> {
    let p = come_patch_path();
    if p.is_file() {
        return Ok(());
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        &p,
        "# dsh-come 壳维护的 patch overlay：dsh-market 安装后禁止其 detached 一键重启\n\
         # （dsh 进程由壳 supervisor 接管：崩溃自愈 / 退避重启 / 滚动日志）\n\
         - id: dsh-market\n\
         \x20 config:\n\
         \x20   allowRestart: false\n",
    )
}

/// 确保目录骨架存在（首次运行补齐）
pub fn ensure_layout() -> std::io::Result<()> {
    std::fs::create_dir_all(logs_dir())?;
    ensure_come_patch()?;
    Ok(())
}
