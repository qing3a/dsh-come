//! 运行时目录布局与状态持久化。
//!
//! ```text
//! %LOCALAPPDATA%\dsh-desktop\
//! ├── config.json          # 启动器配置（端口/重启上限等）
//! ├── node\                # 捆绑 portable Node ≥22（自带 npm/npx/corepack）
//! ├── home\                # 启动器自己的 $DSH_HOME（profile/插件/配置隔离）
//! ├── state.json           # 当前锁定版本 / known_bad / 验证历史
//! └── logs\                # 滚动日志
//! ```
//!
//! 设计依据（DESIGN.md §3/§6）：DSH 包本体经 **npx 通道**（`npx @deepseek-ai/dsh@<ver>`）
//! 解析到 npm 缓存并执行，dsh-desktop 只维护一个版本号（state.current），
//! 不维护版本目录；数据/配置隔离在 %LOCALAPPDATA% 下。

use serde::{Deserialize, Serialize};
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

pub fn node_dir() -> PathBuf {
    root_dir().join("node")
}

/// 捆绑的 Node 可执行文件（portable Node 官方 zip 结构）
pub fn node_exe() -> PathBuf {
    node_dir().join("node.exe")
}

/// 捆绑 Node 内置的 npx-cli（`npx @pkg@ver` 通道入口；npx 非 exe，node 直启可绕过 .cmd 包装）
pub fn npx_cli_js() -> PathBuf {
    node_dir().join("node_modules").join("npm").join("bin").join("npx-cli.js")
}

/// 捆绑 Node 内置的 npm-cli（用于 ensure_pnpm：`npm install -g pnpm --prefix`）
pub fn npm_cli_js() -> PathBuf {
    node_dir().join("node_modules").join("npm").join("bin").join("npm-cli.js")
}

/// 启动器自己的 $DSH_HOME（契约 C3）：profile/插件/配置全隔离
pub fn home_dir() -> PathBuf {
    root_dir().join("home")
}

pub fn logs_dir() -> PathBuf {
    root_dir().join("logs")
}

/// 引擎滚动日志文件（stdout/stderr 重定向，防管道阻塞 + 留诊断）
pub fn engine_log() -> PathBuf {
    logs_dir().join("engine.log")
}

pub fn state_path() -> PathBuf {
    root_dir().join("state.json")
}

/// 测试专用：串行化会改写进程级 `DSH_DESKTOP_HOME` 的测试（updater 更新-回滚闭环 /
/// tray 窗口几何记忆），避免并行测试踩踏 state.json/config.json 的实际落盘位置。
#[cfg(test)]
pub(crate) static DSH_HOME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 壳维护的 patch overlay 路径（home\come.patch.yml）：spawn dsh web 时经 `--patch`
/// 传入（CLI 契约面用法，不写 profile 内部文件）。当前内容：dsh-market 安装后
/// 禁用其 detached 一键重启（重启归 supervisor 管，防止绕过崩溃自愈/退避/日志）。
pub fn come_patch_path() -> PathBuf {
    home_dir().join("come.patch.yml")
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

// ---------- state.json ----------

/// 版本事件（验证历史，供审计/回滚诊断）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEvent {
    pub ver: String,
    pub ok: bool,
    pub at: String,
    pub detail: String,
}

/// 启动器持久状态：当前锁定版本 + 上一版本（回滚目标）+ 待确认更新 + 验证历史
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// 当前锁定版本（如 "0.1.0-rc.6"）；None = 尚未安装任何版本
    pub current: Option<String>,
    /// 上一个锁定的可用版本（回滚目标）：升级/应用更新时把旧 current 记于此，
    /// 托盘「回滚到 vX」切回。None = 无历史（首次安装）。
    /// #[serde(default)]：旧 state.json 无此字段时不炸（缺字段反序列化为 None）。
    #[serde(default)]
    pub previous: Option<String>,
    /// 已验证通过、待用户确认应用的版本（托盘菜单「应用更新」触发切换）
    pub pending: Option<String>,
    /// 验证失败的版本（切换/回滚时跳过）
    pub known_bad: Vec<String>,
    /// 验证历史（最近事件，最多 50 条）
    pub history: Vec<VersionEvent>,
}

