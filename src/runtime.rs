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
    if std::env::var_os("DSH_COME_HOME").is_some() || std::env::var_os("DSH_DESKTOP_HOME").is_some()
    {
        return false; // 显式指路：不迁移也就无所谓旧守护
    }
    let Some(old_root) = legacy_root_dir() else {
        return false;
    };
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
    if std::env::var_os("DSH_COME_HOME").is_some() || std::env::var_os("DSH_DESKTOP_HOME").is_some()
    {
        return; // 显式指路：不迁移
    }
    let new_root = root_dir();
    let Some(old_root) = legacy_root_dir() else {
        return;
    };
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
        Err(e) => crate::supervisor::log(&format!("旧数据目录迁移失败（跳过，下次启动重试）：{e}")),
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

/// 当前 Unix 时间戳（秒）。全壳统一时间源（此前 supervisor/tray/updater 各写一份）。
pub fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// dsh 0.1.1-rc.2+ 依赖的 Node API（Promise.withResolvers / stripTypeScriptTypes /
/// createZstdDecompress）需要 Node 22+。部分环境（IDE 沙箱 / 豆包工作环境等）会把
/// 旧版 node 注入到 PATH 最前面，导致 dsh 用低版本 node 启动而崩溃。这里探测 PATH
/// 中第一个 >= min_major 的 node，把其目录提升到 PATH 最前面，确保 dsh / npm 子进程用对版本。

/// 解析 `node --version` 输出的主版本号（"v22.5.1" → 22）。失败 → None。
fn node_major_version(node_exe: &std::path::Path) -> Option<u32> {
    let mut cmd = std::process::Command::new(node_exe);
    cmd.arg("--version");
    let v = crate::supervisor::capture_trimmed(&mut cmd, std::time::Duration::from_secs(3))?;
    let ver = v.strip_prefix('v')?;
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
/// 带超时：start() 持锁期间会调用本函数（resolved_version），子进程挂起即拖死守护。
pub fn dsh_version() -> Option<String> {
    let runner = dsh_runner()?;
    let args: Vec<String> = vec!["--version".to_string()];
    let mut cmd = dsh_command(&runner, &args);
    crate::supervisor::capture_trimmed(&mut cmd, std::time::Duration::from_secs(3))
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
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        #[cfg(target_os = "windows")]
        {
            assert!(args.iter().any(|a| a == "dsh"), "直启 dsh: {args:?}");
        }
        #[cfg(not(target_os = "windows"))]
        {
            // 直启形态：无 cmd /C 包装
            assert!(
                !args.iter().any(|a| a == "/C"),
                "Unix 不应有 cmd /C: {args:?}"
            );
            assert!(args.iter().any(|a| a == "web"), "透传 web: {args:?}");
        }
        assert!(args.iter().any(|a| a == "web"), "透传 web: {args:?}");
        assert!(
            !args.iter().any(|a| a.starts_with("@deepseek-ai")),
            "无 npm 包名: {args:?}"
        );
    }

    /// 测试专用：独立的临时 DSH_COME_HOME（串行测试下 set_var 安全）
    fn test_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "come-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("DSH_COME_HOME", &home);
        home
    }

    /// ensure_come_patch + ensure_hlp_plugin 全流程（合一个测试串行跑：
    /// 三场景都依赖 DSH_COME_HOME 环境变量，Rust 测试并行会互相覆盖 env）
    #[test]
    fn come_patch_and_hlp_plugin_flow() {
        // 场景 1：旧 patch（只有 dsh-market）→ 自动追加 HLP 条目且保留原内容
        let home = test_home("append");
        let p = home.join("come.patch.yml");
        std::fs::write(&p, "- id: dsh-market\n  config:\n    allowRestart: false\n").unwrap();
        ensure_come_patch().unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(content.contains("dsh-market"), "原条目保留");
        assert!(
            content.contains("- insert:"),
            "HLP 条目用 insert 结构（新增插件语义）"
        );
        assert!(content.contains("- id: dsh-light-cockpit"), "追加 HLP 条目");
        assert!(
            content.contains("name: '@hlp/dsh-light-cockpit'"),
            "HLP 条目用 npm 包名（共享层解析）"
        );
        assert!(
            content.contains("mcp-client-mail"),
            "mail MCP 桥条目（@好友业务工具）"
        );
        assert!(content.contains("mcp-client-biz"), "biz MCP 桥条目");
        assert!(
            content.contains("/business/mail-collab-server"),
            "MCP server 指共享层插件内 business 路径"
        );
        std::fs::remove_dir_all(&home).ok();

        // 场景 2：已有 HLP 条目 → 幂等跳过（不重复追加）
        let home = test_home("idem");
        let p = home.join("come.patch.yml");
        std::fs::write(&p, "- id: dsh-market\n  config:\n    allowRestart: false\n- id: dsh-light-cockpit\n  name: 'file:///x/plugins/dsh-light-cockpit'\n").unwrap();
        ensure_come_patch().unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            content.matches("- id: dsh-light-cockpit").count(),
            1,
            "不重复追加"
        );
        std::fs::remove_dir_all(&home).ok();

        // 场景 3：exe 旁 hlp-plugin 源 → 部署到共享层（DSH_HOME 隔离），第二次幂等跳过
        let home = test_home("deploy");
        let dsh_home = home.join("dsh");
        std::env::set_var("DSH_HOME", &dsh_home);
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let src = exe_dir.join("hlp-plugin");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("package.json"),
            r#"{"name":"@hlp/dsh-light-cockpit","version":"0.0.2"}"#,
        )
        .unwrap();
        // 健康检查要求的完整骨架：入口/注册表目录/两个业务 Server 的 manifest
        std::fs::write(src.join("index.js"), "module.exports = {};").unwrap();
        std::fs::create_dir_all(src.join("lib")).unwrap();
        for biz in ["biz-cockpit-mcp", "mail-collab-server"] {
            std::fs::create_dir_all(src.join("business").join(biz)).unwrap();
            std::fs::write(
                src.join("business").join(biz).join("package.json"),
                r#"{"name":"biz","version":"0.0.1"}"#,
            )
            .unwrap();
        }
        assert!(ensure_hlp_plugin().unwrap(), "应从源部署");
        let dst = dsh_home
            .join("profiles")
            .join("node_modules")
            .join("@hlp")
            .join("dsh-light-cockpit");
        assert!(
            dst.join("package.json").is_file(),
            "部署后 manifest 存在于共享层"
        );
        assert!(!ensure_hlp_plugin().unwrap(), "第二次应幂等跳过");
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&src).ok();
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
        assert!(
            new.join("logs").join("engine.log").is_file(),
            "子目录应完整迁移"
        );
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

