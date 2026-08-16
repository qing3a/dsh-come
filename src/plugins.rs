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
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    /// 商品 id：dsh 插件 = npm 包名（@scope/pkg，安装命令直接用）；
    /// 工作台 = 资产标识（本地路径/仓库入口，安装语义见 kind 与 entry）。
    pub id: String,
    /// 显示名
    pub name: String,
    pub version: String,
    /// 运行时验证通过（dsh-plugin-verify / e2e 报告产出）
    pub verified: bool,
    pub desc: String,
    pub repo: Option<String>,
    /// 商品形态：`workbench`（工作台：场景完整的业务包，可含 UI/工具/外部服务依赖）
    /// 或 `tool`（单件工具）。旧清单缺省按 tool 处理（#[serde(default)]）。
    #[serde(default)]
    pub kind: String,
    /// 工作台所属场景（如「猎头协作」）；市场第一层按此分组。工具缺省为空。
    #[serde(default)]
    pub scenario: String,
    /// 打开入口：工作台装完/本机已有时从哪打开（file:// 本地路径或 http URL）。
    /// dsh 插件形态（client 插件）缺省为空——入口在 dsh web 进程内，无需壳打开。
    #[serde(default)]
    pub entry: Option<String>,
    /// 依赖的外部服务/底座（如 md-api 协作服务器），用户需自行启动。
    #[serde(default)]
    pub requires: Vec<String>,
    /// 验证证据（e2e 通过数/验证报告），区别于 verified 布尔（仅表「已通过」）。
    #[serde(default)]
    pub verify_evidence: Option<String>,
    /// 场景标签（市场按此分组浏览；与 RFC-23 capability / 2276 compose 同构的元数据层）。
    /// 旧清单无此字段时反序列化按空处理（#[serde(default)]）。
    #[serde(default)]
    pub tags: Vec<String>,
}

impl PluginInfo {
    /// 是否工作台形态（kind 字段显式声明；缺失/未知值按单件工具处理）
    pub fn is_workbench(&self) -> bool {
        self.kind == "workbench"
    }
}

/// 内置可信清单（v1 固定；v2 改为从 verified.json GitHub raw 拉取）。
/// 只放运行时验证 ✅ 的插件——这是差异化数据（286 个插件仅极少数有验证证据）。
/// 工作台（kind=workbench）与单件工具（tool）混列，市场展示时工作台优先。
pub fn builtin_marketplace() -> Vec<PluginInfo> {
    vec![
        // 工作台：猎头协作（本地资产形态，entry 指向本机 index.html；经 md-api MCP 协作）
        PluginInfo {
            id: "md-hr".to_string(),
            name: "猎头协作".to_string(),
            version: "2.0.0".to_string(),
            verified: true,
            desc: "候选人/岗位/客户/推荐管道/公海人才池/脱敏报告协作/简历门诊，本地优先 + md-api MCP 云端协作".to_string(),
            repo: Some("Desktop/md-hr".to_string()),
            kind: "workbench".to_string(),
            scenario: "猎头协作".to_string(),
            entry: Some("file:///C:/Users/Administrator/Desktop/md-hr/index.html".to_string()),
            requires: vec!["md-api（Desktop/md-api，默认 8080）".to_string()],
            verify_evidence: Some("e2e-v2..v5 共 95 项全绿（.test/）".to_string()),
            tags: vec!["猎头协作".to_string()],
        },
        PluginInfo {
            id: "@qing3a/dsh-event-auditor".to_string(),
            name: "事件审计".to_string(),
            version: "0.4".to_string(),
            verified: true,
            desc: "事件 waterfall 审计 + /audit 静态页（settings 热改）".to_string(),
            repo: Some("github.com/qing3a/dsh-event-auditor".to_string()),
            kind: String::new(),
            scenario: String::new(),
            entry: None,
            requires: vec![],
            verify_evidence: None,
            tags: vec!["事件审计".to_string()],
        },
        PluginInfo {
            id: "@dsh-external/dsh-tray".to_string(),
            name: "内置托盘增强".to_string(),
            version: "0.1".to_string(),
            verified: true,
            desc: "进程内托盘（气泡通知）。DSH 伴侣已自带托盘，一般无需安装".to_string(),
            repo: Some("github.com/qing3a/dsh-tray".to_string()),
            kind: String::new(),
            scenario: String::new(),
            entry: None,
            requires: vec![],
            verify_evidence: None,
            tags: vec!["托盘与效率".to_string()],
        },
    ]
}

