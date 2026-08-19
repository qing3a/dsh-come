//! 环境探测与安装：node/npm/dsh/winget 探测 + 异步安装（winget/npm）+ 安装状态。
//!
//! 原则（2026-08-19 用户拍板）：**不走 npx 临时拉取**——dsh 缺失就正常安装
//! （`npm install -g @deepseek-ai/dsh`）；node 缺失用 winget 装 LTS（会弹一次 UAC）。
//! PATH 探测合并：进程 PATH + 注册表 User/System PATH + `npm prefix -g` 全局目录
//! （npm install -g 后不重启进程也能找到新装的 dsh）。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 安装任务状态（管理页轮询）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct InstallState {
    /// 正在安装的目标："node" / "dsh" / None
    pub kind: Option<String>,
    pub running: bool,
    pub ok: Option<bool>,
    pub msg: String,
}

static INSTALL: OnceLock<Mutex<InstallState>> = OnceLock::new();

fn install_slot() -> &'static Mutex<InstallState> {
    INSTALL.get_or_init(|| Mutex::new(InstallState::default()))
}

/// 当前安装状态快照。
pub fn install_state() -> InstallState {
    install_slot().lock().map(|g| g.clone()).unwrap_or_default()
}

fn set_install(st: InstallState) {
    if let Ok(mut g) = install_slot().lock() {
        *g = st;
    }
}

// ---------- PATH 探测（进程 + 注册表） ----------

/// 读注册表 PATH（User / System Environment 键）。返回原样字符串列表。
#[cfg(target_os = "windows")]
fn registry_path_values() -> Vec<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_QUERY_VALUE, REG_EXPAND_SZ, REG_SZ,
    };
    const KEYS: &[(&str, &str)] = &[
        ("Software\\Microsoft\\Windows\\CurrentVersion\\Environment", "HKCU"),
        ("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment", "HKLM"),
    ];
    let mut out = Vec::new();
    unsafe {
        for (sub, root) in KEYS {
            let mut hkey: HKEY = 0;
            let wide: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
            let root_key = if *root == "HKCU" {
                HKEY_CURRENT_USER
            } else {
                HKEY_LOCAL_MACHINE
            };
            if RegOpenKeyExW(root_key, wide.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) != 0 {
                continue;
            }
            let name: Vec<u16> = "PATH".encode_utf16().chain(std::iter::once(0)).collect();
            let mut buf = [0u8; 8192];
            let mut len = buf.len() as u32;
            let mut kind = 0u32;
            let rc = RegQueryValueExW(
                hkey,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buf.as_mut_ptr(),
                &mut len,
            );
            if rc == 0 && (kind == REG_SZ || kind == REG_EXPAND_SZ) {
                let s = String::from_utf8_lossy(&buf[..len as usize]).to_string();
                out.push(s);
            }
            RegCloseKey(hkey);
        }
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn registry_path_values() -> Vec<String> {
    Vec::new()
}

/// 展开常见环境变量占位（%SystemRoot%/%USERPROFILE%/%USERNAME%），其余原样保留。
fn expand_common_vars(s: &str) -> String {
    let mut out = s.to_string();
    let map = [
        ("%SystemRoot%", std::env::var_os("SystemRoot").map(|v| v.to_string_lossy().into_owned())),
        ("%USERPROFILE%", std::env::var_os("USERPROFILE").map(|v| v.to_string_lossy().into_owned())),
        ("%USERNAME%", std::env::var_os("USERNAME").map(|v| v.to_string_lossy().into_owned())),
    ];
    for (k, v) in map {
        if let Some(v) = v {
            out = out.replace(k, &v);
        }
    }
    out
}

/// 基础候选 PATH 目录（进程 PATH + 注册表 PATH），**不含** npmpfx——避免与 npm_prefix 互相递归。
fn base_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |d: PathBuf| {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    if let Ok(p) = std::env::var("PATH") {
        for d in std::env::split_paths(&p) {
            push(d);
        }
    }
    for raw in registry_path_values() {
        for d in std::env::split_paths(&expand_common_vars(&raw)) {
            push(d);
        }
    }
    dirs
}

/// 合并后的候选 PATH 目录（进程 PATH + 注册表 PATH + npm 全局 bin），去重保序。
pub fn env_dirs() -> Vec<PathBuf> {
    let mut dirs = base_dirs();
    if let Some(prefix) = npm_prefix() {
        if !dirs.contains(&prefix) {
            dirs.push(prefix);
        }
    }
    dirs
}

/// `npm prefix -g` 输出目录（npm 存在时）。**只用基础 PATH 找 npm**（避免递归）；
/// 结果缓存 30s（npm 全局目录在 dsh 安装前后不变；node 安装后经 invalidate_cache 失效）。
pub fn npm_prefix() -> Option<PathBuf> {
    const TTL: Duration = Duration::from_secs(30);
    let slot = NPMPFX_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        if let Some((at, p)) = g.as_ref() {
            if at.elapsed() < TTL {
                return Some(p.clone());
            }
        }
        let p = compute_npm_prefix();
        if let Some(pp) = &p {
            *g = Some((std::time::Instant::now(), pp.clone()));
        }
        return p;
    }
    None
}

