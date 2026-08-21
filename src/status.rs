//! 管理页（轻量 HTTP 服务，std 零依赖）：状态展示 + 安装/启停操作。
//!
//! 路由：
//! - `GET /`                     → 内嵌 HTML 管理页（状态卡片 + 按钮，JS 每 2s 轮询）
//! - `GET /api/status`           → { eng: 守护状态, env: node/npm/dsh/winget 探测, install: 安装状态 }
//! - `POST /api/install/node`    → 触发 winget 安装 Node.js（异步）
//! - `POST /api/install/dsh`     → 触发 npm install -g @deepseek-ai/dsh（异步）
//! - `POST /api/start`           → 启动 dsh
//! - `POST /api/stop`            → 关闭 dsh
//! - `GET /api/install/status`   → 安装任务状态
//!
//! 失败静默：bind 失败只记日志不影响主流程；单连接读取超时兜底，防挂死。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::AppConfig;

/// 启动管理页服务（阻塞线程内循环）。返回 Err 由调用方静默处理。
pub fn serve(port: u16, cfg: AppConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let cfg = cfg.clone();
        std::thread::spawn(move || handle(&mut stream, &cfg));
    }
    Ok(())
}

fn handle(stream: &mut TcpStream, cfg: &AppConfig) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

    let (status, ctype, body) = route(method, path, cfg);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn route(method: &str, path: &str, cfg: &AppConfig) -> (&'static str, &'static str, String) {
    match (method, path) {
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", admin_html()),
        ("GET", "/api/status") => ("200 OK", "application/json; charset=utf-8", status_json()),
        ("GET", "/api/plugins") => ("200 OK", "application/json; charset=utf-8", plugins_json()),
        ("GET", "/api/dsh/versions") => (
            "200 OK",
            "application/json; charset=utf-8",
            crate::installer::dsh_versions_json().to_string(),
        ),
        ("POST", "/api/dsh/update") => match crate::installer::dsh_latest() {
            Some(v) => match crate::installer::start_dsh_install(&v) {
                Ok(()) => ok_json(&format!("已触发更新到 {v}（异步进行，稍后刷新查看结果）")),
                Err(e) => ("409 Conflict", "application/json; charset=utf-8", err_json(&e)),
            },
            None => (
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                err_json("无法查询 dsh 最新版本（网络或 npm 异常），更新失败"),
            ),
        },
        ("POST", path) if path.starts_with("/api/dsh/install-version/") => {
            let ver = &path["/api/dsh/install-version/".len()..];
            if ver.is_empty() {
                ("400 Bad Request", "application/json; charset=utf-8", err_json("缺少版本号"))
            } else {
                match crate::installer::start_dsh_install(ver) {
                    Ok(()) => ok_json(&format!("已触发安装 dsh@{ver}（异步进行，稍后刷新查看结果）")),
                    Err(e) => ("409 Conflict", "application/json; charset=utf-8", err_json(&e)),
                }
            }
        }
        ("POST", path) if path.starts_with("/api/plugin/uninstall/") => {
            let id = &path["/api/plugin/uninstall/".len()..];
            if id.is_empty() {
                ("400 Bad Request", "application/json; charset=utf-8", err_json("缺少插件 id"))
            } else {
                match uninstall_plugin(id) {
                    Ok(msg) => ok_json(&msg),
                    Err(e) => ("400 Bad Request", "application/json; charset=utf-8", err_json(&e)),
                }
            }
        }
        ("GET", "/api/install/status") => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_string(&crate::installer::install_state()).unwrap_or_else(|_| "{}".into()),
        ),
        ("POST", "/api/install/node") => install_json("node"),
        ("POST", "/api/install/dsh") => install_json("dsh"),
        ("POST", "/api/start") => match crate::supervisor::start(cfg) {
            Ok(()) => ok_json("启动指令已下发"),
            Err(e) => ("500 Internal Server Error", "application/json; charset=utf-8", err_json(&e)),
        },
        ("POST", "/api/stop") => match crate::supervisor::stop() {
            Ok(()) => ok_json("关闭指令已下发"),
            Err(e) => ("500 Internal Server Error", "application/json; charset=utf-8", err_json(&e)),
        },
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".to_string()),
    }
}

