//! 运行时目录布局与系统 dsh/node 定位。
//!
//! ```text
//! %LOCALAPPDATA%\dsh-desktop\      # 启动器自己的数据（可被 DSH_DESKTOP_HOME 覆盖）
//! ├── config.json                  # 启动器配置（端口/重启上限/状态端口等）
//! ├── logs\                        # 滚动日志
//! └── come.patch.yml               # dsh CLI --patch overlay（禁用 dsh-market detached 重启）
//! ```
//!
//! dsh 本体与数据**不由本启动器隔离**，直接走系统 dsh（正常设计逻辑）：
//! - 运行器：PATH 中的系统 `dsh` 命令直启（**无 npx 临时拉取回退**——2026-08-19 用户拍板：
//!   dsh 缺失就走正常安装，见 `src/installer.rs`；探测合并进程 PATH + 注册表 PATH + `npm prefix -g`）
//! - 数据：不设置 DSH_HOME，dsh 用其默认目录（`%USERPROFILE%\.dsh`），与终端里正常用法一致

use std::path::PathBuf;

/// 启动器数据根目录：env DSH_DESKTOP_HOME > %LOCALAPPDATA%\dsh-desktop
pub fn root_dir() -> PathBuf {
    if let Ok(h) = std::env::var("DSH_DESKTOP_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("dsh-desktop"))
        .unwrap_or_else(|| PathBuf::from(".dsh-desktop"))
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

/// dsh 的数据根（$DSH_HOME）：优先环境变量 DSH_HOME，缺省 %USERPROFILE%\.dsh（dsh 官方默认）。
/// 启动器不设置该变量、也不写这里——保持与终端里正常使用 dsh 完全一致。
pub fn system_home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("DSH_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join(".dsh"))
        .unwrap_or_else(|| PathBuf::from(".dsh"))
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

/// 构造 dsh 命令（spawn 用）：`cmd /C <dsh> <args…>`。
/// Windows 上 dsh 是 .cmd 包装，CreateProcess 不能直接执行 .cmd → 用 cmd /C 包装
/// （进程树由 supervisor 的 Job Object / taskkill /T 整树清理）。
pub fn dsh_command(runner: &DshRunner, args: &[String]) -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/C");
    cmd.arg(&runner.0);
    cmd.args(args);
    cmd
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

    /// dsh_command 直启系统 dsh，`cmd /C dsh <args>` 形态
    #[test]
    fn dsh_command_direct_shape() {
        let cmd = dsh_command(&DshRunner(PathBuf::from("dsh")), &["web".to_string()]);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.iter().any(|a| a == "dsh"), "直启 dsh: {args:?}");
        assert!(args.iter().any(|a| a == "web"), "透传 web: {args:?}");
        assert!(!args.iter().any(|a| a.starts_with("@deepseek-ai")), "无 npm 包名: {args:?}");
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