/// 市场分组：工作台（kind=workbench）按场景（scenario）分组优先展示，
/// 单件工具（tool）按场景标签分组。分组只做浏览导航，「全部」列表仍每个商品恰好一次。
/// 工作台分组名取 scenario（缺省「工作台」）；工具分组名取 tags（无标签工具只进「全部」）。
pub fn marketplace_groups(catalog: &[PluginInfo]) -> Vec<(String, Vec<&PluginInfo>)> {
    let mut out: Vec<(String, Vec<&PluginInfo>)> = Vec::new();
    // 工作台优先（按场景分组）
    for p in catalog.iter().filter(|p| p.is_workbench()) {
        let label = if p.scenario.is_empty() { "工作台".to_string() } else { p.scenario.clone() };
        group_push(&mut out, label, p);
    }
    // 单件工具按标签分组（保留标签首次出现顺序；同一工具可属多个场景）
    for p in catalog.iter().filter(|p| !p.is_workbench()) {
        for t in &p.tags {
            group_push(&mut out, t.clone(), p);
        }
    }
    out
}

fn group_push<'a>(out: &mut Vec<(String, Vec<&'a PluginInfo>)>, label: String, p: &'a PluginInfo) {
    match out.iter_mut().find(|(g, _)| *g == label) {
        Some((_, items)) => items.push(p),
        None => out.push((label, vec![p])),
    }
}

/// 远程清单缓存：None = 未拉取或拉取失败（此时市场回退内置清单，离线不打扰）。
static REMOTE_CATALOG: OnceLock<Mutex<Option<Vec<PluginInfo>>>> = OnceLock::new();

/// 拉取 verified.json（GitHub raw）→ 合并进市场清单缓存。返回合并后的清单数量；
/// 失败返回 Err（调用方决定静默——内置清单仍可用，不阻塞）。
pub fn refresh_market_catalog() -> Result<usize, String> {
    let remote = fetch_remote_marketplace()?;
    let merged = merge_catalog(&builtin_marketplace(), &remote);
    let slot = REMOTE_CATALOG.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        *g = Some(merged);
    }
    Ok(remote.len())
}

/// 当前市场清单：有远程缓存 → 内置 + 远程合并；无（离线/未拉取）→ 纯内置兜底。
pub fn market_catalog() -> Vec<PluginInfo> {
    match REMOTE_CATALOG.get().and_then(|m| m.lock().ok()).and_then(|g| g.clone()) {
        Some(merged) => merged,
        None => builtin_marketplace(),
    }
}

/// 工作台打开信息：entry（file:// 本地资产 / http URL）+ 依赖服务清单。
/// 仅工作台形态有语义；普通工具/未收录条目返回 None（调用方提示不可用）。
/// 调用方打开入口前应提示 requires（外部服务需用户自启，壳不代启动）。
pub fn workbench_open(id: &str) -> Option<(String, Vec<String>)> {
    let p = market_catalog().into_iter().find(|p| p.id == id && p.is_workbench())?;
    Some((p.entry?, p.requires))
}

/// 合并规则：内置为基底（最小可信集合），远程按 id 覆盖或追加（远程是验证引擎的增量产出）。
fn merge_catalog(local: &[PluginInfo], remote: &[PluginInfo]) -> Vec<PluginInfo> {
    let mut out: Vec<PluginInfo> = local.to_vec();
    for r in remote {
        if let Some(slot) = out.iter_mut().find(|p| p.id == r.id) {
            *slot = r.clone();
        } else {
            out.push(r.clone());
        }
    }
    out
}

