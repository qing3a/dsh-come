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
    let patch = runtime::come_patch_path();
    let patch = patch.is_file().then_some(patch);
    cmd.arg(&npx)
        .args(supervisor::npx_argv(ver, "127.0.0.1", port, patch.as_deref()))
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
/// 旧版本记入 state.previous（回滚目标）；切换后需重启 dsh 引擎生效（托盘「重启引擎」）。
pub fn apply_pending() -> Result<String, String> {
    let mut state = runtime::load_state();
    let ver = state.pending.clone().ok_or_else(|| "没有待应用的更新".to_string())?;
    state.previous = state.current.clone();
    state.current = Some(ver.clone());
    state.pending = None;
    record(&mut state, &ver, true, "用户确认应用");
    runtime::save_state(&state).map_err(|e| e.to_string())?;
    append_log(&format!("已切换到 v{ver}（重启 dsh 引擎生效）"));
    Ok(ver)
}

/// 一键回滚到上一版本（托盘「回滚到 vX」）：current ↔ previous 交换（可来回切换），
/// 由调用方重启引擎生效。返回切到的版本号。
pub fn rollback() -> Result<String, String> {
    let mut state = runtime::load_state();
    let prev = state.previous.clone().ok_or_else(|| "没有可回滚的上一版本".to_string())?;
    if state.current.as_deref() == Some(prev.as_str()) {
        return Err("当前已是该版本".to_string());
    }
    let cur = state.current.clone();
    state.current = Some(prev.clone());
    state.previous = cur;
    record(&mut state, &prev, true, "用户回滚");
    runtime::save_state(&state).map_err(|e| e.to_string())?;
    append_log(&format!("已回滚到 v{prev}（重启 dsh 引擎生效）"));
    Ok(prev)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 更新-回滚闭环（持 DSH_HOME_TEST_LOCK 与 tray 的窗口几何测试串行，
    /// 避免并行测试踩踏进程级 DSH_DESKTOP_HOME）：
    /// 应用更新记 previous → 回滚交换 current/previous → 无 previous 时报错
    #[test]
    fn update_flow_apply_and_rollback_isolated() {
        let _guard = runtime::DSH_HOME_TEST_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("dsh-update-flow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("DSH_DESKTOP_HOME", &tmp);

        // 场景 1：应用更新 → 旧版本记为回滚目标
        let mut st = runtime::State::default();
        st.current = Some("1.0".to_string());
        st.pending = Some("2.0".to_string());
        runtime::save_state(&st).unwrap();
        assert_eq!(apply_pending().unwrap(), "2.0");
        let after = runtime::load_state();
        assert_eq!(after.current.as_deref(), Some("2.0"));
        assert_eq!(after.previous.as_deref(), Some("1.0"), "旧版本应记为回滚目标");
        assert!(after.pending.is_none());

        // 场景 2：回滚 → current/previous 交换（可再切回）
        assert_eq!(rollback().unwrap(), "1.0");
        let after = runtime::load_state();
        assert_eq!(after.current.as_deref(), Some("1.0"));
        assert_eq!(after.previous.as_deref(), Some("2.0"), "交换后仍可切回");

        // 场景 3：无上一版本 → 明确报错（不静默）
        let _ = std::fs::remove_file(runtime::state_path());
        runtime::save_state(&runtime::State::default()).unwrap();
        assert!(rollback().is_err(), "无上一版本时回滚应报错");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("DSH_DESKTOP_HOME");
    }
}
