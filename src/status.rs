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
    r#"<!doctype html>
<html lang="zh">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>DSH 伴侣 · 管理</title>
<style>
body{font-family:system-ui,sans-serif;max-width:660px;margin:24px auto;padding:0 16px;color:#1f2328;background:#f6f8fa}
h1{font-size:19px;font-weight:500}
.card{background:#fff;border:1px solid #d0d7de;border-radius:12px;padding:16px 18px;margin:14px 0}
h3{margin:0 0 10px;font-size:14px;font-weight:500;color:#444441}
.row{display:flex;gap:8px;flex-wrap:wrap;margin-top:12px}
button{padding:8px 14px;border-radius:8px;border:1px solid #d0d7de;background:#fff;font-size:13px;cursor:pointer}
button:hover{background:#eef1f4}
button:disabled{opacity:.5;cursor:not-allowed}
.ok{color:#1a7f37}.bad{color:#cf222e}.muted{color:#5f5e5a}
pre{background:#f6f8fa;border-radius:8px;padding:10px;font-size:12px;white-space:pre-wrap;word-break:break-all}
</style>
<h1>DSH 伴侣 · 管理</h1>

<div class="card">
<h3>dsh 引擎</h3>
<div id="eng" class="muted">加载中…</div>
<div class="row">
<button onclick="act('start')">启动 dsh</button>
<button onclick="act('stop')">关闭 dsh</button>
</div>
</div>

<div class="card">
<h3>环境与安装</h3>
<div id="env" class="muted">加载中…</div>
<div id="installMsg"></div>
<div class="row">
<button id="btn-node" onclick="act('install/node')">安装 Node.js</button>
<button id="btn-dsh" onclick="act('install/dsh')">安装 dsh</button>
</div>
</div>

<div class="card">
<h3>dsh 版本</h3>
<div id="dshver" class="muted">加载中…</div>
<div class="row">
<button id="btn-upd" onclick="act('dsh/update')">更新到最新</button>
<select id="ver-sel" style="padding:6px;border-radius:8px;border:1px solid #d0d7de"></select>
<button onclick="installVer()">安装所选版本</button>
</div>
</div>

<div class="card">
<h3>已安装插件（web profile）</h3>
<div id="plugins" class="muted">加载中…</div>
</div>

<script>
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
function flash(msg, ok){
  const el = document.getElementById('installMsg');
  el.innerHTML = '<pre class="'+(ok?'ok':'bad')+'">'+msg+'</pre>';
}
async function refresh(){
  try {
    const r = await fetch('/api/status');
    const d = await r.json();
    const e = d.eng||{};
    document.getElementById('eng').innerHTML =
      '<b class="'+(e.running?'ok':'bad')+'">'+(e.running?'运行中':'已停止')+'</b>'+
      (e.ready?' · 就绪':' · 未就绪')+' · 端口 '+e.port+
      (e.pid?' · pid '+e.pid:'')+(e.version?' · '+e.version:'');
    const v = d.env||{};
    document.getElementById('env').innerHTML =
      'Node '+(v.node?'<span class="ok">'+v.node+'</span>':'<span class="bad">未安装</span>')+
      ' · npm '+(v.npm?'<span class="ok">'+v.npm+'</span>':'<span class="bad">未安装</span>')+
      ' · dsh '+(v.dsh?'<span class="ok">'+v.dsh+'</span>':'<span class="bad">未安装</span>')+
      ' · winget '+(v.winget?'<span class="ok">可用</span>':'<span class="muted">不可用</span>');
    const ins = d.install||{};
    document.getElementById('btn-node').disabled = !!ins.running;
    document.getElementById('btn-dsh').disabled = !!ins.running;
    if (ins.running) flash('安装中：'+(ins.kind||'')+' … '+(ins.msg||''), true);
    else if (ins.msg && ins.kind && !ins.running) flash((ins.ok?'安装成功：':'安装失败：')+ins.msg, !!ins.ok);
  } catch(e){
    document.getElementById('eng').innerHTML = '<span class="bad">无法连接状态端点</span>';
  }
  try {
    const vr = await fetch('/api/dsh/versions');
    const dv = await vr.json();
    const vel = document.getElementById('dshver');
    const cur = dv.current||'未安装';
    const lat = dv.latest||'查询失败（离线？）';
    const tag = dv.latest_tag ? ' · <span class="muted">npm stable tag: '+dv.latest_tag+'</span>' : '';
    let badge = dv.has_update ? '<span class="bad">有新版本 '+lat+'</span>' : '<span class="ok">已是最新</span>';
    vel.innerHTML = '当前 <b>'+cur+'</b> · 最新发布 <b>'+lat+'</b>'+tag+' · '+badge;
    const sel = document.getElementById('ver-sel');
    const prev = sel.value;
    sel.innerHTML = '<option value="">选择历史版本…</option>'+(dv.versions||[]).map(v=>'<option value="'+v+'">'+v+'</option>').join('');
    if (prev) sel.value = prev;
    document.getElementById('btn-upd').disabled = !dv.has_update;
  } catch(e){
    document.getElementById('dshver').innerHTML = '<span class="bad">版本信息读取失败</span>';
  }
  try {
    const pr = await fetch('/api/plugins');
    const pl = await pr.json();
    const el = document.getElementById('plugins');
    if (!pl.exists) { el.innerHTML = '<span class="muted">未找到 profile 目录：'+pl.dir+'</span>'; }
    else {
      const b = (pl.bundles||[]).map(x=>'<div>'+x+' <button onclick="uninstall(\''+x+'\')">卸载</button></div>').join('') || '<span class="muted">无</span>';
      const p = (pl.patches||[]).map(x=>'<div><b>'+(x.local?'本地':'')+'</b> '+x.id+' <button onclick="uninstall(\''+x.id+'\')">卸载</button><br><span class="muted" style="font-size:11px">'+x.source+'</span></div>').join('') || '<span class="muted">无</span>';
      el.innerHTML = '<div>市场/NPM（'+(pl.bundles||[]).length+'）：<br>'+b+'</div>'+
        '<div style="margin-top:8px">本地 patch（'+(pl.patches||[]).length+'）：<br>'+p+'</div>';
    }
  } catch(e){
    document.getElementById('plugins').innerHTML = '<span class="bad">插件清单读取失败</span>';
  }
}
setInterval(refresh, 2000);
refresh();
</script>"#
        .to_string()
}