pub fn load_state() -> State {
    match std::fs::read_to_string(state_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => State::default(),
    }
}

pub fn save_state(s: &State) -> std::io::Result<()> {
    let p = state_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(s)?)
}

pub fn record_event(s: &mut State, ev: VersionEvent) {
    s.history.push(ev);
    if s.history.len() > 50 {
        s.history.drain(..s.history.len() - 50);
    }
}

/// 确保目录骨架存在（首次运行补齐）
pub fn ensure_layout() -> std::io::Result<()> {
    for d in [
        home_dir(),
        logs_dir(),
    ] {
        std::fs::create_dir_all(d)?;
    }
    ensure_come_patch()?;
    Ok(())
}

// ---------- 捆绑 Node 自举安装 ----------

/// 目标 Node 大版本（DSH 要求 Node >= 22）。固定已知良好版本保证确定性；
/// 需要跟随时手动升，与 dsh 版本解耦（Node 22 是 LTS，长期可用）。
const NODE_VERSION: &str = "22.14.0";

/// 下载候选：官方优先，npmmirror 镜像兜底（国内网络 nodejs.org 可能慢/被墙）
fn node_zip_candidates() -> Vec<String> {
    let file = format!("node-v{NODE_VERSION}-win-x64.zip");
    vec![
        format!("https://nodejs.org/dist/v{NODE_VERSION}/{file}"),
        format!("https://npmmirror.com/mirrors/node/v{NODE_VERSION}/{file}"),
    ]
}

fn append_log(line: &str) {
    crate::supervisor::log(line);
}

/// 下载 node zip 到目标路径；逐个候选尝试，成功即返回
/// 下载百分比（0-100；无总长信息时 None）
fn download_percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let t = total?;
    if t == 0 {
        return Some(0);
    }
    Some(((downloaded * 100) / t).min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::download_percent;

    #[test]
    fn percent_basic() {
        assert_eq!(download_percent(50, Some(100)), Some(50));
        assert_eq!(download_percent(0, Some(100)), Some(0));
        assert_eq!(download_percent(100, Some(100)), Some(100));
    }

    #[test]
    fn percent_capped_and_unknown() {
        // 超过总量（服务端长度偏差）封顶 100
        assert_eq!(download_percent(150, Some(100)), Some(100));
        // 无总长信息（无 content-length）→ None，调用方退回阶段提示
        assert_eq!(download_percent(50, None), None);
        // 总量 0 防除零
        assert_eq!(download_percent(0, Some(0)), Some(0));
    }
}

/// 下载 node zip 到目标路径；逐个候选尝试，成功即返回。
/// 流式下载（防 30MB 一次性进内存）+ 百分比进度（stage 状态行实时显示）。
fn download_node_zip(zip_path: &std::path::Path) -> Result<(), String> {
    use std::io::{Read, Write};
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // 流式读 body：墙内 30MB 可能慢，给足时间
        .build()
        .map_err(|e| e.to_string())?;
    let mut last_err = String::new();
    for url in node_zip_candidates() {
        append_log(&format!("下载 Node（{url}）…"));
        match client.get(&url).send() {
            Ok(resp) if resp.status().is_success() => {
                let total = resp.content_length();
                if let Some(dir) = zip_path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let mut f = match std::fs::File::create(zip_path) {
                    Ok(f) => f,
                    Err(e) => {
                        last_err = format!("创建文件失败: {e}");
                        continue;
                    }
                };
                let mut reader = resp; // blocking::Response 实现 Read
                let mut buf = vec![0u8; 256 * 1024];
                let mut downloaded: u64 = 0;
                let mut last_pct = 0u8;
                let mut last_tick = std::time::Instant::now();
                let mut io_err: Option<String> = None;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            downloaded += n as u64;
                            if f.write_all(&buf[..n]).is_err() {
                                io_err = Some("写入 zip 失败".to_string());
                                break;
                            }
                            // 进度节流：跨百分比且 ≥200ms 才更新（避免高频刷共享状态）
                            let pct = download_percent(downloaded, total).unwrap_or(0);
                            if pct > last_pct && last_tick.elapsed().as_millis() >= 200 {
                                last_pct = pct;
                                last_tick = std::time::Instant::now();
                                crate::supervisor::set_stage(&format!("首次安装：下载 Node {pct}%…"));
                            }
                        }
                        Err(e) => {
                            io_err = Some(format!("下载中断: {e}"));
                            break;
                        }
                    }
                }
                if let Some(err) = io_err {
                    last_err = err;
                    continue;
                }
                if f.flush().is_err() {
                    last_err = "写入 zip 失败（flush）".to_string();
                    continue;
                }
                append_log(&format!("Node 下载完成（{} MB）", downloaded / 1024 / 1024));
                crate::supervisor::set_stage("首次安装：解压 Node…");
                return Ok(());
            }
            Ok(resp) => last_err = format!("HTTP {}", resp.status()),
            Err(e) => last_err = format!("连接失败: {e}"),
        }
    }
    Err(format!("所有下载源均失败（{last_err}）"))
}

