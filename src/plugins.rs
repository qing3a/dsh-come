//! 插件市场：可信清单（✓已验证）+ 一键安装/卸载。
//!
//! 执行机制（契约 C5）：`dsh plugin --profile web <pnpm args>` 转发到 profile 目录的 pnpm；
//! 插件装进 $DSH_HOME/profiles/web/（启动器隔离的 home\），不碰 dsh 包本体。
//! 市场只负责「提供可信清单 + 一键调用 + 装后提示重启」——信任决策保留给用户，
//! 自动信任只给已验证清单（dsh-plugin-verify / dsh-event-auditor 产出喂 verified.json）。
//!
//! pnpm 依赖处理：捆绑 Node 自带 npm/npx 但无 pnpm → ensure_pnpm() 把 pnpm 装进捆绑
//! node 的全局目录（幂等），spawn dsh plugin 时把 node_dir 注入 PATH 供其解析。

use crate::config::AppConfig;
use crate::runtime;
use crate::supervisor;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    /// npm 包名（@scope/pkg）——安装命令直接用
    pub id: String,
    /// 显示名
    pub name: String,
    pub version: String,
    /// 运行时验证通过（dsh-plugin-verify 产出）
    pub verified: bool,
    pub desc: String,
    pub repo: Option<String>,
}

/// 内置可信清单（v1 固定；v2 改为从 verified.json GitHub raw 拉取）。
/// 只放运行时验证 ✅ 的插件——这是差异化数据（286 个插件仅极少数有验证证据）。
pub fn builtin_marketplace() -> Vec<PluginInfo> {
    vec![
        PluginInfo {
            id: "@qing3a/dsh-event-auditor".to_string(),
            name: "事件审计".to_string(),
            version: "0.4".to_string(),
            verified: true,
            desc: "事件 waterfall 审计 + /audit 静态页（settings 热改）".to_string(),
            repo: Some("github.com/qing3a/dsh-event-auditor".to_string()),
        },
        PluginInfo {
            id: "@dsh-external/dsh-tray".to_string(),
            name: "内置托盘增强".to_string(),
            version: "0.1".to_string(),
            verified: true,
            desc: "进程内托盘（气泡通知）。桌面版已自带托盘，一般无需安装".to_string(),
            repo: Some("github.com/qing3a/dsh-tray".to_string()),
        },
    ]
}

/// 远程清单（v2 预留）：GET verified.json（GitHub raw）→ 增量覆盖内置清单。
/// 依赖 GitHub 仓库建立后启用；结构上与 builtin_marketplace 同构。
#[allow(dead_code)]
pub fn fetch_remote_marketplace() -> Result<Vec<PluginInfo>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get("https://raw.githubusercontent.com/qing3a/dsh-desktop/main/verified.json")
        .send()
        .map_err(|e| format!("拉取 verified.json 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("verified.json 返回 {}", resp.status()));
    }
    resp.json().map_err(|e| format!("解析 verified.json 失败: {e}"))
}

/// 确保 pnpm 在捆绑 node 全局目录可用（幂等：存在 pnpm.cmd 即跳过）。
/// `node npm-cli.js install --prefix <node_dir> -g pnpm`
fn ensure_pnpm() -> Result<(), String> {
    let pnpm_exe = runtime::node_dir().join("pnpm.cmd");
    if pnpm_exe.is_file() {
        return Ok(());
    }
    let node = runtime::node_exe();
    let npm = runtime::npm_cli_js();
    if !node.is_file() {
        return Err(format!("未找到捆绑 Node: {}", node.display()));
    }
    if !npm.is_file() {
        return Err(format!("未找到捆绑 npm-cli: {}", npm.display()));
    }
    supervisor::log(&format!("首次插件操作：安装 pnpm 到捆绑 node（--prefix {}）", runtime::node_dir().display()));
    let out = Command::new(&node)
        .arg(&npm)
        .args(["install", "--prefix", &runtime::node_dir().to_string_lossy(), "-g", "pnpm"])
        .output()
        .map_err(|e| format!("安装 pnpm 失败: {e}"))?;
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr).lines().rev().take(5).collect::<Vec<_>>().join("\n");
        return Err(format!("安装 pnpm 失败（code={:?}）\n{tail}", out.status.code()));
    }
    if !pnpm_exe.is_file() {
        return Err(format!("pnpm 安装后仍缺失: {}", pnpm_exe.display()));
    }
    supervisor::log("pnpm 就绪");
    Ok(())
}

/// 执行 dsh plugin 命令（契约 C5）：node npx-cli.js --yes @deepseek-ai/dsh@<ver> plugin --profile web <sub...>
/// PATH 注入 node_dir（转发到的 pnpm 从捆绑 node 解析，不依赖系统安装）。
fn run_plugin_cmd(dsh_ver: &str, sub: &[&str]) -> Result<String, String> {
    let node = runtime::node_exe();
    let npx = runtime::npx_cli_js();
    let home = runtime::home_dir();
    let mut cmd = Command::new(&node);
    cmd.arg(&npx)
        .arg("--yes")
        .arg(format!("@deepseek-ai/dsh@{dsh_ver}"))
        .args(["plugin", "--profile", "web"])
        .args(sub)
        .current_dir(&home)
        .env("DSH_HOME", &home);
    supervisor::hide_window(&mut cmd);
    // PATH 前插 node_dir：让 dsh plugin 转发到的 pnpm 可解析
    if let Some(p) = std::env::var_os("PATH") {
        let mut paths = vec![runtime::node_dir().to_string_lossy().into_owned()];
        paths.push(p.to_string_lossy().into_owned());
        cmd.env("PATH", paths.join(";"));
    }
    let out = cmd.output().map_err(|e| format!("执行 dsh plugin 失败: {e}"))?;
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("dsh plugin 失败（code={:?}）\n{tail}", out.status.code()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() {
        Ok("已执行（无输出）".to_string())
    } else {
        Ok(stdout.lines().rev().take(5).collect::<Vec<_>>().join("\n"))
    }
}

/// 安装插件（装进 $DSH_HOME/profiles/web/；完成后需重启 dsh 生效——不自动打断运行中会话）
pub fn install_plugin(cfg: &AppConfig, id: &str) -> Result<String, String> {
    let ver = current_dsh_version(cfg)?;
    ensure_pnpm()?;
    supervisor::log(&format!("安装插件 {id}（dsh {ver}）"));
    let out = run_plugin_cmd(&ver, &["add", id])?;
    Ok(format!("已安装 {id}（重启 dsh 后生效）\n{out}"))
}

/// 卸载插件
pub fn uninstall_plugin(cfg: &AppConfig, id: &str) -> Result<String, String> {
    let ver = current_dsh_version(cfg)?;
    ensure_pnpm()?;
    supervisor::log(&format!("卸载插件 {id}"));
    let out = run_plugin_cmd(&ver, &["remove", id])?;
    Ok(format!("已卸载 {id}（重启 dsh 后生效）\n{out}"))
}

/// 已安装插件：读 $DSH_HOME/profiles/web/package.json 的 dependencies 键
pub fn installed_plugins() -> Vec<String> {
    let pkg = runtime::home_dir().join("profiles").join("web").join("package.json");
    let Ok(s) = std::fs::read_to_string(&pkg) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { return Vec::new() };
    let mut out: Vec<String> = v
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn current_dsh_version(_cfg: &AppConfig) -> Result<String, String> {
    runtime::load_state()
        .current
        .ok_or_else(|| "尚未完成首次启动（无锁定版本）".to_string())
}