/// 远程清单：GET verified-plugins.json（GitHub raw）→ 解析为 PluginInfo 列表。
/// 清单结构上与 builtin_marketplace 同构（见插件市场文档），由验证引擎（dsh-plugin-verify）
/// 产出后提交到仓库。
/// 命名注意：与 DESIGN §5 的 verified.json（dsh **版本**已验证清单，channel A）区分——
/// 本文件是**插件**清单，固定 verified-plugins.json，避免两个语义共用一个 raw 文件。
fn fetch_remote_marketplace() -> Result<Vec<PluginInfo>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get("https://raw.githubusercontent.com/qing3a/dsh-desktop/main/verified-plugins.json")
        .send()
        .map_err(|e| format!("拉取 verified-plugins.json 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("verified-plugins.json 返回 {}", resp.status()));
    }
    resp.json().map_err(|e| format!("解析 verified-plugins.json 失败: {e}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, version: &str) -> PluginInfo {
        PluginInfo {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            verified: true,
            desc: String::new(),
            repo: None,
            kind: String::new(),
            scenario: String::new(),
            entry: None,
            requires: vec![],
            verify_evidence: None,
            tags: vec![],
        }
    }

    #[test]
    fn merge_remote_overrides_local_by_id() {
        let local = vec![p("@a/x", "1.0")];
        let remote = vec![p("@a/x", "2.0")];
        let merged = merge_catalog(&local, &remote);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].version, "2.0");
    }

    #[test]
    fn merge_remote_appends_new_ids() {
        let local = vec![p("@a/x", "1.0")];
        let remote = vec![p("@b/y", "0.1")];
        let merged = merge_catalog(&local, &remote);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].id, "@b/y");
    }

    #[test]
    fn merge_keeps_local_base_when_remote_empty() {
        let local = vec![p("@a/x", "1.0")];
        let merged = merge_catalog(&local, &[]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].version, "1.0");
    }

    /// 工作台按场景分组优先；工具按标签分组；无标签工具只进「全部」（不进分组）
    #[test]
    fn groups_workbench_by_scenario_first() {
        let mut wb = p("md-hr", "2.0.0");
        wb.kind = "workbench".to_string();
        wb.scenario = "猎头协作".to_string();
        let mut tool = p("@a/x", "1.0");
        tool.tags = vec!["事件审计".to_string()];
        let mut untagged = p("@b/y", "1.0");
        let catalog = vec![tool.clone(), untagged.clone(), wb.clone()];
        let groups = marketplace_groups(&catalog);
        assert_eq!(groups.len(), 2, "工作台场景 + 工具标签各一组，无标签工具不进分组");
        assert_eq!(groups[0].0, "猎头协作", "工作台分组排最前");
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[1].0, "事件审计");
        assert_eq!(groups[1].1.len(), 1);
    }

    /// 旧清单（无 kind/scenario 字段）反序列化按 tool 处理，不炸
    #[test]
    fn legacy_json_without_new_fields_deserializes() {
        let old = r#"[{"id":"@a/x","name":"x","version":"1.0","verified":true,"desc":"","repo":null,"tags":["t"]}]"#;
        let v: Vec<PluginInfo> = serde_json::from_str(old).unwrap();
        assert_eq!(v.len(), 1);
        assert!(!v[0].is_workbench());
        assert!(v[0].scenario.is_empty());
        assert!(v[0].entry.is_none());
    }

    /// 内置清单的猎头协作工作台：workbench_open 返回 entry + requires
    #[test]
    fn workbench_open_returns_entry_and_requires() {
        let (entry, requires) = workbench_open("md-hr").expect("内置清单应含猎头协作工作台");
        assert!(entry.starts_with("file:///"), "本地资产入口应为 file://");
        assert!(!requires.is_empty(), "猎头协作应声明 md-api 依赖");
    }

    /// 单件工具 / 未知 id：workbench_open 返回 None（无打开语义）
    #[test]
    fn workbench_open_none_for_tool_or_unknown() {
        assert!(workbench_open("@qing3a/dsh-event-auditor").is_none(), "单件工具无 entry");
        assert!(workbench_open("no-such-id").is_none());
    }

    /// 空清单分组为空（不产生空分组项）
    #[test]
    fn groups_empty_catalog() {
        assert!(marketplace_groups(&[]).is_empty());
    }
}