fn compute_npm_prefix() -> Option<PathBuf> {
    let npm = find_in(&base_dirs(), "npm")?;
    let mut cmd = std::process::Command::new(&npm);
    cmd.args(["prefix", "-g"]);
    crate::supervisor::hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

static NPMPFX_CACHE: OnceLock<Mutex<Option<(std::time::Instant, PathBuf)>>> = OnceLock::new();

fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    for dir in dirs {
        for ext in [".exe", ".cmd", ".bat", ""] {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 在合并 PATH 里找可执行命令（.exe/.cmd/.bat/无扩展名）。
pub fn which(name: &str) -> Option<PathBuf> {
    find_in(&env_dirs(), name)
}

/// 命令版本探测（`<cmd> --version` 首行）。
fn version_of(cmd: &str) -> Option<String> {
    let exe = which(cmd)?;
    let mut c = std::process::Command::new(&exe);
    c.arg("--version");
    crate::supervisor::hide_window(&mut c);
    let out = c.output().ok()?;
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

/// 存在性探测：纯文件查找（不 spawn 进程，快）。版本展示单独跑 version_of（只对 node/dsh）。
pub fn node_installed() -> bool {
    which("node").is_some()
}
pub fn npm_installed() -> bool {
    which("npm").is_some()
}
pub fn dsh_installed() -> bool {
    which("dsh").is_some()
}

/// 综合探测快照（管理页展示）。带 5s TTL 缓存——探测要 spawn npm/node 等进程，
/// 管理页每 2s 轮询时不能每次都跑；安装完成后 invalidate_cache 失效。
pub fn probe() -> serde_json::Value {
    const TTL: Duration = Duration::from_secs(5);
    let cache = PROBE_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = cache.lock() {
        if let Some((at, v)) = g.as_ref() {
            if at.elapsed() < TTL {
                return v.clone();
            }
        }
        let v = probe_uncached();
        *g = Some((std::time::Instant::now(), v.clone()));
        return v;
    }
    probe_uncached()
}

/// 安装完成后使探测/路径/版本缓存失效（node/dsh 刚装好/更新，需重探测）。
pub fn invalidate_cache() {
    if let Ok(mut g) = PROBE_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *g = None;
    }
    if let Ok(mut g) = NPMPFX_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *g = None;
    }
    if let Ok(mut g) = NPM_VIEW_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *g = None;
    }
}

fn probe_uncached() -> serde_json::Value {
    // 存在性 = 纯文件查找（快）；版本只对 node/dsh 跑（npm/winget 的 cmd 链 spawn 慢，不跑）
    serde_json::json!({
        "node": if which("node").is_some() { version_of("node").or(Some("已安装".to_string())) } else { None },
        "npm": if which("npm").is_some() { Some("已安装".to_string()) } else { None },
        "dsh": if which("dsh").is_some() { version_of("dsh").or(Some("已安装".to_string())) } else { None },
        "winget": if which("winget").is_some() { Some("已安装".to_string()) } else { None },
    })
}

static PROBE_CACHE: OnceLock<Mutex<Option<(std::time::Instant, serde_json::Value)>>> =
    OnceLock::new();

// ---------- 异步安装 ----------

/// 触发异步安装（node / dsh）。已在安装中 → 返回 Err。
/// 执行结果写入 InstallState（管理页 /api/install/status 轮询）。
pub fn start_install(kind: &str) -> Result<(), String> {
    let kind = kind.to_string();
    start_install_boxed(kind.clone(), move || match kind.as_str() {
        "node" => install_node(),
        "dsh" => install_dsh(None),
        other => (false, format!("未知安装目标: {other}")),
    })
}

/// 触发 dsh 安装/更新/指定版本（spec：`latest` 强制更新到最新；`0.1.0-rc.5` 装指定版本）。
/// 复用同一安装单任务 slot（安装中互斥）。
pub fn start_dsh_install(spec: &str) -> Result<(), String> {
    let spec = spec.to_string();
    start_install_boxed("dsh".to_string(), move || install_dsh(Some(&spec)))
}

fn start_install_boxed(
    kind: String,
    f: impl FnOnce() -> (bool, String) + Send + 'static,
) -> Result<(), String> {
    let running = install_slot().lock().map_err(|e| e.to_string())?.running;
    if running {
        let k = install_slot().lock().map_err(|e| e.to_string())?.kind.clone();
        return Err(format!("已有安装任务进行中（{}）", k.unwrap_or_default()));
    }
    set_install(InstallState {
        kind: Some(kind.clone()),
        running: true,
        ok: None,
        msg: format!("开始安装 {kind} …"),
    });
    std::thread::spawn(move || {
        let (ok, msg) = f();
        // 安装结束：失效探测/版本缓存（新装的 node/dsh 要能被立即识别）
        invalidate_cache();
        set_install(InstallState {
            kind: Some(kind.clone()),
            running: false,
            ok: Some(ok),
            msg,
        });
    });
    Ok(())
}

/// winget 静默安装 Node.js LTS。返回 (成功?, 结果文案)。
fn install_node() -> (bool, String) {
    if node_installed() {
        return (true, "Node.js 已安装，无需重复安装".to_string());
    }
    let Some(winget) = which("winget") else {
        return (
            false,
            "未找到 winget。请手动安装 Node.js（https://nodejs.org 下载 LTS 安装包），装完刷新管理页。".to_string(),
        );
    };
    let mut cmd = std::process::Command::new(&winget);
    cmd.args([
        "install", "-e", "--id", "OpenJS.NodeJS.LTS",
        "--silent", "--accept-package-agreements", "--accept-source-agreements",
        "--disable-interactivity",
    ]);
    crate::supervisor::hide_window(&mut cmd);
    match cmd.output() {
        Ok(out) => {
            let tail = tail_text(&out.stdout, &out.stderr);
            if out.status.success() && node_installed() {
                (true, format!("Node.js 安装成功。{tail}"))
            } else {
                (
                    false,
                    format!("Node.js 安装失败（退出码 {:?}）。{tail}", out.status.code()),
                )
            }
        }
        Err(e) => (false, format!("无法启动 winget: {e}")),
    }
}

/// 安装/更新/退回 dsh（npm install -g）。
/// `spec`：None=缺省安装（已装则跳过）；Some("latest")=强制更新到最新；Some(版本号)=装指定版本。
/// 关键修复（2026-08-20）：npm 必须用「与 dsh 命令同目录」的那一个——否则 npm install -g 会装到
/// %AppData%\npm，而 PATH 里另一套 node 生态的 dsh 排前面，导致「装完版本没变」；装完还要
/// **验证 dsh --version == 目标**才算成功，不再被 PATH 里旧 dsh 掩盖（旧判断只查存在，恒误报成功）。
fn install_dsh(spec: Option<&str>) -> (bool, String) {
    if !npm_installed() {
        return (false, "未找到 npm，请先安装 Node.js（管理页「安装 Node」）".to_string());
    }
    if spec.is_none() && dsh_installed() {
        return (true, "dsh 已安装，无需重复安装".to_string());
    }
    // None → 装默认 latest；Some(spec) → @latest 或 @<版本号>
    let pkg = match spec {
        None => "@deepseek-ai/dsh".to_string(),
        Some(s) => format!("@deepseek-ai/dsh@{s}"),
    };
    let Some(npm) = npm_for_dsh().or_else(|| which("npm")) else {
        return (false, "未找到 npm 命令".to_string());
    };
    let mut cmd = std::process::Command::new(&npm);
    cmd.args(["install", "-g", &pkg]);
    crate::supervisor::hide_window(&mut cmd);
    // npm 静默期输出少，给足超时（首次下载依赖树可能较慢）
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("无法启动 npm: {e}")),
    };
    let out = child.wait_with_output();
    match out {
        Ok(out) => {
            let tail = tail_text(&out.stdout, &out.stderr);
            if !out.status.success() {
                return (
                    false,
                    format!(
                        "dsh 安装失败（退出码 {:?}，{pkg}）。{tail} 若重试仍失败，请先在终端执行 `npm uninstall -g @deepseek-ai/dsh` 再安装。",
                        out.status.code()
                    ),
                );
            }
            // 版本验证：装完 dsh --version 应与目标一致（防 PATH 解析到旧 dsh 造成假成功）
            let ver = version_of("dsh");
            match spec {
                Some(want) if want != "latest" && ver.as_deref() == Some(want) => (
                    true,
                    format!("dsh 更新成功（{pkg}，现为 {}）。{tail}", ver.unwrap_or_default()),
                ),
                Some(want) => (
                    false,
                    format!(
                        "npm 安装结束但 dsh 版本未变为目标（当前 {:?}，期望 {want}；npm={}，dsh 解析自 {}）。{tail} 可能是 PATH 中另一套 node 生态的 dsh 排在前，请检查 PATH 或在终端执行 `npm uninstall -g @deepseek-ai/dsh` 后重试。",
                        ver,
                        npm.display(),
                        which("dsh").map(|p| p.display().to_string()).unwrap_or_default()
                    ),
                ),
                None => (true, format!("dsh 安装成功（{pkg}）。{tail}")),
            }
        }
        Err(e) => (false, format!("等待 npm 结束失败: {e}")),
    }
}

