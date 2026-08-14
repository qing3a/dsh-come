//! DSH 版本管理（npx 通道）：registry 检查 → 冒烟验证 → 切换/回滚。
//!
//! 设计依据（DESIGN.md §5/§6）：验证通过才切换，否则自动回滚，小白无感知。
//! - 更新源：npm registry（dist-tags.latest），经 `npx @deepseek-ai/dsh@<ver>` 通道执行——
//!   下载/缓存/解析全交给 npm 生态，壳只维护 state.current 一个版本号（npx 通道，非 npx 盲用：
//!   版本号由本模块验证后才写入 state.current，`--yes` 由 supervisor::npx_argv 统一钉死）
//! - 冒烟 v1（轻量）：npx 起 dsh web 到临时端口 → HTTP 200 → 杀树干净退出（契约 C1/C2）
//! - 切换：state.current 更新；失败 → known_bad 记录 + 保留旧版本号回滚
//!   （回滚依赖 npx 缓存：缓存里留有旧版则离线可用；缓存被清则需重下——换取实现大幅简化）

use crate::config::AppConfig;
use crate::runtime;
use crate::supervisor;
use std::process::{Command, Stdio};
use std::time::Duration;

/// registry 元数据（只取 dist-tags.latest）
#[derive(serde::Deserialize)]
struct RegistryMeta {
    #[serde(rename = "dist-tags")]
    dist_tags: DistTags,
}

#[derive(serde::Deserialize)]
struct DistTags {
    latest: String,
}

/// 更新结果（托盘状态行 / 日志展示）
#[derive(Debug, Clone)]
pub enum UpdateResult {
    /// 已是最新（无更新）
    UpToDate(String),
    /// 发现新版本且已验证通过，待用户确认应用（state.pending 已存）
    Pending(String),
    Failed(String),
}

fn append_log(line: &str) {
    supervisor::log(line);
}

/// GET https://registry.npmjs.org/@deepseek-ai/dsh → dist-tags.latest
pub fn latest_from_registry() -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get("https://registry.npmjs.org/@deepseek-ai/dsh")
        .send()
        .map_err(|e| format!("查询 npm registry 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("npm registry 返回 {}", resp.status()));
    }
    let meta: RegistryMeta = resp.json().map_err(|e| format!("解析 registry 响应失败: {e}"))?;
    Ok(meta.dist_tags.latest)
}

/// 找一个空闲高位端口（冒烟验证用，避免撞主端口 3080）
fn find_free_port() -> u16 {
    for port in 3100..=3199 {
        if !supervisor::http_ok(port, 300) {
            return port;
        }
    }
    3081
}

/// 冒烟验证 v1（轻量）：npx 起 dsh web 到临时端口 → HTTP 200 → 杀树干净退出。
/// 首次会经 npx 下载 dsh 包（可能较慢），故轮询上限放宽到 120s。
/// v2 扩展为 mock-llm 全 waterfall（收编 dsh-plugin-verify 引擎，见 DESIGN §5）。
fn smoke_test(ver: &str) -> Result<(), String> {
    let port = find_free_port();
    // npx 通道启动（契约 C1/C3：DSH_HOME 隔离；--yes + 钉版由 npx_argv 保证）
    let node = runtime::node_exe();
    let npx = runtime::npx_cli_js();
    let home = runtime::home_dir();
    let mut cmd = Command::new(&node);
    cmd.arg(&npx)
        .args(supervisor::npx_argv(ver, "127.0.0.1", port))
        .current_dir(&home)
        .env("DSH_HOME", &home);
    supervisor::hide_window(&mut cmd);
    // 日志进临时文件，防管道写满
    let logp = runtime::logs_dir().join(format!("smoke-{ver}.log"));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&logp)
        .map_err(|e| e.to_string())?;
    cmd.stdout(Stdio::from(f.try_clone().map_err(|e| e.to_string())?));
    cmd.stderr(Stdio::from(f));
    let mut child = cmd.spawn().map_err(|e| format!("冒烟启动失败: {e}"))?;
    let pid = child.id();

    // 轮询 HTTP 200（与启动超时一致；npx 首次下载依赖树可能较慢）
    let timeout = crate::config::load().startup_timeout_secs;
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        if supervisor::http_ok(port, 1000) {
            ok = true;
            break;
        }
        if child.try_wait().ok().flatten().is_some() {
            break; // 进程先退了 = 失败
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // 杀进程树（taskkill /T，防残留 node 占端口）
    let mut tk = Command::new("taskkill");
    tk.args(["/T", "/F", "/PID", &pid.to_string()]);
    supervisor::hide_window(&mut tk);
    let _ = tk.status();
    let _ = child.wait();

    if ok {
        append_log(&format!("冒烟验证通过: {ver}（HTTP 200 @:{port}）"));
        Ok(())
    } else {
        let tail = std::fs::read_to_string(&logp).unwrap_or_default();
        let tail = tail.lines().rev().take(10).collect::<Vec<_>>().join("\n");
        Err(format!("冒烟验证失败: {ver}（未见 HTTP 200，日志尾部：\n{tail}）"))
    }
}