fn install_json(kind: &str) -> (&'static str, &'static str, String) {
    match crate::installer::start_install(kind) {
        Ok(()) => ok_json(&format!("已触发安装 {}（异步进行，稍后刷新查看结果）", kind)),
        Err(e) => ("409 Conflict", "application/json; charset=utf-8", err_json(&e)),
    }
}

fn ok_json(msg: &str) -> (&'static str, &'static str, String) {
    (
        "200 OK",
        "application/json; charset=utf-8",
        serde_json::json!({ "ok": true, "msg": msg }).to_string(),
    )
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "msg": msg }).to_string()
}

/// 组合状态：守护状态 + 环境探测 + 安装状态。
fn status_json() -> String {
    let st = crate::supervisor::status();
    let eng = serde_json::to_value(&st).unwrap_or(serde_json::Value::Null);
    let env = crate::installer::probe();
    let install = crate::installer::install_state();
    serde_json::json!({ "eng": eng, "env": env, "install": install }).to_string()
}

// ---------- 插件清单（/api/plugins） ----------

/// dsh profile 目录：壳启动的是 `dsh web` → 读 web profile 的挂载配置
fn profile_dir() -> PathBuf {
    crate::runtime::system_home_dir()
        .join("profiles")
        .join("web")
}

/// 市场/NPM bundle 清单：读 profile/package.json 的 `dsh.profile.bundles`
fn parse_bundles(profile_dir: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(profile_dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    v["dsh"]["profile"]["bundles"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// 收集一条 `- id:` / `name:` patch 条目（id/name 至少其一才记录）
fn push_patch_item(
    items: &mut Vec<serde_json::Value>,
    id: &mut Option<String>,
    name: &mut Option<String>,
) {
    if id.is_some() || name.is_some() {
        let n = name.take().unwrap_or_default();
        items.push(serde_json::json!({
            "id": id.take().unwrap_or_default(),
            "source": n,
            "local": n.starts_with("file:"),
        }));
    }
}

/// 本地 patch 插件：解析 profile/cordis.patch.yml 的 `- id:` / `name:` 条目
/// （cordis.patch.yml 是 dsh profile 的 patch overlay；`name: 'file://…'` = 本地源码硬加载）
fn parse_patches(profile_dir: &Path) -> Vec<serde_json::Value> {
    let Ok(content) = std::fs::read_to_string(profile_dir.join("cordis.patch.yml")) else {
        return Vec::new();
    };
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut cur_id: Option<String> = None;
    let mut cur_name: Option<String> = None;
    for raw in content.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("- id:") {
            push_patch_item(&mut items, &mut cur_id, &mut cur_name);
            cur_id = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("name:") {
            cur_name = Some(rest.trim().trim_matches('\'').trim_matches('"').to_string());
        }
    }
    push_patch_item(&mut items, &mut cur_id, &mut cur_name);
    items
}

/// 插件清单 JSON：bundle（市场/NPM）+ patch（本地 file://）
fn plugins_json() -> String {
    let dir = profile_dir();
    serde_json::json!({
        "profile": "web",
        "dir": dir.display().to_string(),
        "exists": dir.is_dir(),
        "bundles": parse_bundles(&dir),
        "patches": parse_patches(&dir),
    })
    .to_string()
}

// ---------- 插件卸载（/api/plugin/uninstall/<id>） ----------

/// 插件卸载：先按本地 patch（cordis.patch.yml 条目）匹配，否则按市场 bundle（dsh plugin remove）。
/// 核心 bundle（dsh-base / dsh-web-app）禁止卸载——卸了引擎就废了。
fn uninstall_plugin(id: &str) -> Result<String, String> {
    let dir = profile_dir();
    if parse_patches(&dir).iter().any(|p| p["id"].as_str() == Some(id)) {
        return uninstall_patch(&dir, id);
    }
    if parse_bundles(&dir).iter().any(|b| b == id) {
        const CORE: &[&str] = &["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"];
        if CORE.contains(&id) {
            return Err(format!("{id} 是 dsh 核心包，卸载会破坏引擎，已禁止"));
        }
        return uninstall_bundle(id);
    }
    Err(format!("未找到插件: {id}"))
}

/// 本地 patch 卸载：备份后从 cordis.patch.yml 移除 `- id: <target>` 条目及其子行。
/// 重启引擎后生效（patch overlay 是启动时组装的）。
fn uninstall_patch(dir: &Path, target: &str) -> Result<String, String> {
    let path = dir.join("cordis.patch.yml");
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    let mut removed = false;
    while i < lines.len() {
        let is_target = lines[i]
            .trim()
            .strip_prefix("- id:")
            .map(|r| r.trim() == target)
            .unwrap_or(false);
        if is_target {
            removed = true;
            i += 1;
            // 跳过该条目的子行（缩进/空/注释），直到下一个顶层项（无缩进的非注释非空行）
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() || l.trim_start().starts_with('#') {
                    i += 1;
                    continue;
                }
                if !l.starts_with(char::is_whitespace) {
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }
    if !removed {
        return Err(format!("cordis.patch.yml 中未找到条目: {target}"));
    }
    // 备份原文件（可回滚）
    let bak = path.with_extension("patch.yml.bak");
    let _ = std::fs::copy(&path, &bak);
    // 已无有效条目 → 写注释空 patch（保持合法 YAML，dsh 读作空 overlay）
    let has_entry = out
        .iter()
        .any(|l| l.trim_start().starts_with("- insert:") || l.trim_start().starts_with("- replace:"));
    let new_content = if has_entry {
        out.join("\n") + "\n"
    } else {
        "# 已卸载全部 patch overlay（dsh-come 管理页，原内容见 cordis.patch.yml.bak）\n".to_string()
    };
    std::fs::write(&path, new_content).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(format!(
        "已卸载 patch 插件「{target}」（原文件备份为 cordis.patch.yml.bak，重启引擎后生效）"
    ))
}

/// 市场 bundle 卸载：`dsh plugin --profile web remove <pkg>`（转发 pnpm remove）。
fn uninstall_bundle(id: &str) -> Result<String, String> {
    let Some(runner) = crate::runtime::dsh_runner() else {
        return Err("未找到系统 dsh 命令".to_string());
    };
    let args: Vec<String> = vec![
        "plugin".into(),
        "--profile".into(),
        "web".into(),
        "remove".into(),
        id.to_string(),
    ];
    let mut cmd = crate::runtime::dsh_command(&runner, &args);
    crate::supervisor::hide_window(&mut cmd);
    let out = cmd.output().map_err(|e| format!("无法启动 dsh: {e}"))?;
    let tail = crate::installer::tail_text(&out.stdout, &out.stderr);
    if out.status.success() {
        Ok(format!("已卸载 bundle「{id}」（重启引擎后生效）。{tail}"))
    } else {
        Err(format!("卸载 {id} 失败（退出码 {:?}）。{tail}", out.status.code()))
    }
}

fn admin_html() -> String {
    r##"<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>DSH 伴侣 · 管理</title>
<style>
:root{--ds:#4D6BFE;--ds-dark:#3a57e0;--bg:#F7F8FA;--card:#fff;--text:#1A1A1A;--muted:#6B7280;--border:#E5E7EB;--ok:#10B981;--bad:#EF4444;--warn:#F59E0B;--radius:16px;--shadow:0 1px 3px rgba(0,0,0,.06),0 4px 12px rgba(0,0,0,.04)}
*{box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,"PingFang SC","Microsoft YaHei",sans-serif;margin:0;padding:24px 16px;background:var(--bg);color:var(--text);line-height:1.5}
.wrap{max-width:840px;margin:0 auto}
header{display:flex;align-items:center;gap:14px;margin-bottom:24px}
header h1{font-size:22px;font-weight:600;margin:0;letter-spacing:-.3px}
header p{margin:2px 0 0;font-size:13px;color:var(--muted)}
.whale{width:42px;height:42px;flex-shrink:0;color:var(--ds)}
.card{background:var(--card);border:1px solid var(--border);border-radius:var(--radius);padding:20px 22px;margin-bottom:16px;box-shadow:var(--shadow)}
.card h3{font-size:14px;font-weight:600;color:var(--muted);text-transform:uppercase;letter-spacing:.5px;margin:0 0 14px}
.grid2{display:grid;grid-template-columns:repeat(auto-fit,minmax(300px,1fr));gap:16px}
.row{display:flex;gap:10px;flex-wrap:wrap;margin-top:14px;align-items:center}
.row.right{justify-content:flex-end;margin-top:8px}
button{appearance:none;border:1px solid var(--border);background:#fff;color:var(--text);padding:8px 16px;border-radius:10px;font-size:13px;font-weight:500;cursor:pointer;transition:all .12s}
button:hover{border-color:#cbd5e1;background:#f9fafb;transform:translateY(-1px)}
button:disabled{opacity:.5;cursor:not-allowed;transform:none}
button.primary{background:var(--ds);color:#fff;border-color:var(--ds)}
button.primary:hover{background:var(--ds-dark);border-color:var(--ds-dark)}
button.danger{color:var(--bad);border-color:#fecaca}
button.danger:hover{background:#fef2f2}
select{padding:7px 12px;border:1px solid var(--border);border-radius:10px;background:#fff;font-size:13px;color:var(--text);min-width:160px}
.badge{display:inline-flex;align-items:center;gap:6px;padding:4px 10px;border-radius:999px;font-size:12px;font-weight:600}
.badge.ok{background:#d1fae5;color:#065f46}
.badge.bad{background:#fee2e2;color:#991b1b}
.badge.warn{background:#fef3c7;color:#92400e}
.badge.muted{background:#f3f4f6;color:#4b5563}
.status-big{font-size:18px;font-weight:600;margin-bottom:4px}
.status-meta{color:var(--muted);font-size:13px}
.env-line{display:flex;gap:24px;flex-wrap:wrap;font-size:13px}
.env-item{display:flex;align-items:center;gap:6px}
.env-item .dot{width:7px;height:7px;border-radius:50%}
.env-item .dot.ok{background:var(--ok)}.env-item .dot.bad{background:var(--bad)}.env-item .dot.muted{background:#d1d5db}
.list{font-size:13px}
.item{display:flex;align-items:flex-start;justify-content:space-between;gap:12px;padding:10px 0;border-bottom:1px solid var(--border)}
.item:last-child{border-bottom:none}
.item .name{font-weight:500;word-break:break-all}
.item .source{font-size:12px;color:var(--muted);margin-top:2px}
.item .tag{font-size:11px;padding:2px 6px;border-radius:6px;background:#eef2ff;color:var(--ds);font-weight:600;margin-left:6px}
.empty{color:var(--muted);font-size:13px;padding:8px 0}
#toast{position:fixed;right:16px;bottom:16px;max-width:420px;background:#fff;border:1px solid var(--border);border-radius:12px;padding:14px 18px;box-shadow:0 10px 25px rgba(0,0,0,.12);font-size:13px;z-index:50;transform:translateY(120%);transition:transform .25s;pointer-events:none}
#toast.show{transform:translateY(0)}
#toast.ok{border-left:4px solid var(--ok)}
#toast.bad{border-left:4px solid var(--bad)}
</style>
</head>
<body>
<div class="wrap">
<header>
<svg class="whale" viewBox="0 0 50 50" fill="currentColor" aria-hidden="true"><path d="M48.8354 10.0479C48.3232 9.79199 48.1025 10.2798 47.8032 10.5278C47.7007 10.6079 47.6143 10.7119 47.5273 10.8076C46.7793 11.624 45.9048 12.1597 44.7622 12.0957C43.0923 12 41.666 12.5356 40.4058 13.8398C40.1377 12.2319 39.2476 11.272 37.8926 10.6558C37.1836 10.3359 36.4668 10.0156 35.9702 9.31982C35.6235 8.82373 35.5293 8.27197 35.356 7.72754C35.2456 7.3999 35.1353 7.06396 34.7651 7.00781C34.3633 6.94385 34.2056 7.2876 34.0479 7.57568C33.418 8.75195 33.1733 10.0479 33.1973 11.3599C33.2524 14.312 34.4736 16.6641 36.8999 18.3359C37.1758 18.5278 37.2466 18.7197 37.1597 19C36.9946 19.5757 36.7974 20.1357 36.624 20.7119C36.5137 21.0801 36.3486 21.1597 35.9624 21C34.6309 20.4321 33.481 19.5918 32.4644 18.5757C30.7393 16.8721 29.1792 14.9917 27.2334 13.52C26.7764 13.1758 26.3193 12.856 25.8467 12.5518C23.8618 10.584 26.1069 8.96777 26.627 8.77588C27.1704 8.57568 26.8159 7.8877 25.0591 7.896C23.3022 7.90381 21.6953 8.50391 19.647 9.30371C19.3477 9.42383 19.0322 9.51172 18.7095 9.58398C16.8501 9.22363 14.9199 9.14355 12.9033 9.37598C9.10596 9.80762 6.07275 11.6396 3.84326 14.7681C1.16455 18.5278 0.53418 22.7998 1.30664 27.2559C2.11768 31.9521 4.46582 35.8398 8.07373 38.8799C11.8159 42.0322 16.1255 43.5762 21.041 43.2803C24.0269 43.104 27.3516 42.6963 31.1016 39.4561C32.0469 39.936 33.0396 40.1279 34.686 40.272C35.9546 40.3921 37.1758 40.208 38.1211 40.0078C39.6021 39.688 39.4995 38.2881 38.9639 38.0322C34.623 35.9678 35.5762 36.8081 34.71 36.1279C36.9155 33.4639 40.2402 30.6958 41.54 21.728C41.6426 21.0161 41.5557 20.5679 41.54 19.9917C41.5322 19.6396 41.6108 19.5039 42.0049 19.4639C43.0923 19.3359 44.1479 19.0317 45.1167 18.4878C47.9292 16.9199 49.064 14.3438 49.3315 11.2559C49.3711 10.7837 49.3237 10.2959 48.8354 10.0479ZM24.3262 37.8398C20.1196 34.4639 18.0791 33.3521 17.2358 33.3999C16.4482 33.4482 16.5898 34.3682 16.7632 34.9678C16.9443 35.5601 17.1812 35.9683 17.5117 36.4878C17.7402 36.832 17.8979 37.3442 17.2832 37.728C15.9282 38.584 13.5728 37.4399 13.4624 37.3838C10.7207 35.7358 8.42822 33.5601 6.81348 30.584C5.25342 27.7197 4.34766 24.6479 4.19775 21.3677C4.1582 20.5757 4.38672 20.2959 5.15869 20.1519C6.17529 19.96 7.22314 19.9199 8.23926 20.0718C12.5327 20.7119 16.1885 22.6719 19.2529 25.7759C21.002 27.5439 22.3252 29.6558 23.6885 31.7202C25.1377 33.9121 26.6978 36 28.6831 37.7119C29.3843 38.312 29.9434 38.7681 30.479 39.104C28.8643 39.2881 26.1699 39.3281 24.3262 37.8398ZM26.3433 24.6001C26.3433 24.248 26.6191 23.9678 26.9658 23.9678C27.0444 23.9678 27.1152 23.9839 27.1782 24.0078C27.2651 24.04 27.3438 24.0879 27.4067 24.1602C27.5171 24.272 27.5801 24.4321 27.5801 24.6001C27.5801 24.9521 27.3042 25.2319 26.9575 25.2319C26.6108 25.2319 26.3433 24.9521 26.3433 24.6001ZM32.6064 27.8799C32.2046 28.0479 31.8027 28.1919 31.4165 28.208C30.8179 28.2397 30.1641 27.9922 29.8096 27.688C29.2583 27.2158 28.8643 26.9521 28.6987 26.1279C28.6279 25.7759 28.6675 25.2319 28.7305 24.9199C28.8721 24.248 28.7144 23.8159 28.2495 23.4238C27.8716 23.104 27.3911 23.0161 26.8633 23.0161C26.666 23.0161 26.4849 22.9277 26.3511 22.856C26.1304 22.7441 25.9492 22.4639 26.1226 22.1201C26.1777 22.0078 26.4458 21.7358 26.5088 21.688C27.2256 21.272 28.0527 21.4077 28.8169 21.7197C29.5259 22.0161 30.0615 22.5601 30.834 23.3281C31.6216 24.2559 31.7632 24.5117 32.2124 25.208C32.5669 25.752 32.8901 26.312 33.1104 26.9521C33.2446 27.3521 33.0713 27.6802 32.6064 27.8799Z"/></svg>
<div>
<h1>DSH 伴侣</h1>
<p>DeepSeek Harness 本地守护 · 管理页</p>
</div>
</header>

<div class="card">
<h3>dsh 引擎</h3>
<div class="status-big" id="engStatus">加载中…</div>
<div class="status-meta" id="engMeta">等待状态端点响应</div>
<div class="row">
<button class="primary" onclick="openDsh()">打开 dsh 界面</button>
<button onclick="act('start')">启动 dsh</button>
<button onclick="act('stop')">关闭 dsh</button>
</div>
</div>

<div class="grid2">
<div class="card">
<h3>运行环境</h3>
<div class="env-line" id="env">加载中…</div>
<div class="row">
<button id="btn-node" onclick="act('install/node')">安装 Node.js</button>
<button id="btn-dsh" onclick="act('install/dsh')">安装 dsh</button>
</div>
</div>

<div class="card">
<h3>版本管理</h3>
<div id="dshver">加载中…</div>
<div class="row">
<button class="primary" id="btn-upd" onclick="act('dsh/update')">更新到最新</button>
<select id="ver-sel"><option value="">选择历史版本…</option></select>
<button onclick="installVer()">安装所选</button>
</div>
</div>
</div>

<div class="card">
<h3>已安装插件（web profile）</h3>
<div class="list" id="plugins">加载中…</div>
</div>

<div id="toast"></div>
</div>

<script>
let DSPORT = 3080;
function openDsh(){ window.open('http://127.0.0.1:'+DSPORT, '_blank'); }
async function act(a){
  try {
    const r = await fetch('/api/'+a,{method:'POST'});
    const d = await r.json();
    if (d.msg) flash(d.msg, d.ok!==false);
  } catch(e){ flash('请求失败：'+e, false); }
  refresh();
}
async function installVer(){
  const v = document.getElementById('ver-sel').value;
  if (!v) { flash('请先选择要安装的历史版本', false); return; }
  try {
    const r = await fetch('/api/dsh/install-version/'+encodeURIComponent(v),{method:'POST'});
    const d = await r.json();
    flash(d.msg, d.ok!==false);
  } catch(e){ flash('安装请求失败：'+e, false); }
  refresh();
}
async function uninstall(id){
  if(!confirm('确定卸载插件「'+id+'」吗？卸载后需重启引擎生效。')) return;
  try {
    const r = await fetch('/api/plugin/uninstall/'+encodeURIComponent(id),{method:'POST'});
    const d = await r.json();
    flash(d.msg, d.ok!==false);
  } catch(e){ flash('卸载请求失败：'+e, false); }
  refresh();
}
let toastTimer;
function flash(msg, ok){
  const el = document.getElementById('toast');
  el.className = ok ? 'ok' : 'bad';
  el.textContent = msg;
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), ok ? 5000 : 8000);
}
function fmtEnv(label, value, ok){
  return '<div class="env-item"><span class="dot '+(ok?'ok':'bad')+'"></span><span>'+label+' '+value+'</span></div>';
}
async function refresh(){
  try {
    const r = await fetch('/api/status');
    const d = await r.json();
    const e = d.eng||{};
    if (e.port) DSPORT = e.port;
    const running = !!e.running;
    const ready = !!e.ready;
    const big = document.getElementById('engStatus');
    const meta = document.getElementById('engMeta');
    if (running && ready) {
      big.innerHTML = '<span class="badge ok">运行中</span>';
      meta.innerHTML = 'dsh 已就绪 · 端口 '+e.port+(e.pid?' · PID '+e.pid:'')+(e.version?' · '+e.version:'');
    } else if (running) {
      big.innerHTML = '<span class="badge warn">启动中</span>';
      meta.innerHTML = 'dsh 正在启动'+(e.stage?' · '+e.stage:'')+(e.pid?' · PID '+e.pid:'');
    } else if (e.last_error) {
      big.innerHTML = '<span class="badge bad">已停止</span>';
      meta.innerHTML = '上次错误：'+e.last_error;
    } else {
      big.innerHTML = '<span class="badge bad">已停止</span>';
      meta.innerHTML = 'dsh 未在运行';
    }
    const v = d.env||{};
    document.getElementById('env').innerHTML =
      fmtEnv('Node', v.node||'未安装', !!v.node)+
      fmtEnv('npm', v.npm||'未安装', !!v.npm)+
      fmtEnv('dsh', v.dsh||'未安装', !!v.dsh)+
      fmtEnv('winget', v.winget||'不可用', !!v.winget);
    const ins = d.install||{};
    document.getElementById('btn-node').disabled = !!ins.running;
    document.getElementById('btn-dsh').disabled = !!ins.running;
    if (ins.running) flash('安装中：'+(ins.kind||'')+' … '+(ins.msg||''), true);
    else if (ins.msg && ins.kind && !ins.running) flash((ins.ok?'安装成功：':'安装失败：')+ins.msg, !!ins.ok);
  } catch(e){
    document.getElementById('engStatus').innerHTML = '<span class="badge bad">连接失败</span>';
    document.getElementById('engMeta').textContent = '无法连接状态端点';
  }
  try {
    const vr = await fetch('/api/dsh/versions');
    const dv = await vr.json();
    const cur = dv.current||'未安装';
    const lat = dv.latest||'查询失败';
    const tagInfo = dv.latest_tag ? ' · npm stable tag: '+dv.latest_tag : '';
    const badge = dv.has_update ? '<span class="badge warn">可更新到 '+lat+'</span>' : '<span class="badge ok">已是最新</span>';
    document.getElementById('dshver').innerHTML =
      '<div style="margin-bottom:8px">当前 <b>'+cur+'</b> · 最新发布 <b>'+lat+'</b>'+tagInfo+'</div>'+badge;
    const sel = document.getElementById('ver-sel');
    const prev = sel.value;
    sel.innerHTML = '<option value="">选择历史版本…</option>'+(dv.versions||[]).map(v=>'<option value="'+v+'">'+v+'</option>').join('');
    if (prev) sel.value = prev;
    document.getElementById('btn-upd').disabled = !dv.has_update;
  } catch(e){
    document.getElementById('dshver').innerHTML = '<span class="badge bad">版本信息读取失败</span>';
  }
  try {
    const pr = await fetch('/api/plugins');
    const pl = await pr.json();
    const el = document.getElementById('plugins');
    if (!pl.exists) { el.innerHTML = '<div class="empty">未找到 profile 目录：'+pl.dir+'</div>'; }
    else {
      let html = '<div style="margin-bottom:12px"><b>市场 / NPM</b> <span class="badge muted">'+(pl.bundles||[]).length+'</span></div>';
      html += (pl.bundles||[]).length ? (pl.bundles||[]).map(x=>'<div class="item"><div><div class="name">'+x+'</div></div><button class="danger" onclick="uninstall(\''+x+'\')">卸载</button></div>').join('') : '<div class="empty">无</div>';
      html += '<div style="margin:18px 0 12px"><b>本地 patch</b> <span class="badge muted">'+(pl.patches||[]).length+'</span></div>';
      html += (pl.patches||[]).length ? (pl.patches||[]).map(x=>'<div class="item"><div><div class="name">'+x.id+(x.local?'<span class="tag">本地</span>':'')+'</div><div class="source">'+x.source+'</div></div><button class="danger" onclick="uninstall(\''+x.id+'\')">卸载</button></div>').join('') : '<div class="empty">无</div>';
      el.innerHTML = html;
    }
  } catch(e){
    document.getElementById('plugins').innerHTML = '<span class="badge bad">插件清单读取失败</span>';
  }
}
setInterval(refresh, 2000);
refresh();
</script>
</body>
</html>"##
        .to_string()
}