/// 解压 zip 到目录（纯 Rust zip crate；Windows 路径安全由 mangled_name 保证）
fn unzip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开 zip 失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("解析 zip 失败: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        let outpath = dest.join(entry.mangled_name());
        if entry.is_dir() {
            std::fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&outpath).map_err(|e| format!("创建 {} 失败: {e}", outpath.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("写出 {} 失败: {e}", outpath.display()))?;
        }
    }
    Ok(())
}

/// 确保捆绑 Node 就绪：node.exe + npx-cli.js 存在则 Ok；
/// 否则自动下载官方 portable Node 并解压到 node\（等效「一条命令搞定」，小白无感）。
/// 幂等：已就绪直接返回。失败统一写引擎日志（stderr 在无控制台会话会被隐藏）。
pub fn ensure_node() -> Result<(), String> {
    match ensure_node_inner() {
        Ok(()) => Ok(()),
        Err(e) => {
            append_log(&format!("Node 自举安装失败: {e}"));
            Err(e)
        }
    }
}

fn ensure_node_inner() -> Result<(), String> {
    if node_exe().is_file() && npx_cli_js().is_file() {
        return Ok(());
    }
    if node_exe().is_file() && !npx_cli_js().is_file() {
        return Err(format!(
            "node.exe 存在但 npx-cli.js 缺失（{}），portable Node 不完整，请清空 node\\ 后重试",
            npx_cli_js().display()
        ));
    }

    append_log("首次运行：开始自动安装 Node（约 30MB）…");
    crate::supervisor::set_stage("首次安装：下载 Node（约 30MB）…");
    let root = root_dir();
    let zip_path = root.join(format!("node-v{NODE_VERSION}-win-x64.zip"));
    let tmp = root.join("node-tmp");
    let _ = std::fs::remove_dir_all(&tmp);

    download_node_zip(&zip_path)?;
    crate::supervisor::set_stage("首次安装：解压 Node…");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    unzip(&zip_path, &tmp)?;

    // 解压目录：node-tmp\node-v<ver>-win-x64\ → 内容移动到 node\
    let extracted = tmp.join(format!("node-v{NODE_VERSION}-win-x64"));
    if !extracted.join("node.exe").is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("解压后未找到预期目录（{}），安装失败", extracted.display()));
    }
    let node_dir_path = node_dir();
    if node_dir_path.exists() {
        let _ = std::fs::remove_dir_all(&node_dir_path);
    }
    std::fs::rename(&extracted, &node_dir_path).map_err(|e| format!("移动 Node 失败: {e}"))?;

    // 清理临时文件
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_file(&zip_path);

    if !node_exe().is_file() || !npx_cli_js().is_file() {
        return Err("Node 安装后校验失败（node.exe / npx-cli.js 缺失）".to_string());
    }
    append_log(&format!(
        "Node {NODE_VERSION} 已就绪: {}",
        node_dir_path.display()
    ));
    crate::supervisor::set_stage("");
    Ok(())
}