/// 检查更新（主入口，托盘「检查更新」触发）。
/// npx 通道无需预安装：直接冒烟验证新版；**验证通过只存 pending，不切换**——
/// 用户从托盘菜单「应用更新」确认后才切换（更新前询问，尽量不打断）。
/// 阻塞执行；调用方应放后台线程。
pub fn check_and_install(_cfg: &AppConfig) -> UpdateResult {
    let latest = match latest_from_registry() {
        Ok(v) => v,
        Err(e) => return UpdateResult::Failed(e),
    };
    let mut state = runtime::load_state();
    let current = state.current.clone();

    // 已是最新
    if current.as_deref() == Some(latest.as_str()) {
        return UpdateResult::UpToDate(latest);
    }
    // known_bad 里已有该版本 → 明确失败（不静默重验）
    if state.known_bad.iter().any(|v| v == &latest) {
        return UpdateResult::Failed(format!("{latest} 已被标记为不可用（此前验证失败），跳过"));
    }
    // 已是待确认状态 → 不重复验证
    if state.pending.as_deref() == Some(latest.as_str()) {
        return UpdateResult::Pending(latest);
    }

    append_log(&format!("发现新版本: current={current:?} → latest={latest}"));

    // 冒烟验证（npx 会缓存下载该版本）
    match smoke_test(&latest) {
        Ok(()) => {
            state.pending = Some(latest.clone());
            record(&mut state, &latest, true, "冒烟验证通过，待用户确认");
            let _ = runtime::save_state(&state);
            append_log(&format!("新版本 {latest} 已验证，等待用户确认应用"));
            UpdateResult::Pending(latest)
        }
        Err(e) => {
            if !state.known_bad.iter().any(|v| v == &latest) {
                state.known_bad.push(latest.clone());
            }
            record(&mut state, &latest, false, &e);
            let _ = runtime::save_state(&state);
            append_log(&format!("版本 {latest} 验证失败，保留当前 {current:?}（回滚通道）"));
            UpdateResult::Failed(e)
        }
    }
}

/// 应用待确认的更新：把 state.pending 提升为 current（用户从托盘「应用更新」确认后调用）。
/// 切换后需重启 dsh 引擎生效（托盘「重启引擎」）。
pub fn apply_pending() -> Result<String, String> {
    let mut state = runtime::load_state();
    let ver = state.pending.clone().ok_or_else(|| "没有待应用的更新".to_string())?;
    state.current = Some(ver.clone());
    state.pending = None;
    record(&mut state, &ver, true, "用户确认应用");
    runtime::save_state(&state).map_err(|e| e.to_string())?;
    append_log(&format!("已切换到 v{ver}（重启 dsh 引擎生效）"));
    Ok(ver)
}

fn record(state: &mut runtime::State, ver: &str, ok: bool, detail: &str) {
    runtime::record_event(
        state,
        runtime::VersionEvent {
            ver: ver.to_string(),
            ok,
            at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            detail: detail.to_string(),
        },
    );
}

/// 首次引导：无 current → 用 latest 直接启动（启动本身就是验证：就绪探测 HTTP 200；
/// 不查 registry——避免启动时联网，加快启动；npx 会自动解析 latest）。
/// 有 current → 直接按锁定版本启动。由 main 后台线程调度。
pub fn bootstrap(cfg: &AppConfig) -> bool {
    let state = runtime::load_state();
    let ver = match state.current {
        Some(v) => v,
        None => {
            append_log("首次引导：使用最新版本启动（npx 自动解析）");
            supervisor::set_stage("首次启动：下载 DSH（约 1-3 分钟）…");
            "latest".to_string()
        }
    };
    match supervisor::start(cfg, &ver) {
        Ok(()) => {
            // 持久化锁定版本：state.json 记录 current（后续启动不再依赖 registry/npx 解析）
            let mut st = runtime::load_state();
            if st.current.as_deref() != Some(ver.as_str()) {
                st.current = Some(ver.clone());
                record(&mut st, &ver, true, "首次引导锁定");
                let _ = runtime::save_state(&st);
            }
            true
        }
        Err(e) => {
            append_log(&format!("启动失败: {e}"));
            false
        }
    }
}