/// 与 dsh 命令同目录的 npm：确保 `npm install -g` 的落点就是 PATH 解析 `dsh` 的那个全局目录。
/// （避免装到 %AppData%\npm 而 C:\tools\nodejs 等另一套 node 的 dsh 排前面 → 装完版本不变）
fn npm_for_dsh() -> Option<PathBuf> {
    let dsh = which("dsh")?;
    let dir = dsh.parent()?;
    for ext in [".exe", ".cmd", ""] {
        let p = dir.join(format!("npm{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

// ---------- npm registry 版本查询（dsh 更新检查） ----------

static NPM_VIEW_CACHE: OnceLock<Mutex<Option<(std::time::Instant, String, serde_json::Value)>>> =
    OnceLock::new();

/// `npm view @deepseek-ai/dsh <field>`（field: version=latest | versions=全部），60s TTL 缓存。
/// 网络/命令失败 → None（UI 显示「查询失败/离线」）。
fn npm_view(field: &str) -> Option<serde_json::Value> {
    const TTL: Duration = Duration::from_secs(60);
    let cache = NPM_VIEW_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = cache.lock() {
        // 缓存按 field 区分（version 是字符串、versions 是数组，不能共用一个槽）
        if let Some((at, f, v)) = g.as_ref() {
            if f == field && at.elapsed() < TTL {
                return Some(v.clone());
            }
        }
        let v = npm_view_uncached(field);
        if let Some(vv) = &v {
            *g = Some((std::time::Instant::now(), field.to_string(), vv.clone()));
        }
        return v;
    }
    npm_view_uncached(field)
}

fn npm_view_uncached(field: &str) -> Option<serde_json::Value> {
    let npm = which("npm")?;
    let mut cmd = std::process::Command::new(&npm);
    cmd.args(["view", "@deepseek-ai/dsh", field]);
    if field == "versions" || field == "dist-tags" {
        cmd.arg("--json");
    }
    crate::supervisor::hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return None;
    }
    if field == "versions" || field == "dist-tags" {
        serde_json::from_str::<serde_json::Value>(&s).ok()
    } else {
        Some(serde_json::Value::String(s))
    }
}

/// dsh 最新发布版 = 版本列表最后一项（rc 包的 dist-tag `latest` 常滞后于实际发布——
/// 如 rc.8 已发布但 latest 仍指 rc.7）；列表不可得时退回 dist-tag latest。
pub fn dsh_latest() -> Option<String> {
    let versions = dsh_versions();
    if let Some(v) = versions.last() {
        return Some(v.clone());
    }
    npm_view("version").and_then(|v| v.as_str().map(String::from))
}

/// npm dist-tags（{latest, next, …}）：latest 是官方 stable tag，next 是预发布候选。
pub fn dsh_dist_tags() -> serde_json::Value {
    npm_view("dist-tags").unwrap_or(serde_json::Value::Null)
}

/// dsh 全部已发布版本（npm view versions，升序）。
pub fn dsh_versions() -> Vec<String> {
    npm_view("versions")
        .and_then(|v| v.as_array().cloned())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// dsh 版本状态（管理页「dsh 版本」卡片）：
/// current / latest（最新发布）/ latest_tag（npm stable tag）/ tags / has_update / versions。
pub fn dsh_versions_json() -> serde_json::Value {
    let current = version_of("dsh");
    let latest = dsh_latest();
    let tags = dsh_dist_tags();
    let latest_tag = tags.get("latest").and_then(|v| v.as_str()).map(String::from);
    let versions = dsh_versions();
    let has_update = match (&latest, &current) {
        (Some(l), Some(c)) => l != c,
        (Some(_), None) => true, // 未安装 dsh（或探测失败）但有最新版
        _ => false,
    };
    serde_json::json!({
        "current": current,
        "latest": latest,
        "latest_tag": latest_tag,
        "tags": tags,
        "has_update": has_update,
        "versions": versions,
    })
}

/// stdout/stderr 尾部文本（各取末尾 300 字，去空行），用于错误回显。
pub fn tail_text(stdout: &[u8], stderr: &[u8]) -> String {
    let mut parts = Vec::new();
    for bytes in [stdout, stderr] {
        let s = String::from_utf8_lossy(bytes);
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = lines.len().saturating_sub(6);
        parts.push(lines[start..].join(" | "));
    }
    let joined = parts.join(" ").trim().to_string();
    if joined.is_empty() {
        String::new()
    } else {
        format!("详情: {joined}")
    }
}