/// HLP 插件的部署目录 = DSH 共享层 `~/.dsh/profiles/node_modules/@hlp/dsh-light-cockpit`。
/// v1.4.0 实测：patch `name: file:///<root>/plugins/...` 不可行——① Node ESM 拒绝目录导入；
/// ② 指到 index.js 后宿主依赖（@deepseek-ai/dsh-tools 等）解析断链（只有 profiles/node_modules
/// 下才有宿主包）。故发行版附带插件**部署进共享层**（依赖链完整），patch 用 npm 包名加载。
/// 环境隔离：经 DSH_HOME（system_home_dir），测试可指临时目录。
pub fn hlp_plugin_dir() -> PathBuf {
    system_home_dir()
        .join("profiles")
        .join("node_modules")
        .join("@hlp")
        .join("dsh-light-cockpit")
}

/// 递归复制目录（hlp_plugin 部署用；插件树无符号链接，不做链接特殊处理）
pub(crate) fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<u64> {
    let mut copied = 0u64;
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copied += copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// 确保发行版附带的 HLP 插件已部署到 DSH 共享层（~/.dsh/profiles/node_modules/@hlp）。
/// 幂等判定用 hlp_plugin::healthy（比旧版「package.json 可解析」更强：损坏态自动重装）。
/// 部署实现单一来源 = hlp_plugin::deploy_from_source（此前 runtime 里有一份逐字相同的
/// 候选列表+复制循环，改一处漏一处）。无可用源/复制失败 → Ok(false) 留痕不阻塞启动
/// （patch 条目加载不到仅影响 LocalApp，与 dsh-market 未装同语义）。
/// 返回 Ok(true) 表示本次执行了部署。
pub fn ensure_hlp_plugin() -> std::io::Result<bool> {
    if crate::hlp_plugin::healthy() {
        return Ok(false); // 幂等：已部署且结构完整
    }
    match crate::hlp_plugin::deploy_from_source() {
        Ok(_) => Ok(true),
        Err(e) => {
            crate::supervisor::log(&format!("HLP 插件自动部署跳过：{e}"));
            Ok(false)
        }
    }
}

/// come.patch.yml 的 HLP 段：**insert 结构**（新增插件语义；裸条目 = 对已有插件做
/// 配置覆盖——dsh-market 那种——新装插件必须 `- insert:`，否则 DSH 报 entry not found，
/// v1.4.0 实测）。三条：① HLP 宿主插件（npm 包名，共享层解析）；② biz MCP 桥；
/// ③ mail MCP 桥——@好友 mentionable 的业务工具（mcp__mail-collab__*）必须经静态
/// mcp-client 注册才进 Agent 工具面板（launch 不注入工具，v1.2.7 实证）。
fn hlp_patch_entries() -> String {
    let base = hlp_plugin_dir().to_string_lossy().replace('\\', "/");
    let biz = format!("{base}/business/biz-cockpit-mcp");
    let mail = format!("{base}/business/mail-collab-server");
    format!(
        "- insert:\n    \
         - id: dsh-light-cockpit\n      \
         name: '@hlp/dsh-light-cockpit'\n    \
         - id: mcp-client-biz\n      \
         name: '@deepseek-ai/dsh-mcp-client'\n      \
         config:\n        \
         transport: stdio\n        \
         serverName: biz-cockpit\n        \
         command: node\n        \
         args:\n          \
         - '{biz}/index.js'\n        \
         cwd: '{biz}'\n        \
         toolCallTimeoutMs: 30000\n        \
         failOnStartupError: true\n    \
         - id: mcp-client-mail\n      \
         name: '@deepseek-ai/dsh-mcp-client'\n      \
         config:\n        \
         transport: stdio\n        \
         serverName: mail-collab\n        \
         command: node\n        \
         args:\n          \
         - '{mail}/index.js'\n        \
         cwd: '{mail}'\n        \
         toolCallTimeoutMs: 60000\n        \
         failOnStartupError: true\n"
    )
}

/// 幂等维护 come.patch.yml（内容感知，不再"存在即跳过"）：
/// - 不存在 → 写默认（dsh-market + dsh-light-cockpit）
/// - 存在但缺 dsh-light-cockpit 条目 → **追加**（旧用户升级后自动获得 HLP，不破坏原配置）
/// - 已含 → 原样跳过
/// dsh-market / dsh-light-cockpit 未安装/未部署时条目在加载期仅 warn 一条
/// （applyEntryPatches 对未找到的 entry 报 warning 后跳过），无副作用。
pub fn ensure_come_patch() -> std::io::Result<()> {
    let p = come_patch_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    const HLP_MARKER: &str = "dsh-light-cockpit";
    if p.is_file() {
        let existing = std::fs::read_to_string(&p).unwrap_or_default();
        if !existing.contains(HLP_MARKER) {
            let mut updated = existing;
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str("# dsh-come v0.3+：HLP 协议层 + LocalApp 生态（root\\plugins\\dsh-light-cockpit）\n");
            updated.push_str(&hlp_patch_entries());
            std::fs::write(&p, updated)?;
        }
        return Ok(());
    }
    let mut content = String::from(
        "# dsh-come 壳维护的 patch overlay：dsh-market 安装后禁止其 detached 一键重启\n\
         # （dsh 进程由壳 supervisor 接管：崩溃自愈 / 退避重启 / 滚动日志）\n\
         - id: dsh-market\n\
         \x20 config:\n\
         \x20   allowRestart: false\n\
         # HLP 协议层 + LocalApp 生态（v0.3+，root\\plugins\\dsh-light-cockpit）\n",
    );
    content.push_str(&hlp_patch_entries());
    std::fs::write(&p, content)
}

/// 确保目录骨架存在（首次运行补齐）
pub fn ensure_layout() -> std::io::Result<()> {
    std::fs::create_dir_all(logs_dir())?;
    ensure_hlp_plugin()?;
    ensure_come_patch()?;
    Ok(())
}
