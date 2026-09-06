//! 管理页（轻量 HTTP 服务，std 零依赖）：状态展示 + 安装/启停/版本/插件操作。
//!
//! 路由：
//! - `GET /`                     → 内嵌 HTML 管理页（状态卡片 + 按钮，JS 每 2s 轮询）
//! - `GET /api/status`           → { eng: 守护状态, env: node/npm/dsh/winget 探测, install: 安装状态 }
//! - `GET /api/dsh/versions`     → { current, latest, latest_tag, tags, has_update, versions }（dsh 版本管理）
//! - `POST /api/dsh/update`      → 更新 dsh 到最新（异步）
//! - `POST /api/dsh/install-version/<ver>` → 安装指定版本 dsh（异步）
//! - `GET /api/plugins`          → web profile 插件清单（bundles + patches + 内置市场）
//! - `POST /api/plugin/install`  → 安装插件（query: src=<本地路径> 或 id=<内置清单 id>，异步）
//! - `POST /api/plugin/uninstall/<id>` → 卸载插件（patch 条目移除 / dsh plugin remove）
//! - `POST /api/install/node`    → 触发 winget 安装 Node.js（异步）
//! - `POST /api/install/dsh`     → 触发 npm install -g @deepseek-ai/dsh（异步）
//! - `POST /api/dsh/uninstall`   → 纯净卸载 dsh（同步；query: keepData=0/1, cleanShim=0/1）
//! - `POST /api/start`           → 启动 dsh
//! - `POST /api/stop`            → 关闭 dsh
//! - `GET /api/install/status`   → 安装任务状态
//!
//! 失败静默：bind 失败只记日志不影响主流程；单连接读取超时兜底，防挂死。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::config::AppConfig;

/// 请求头读取上限（请求行 + 全部头）。超出直接 431，防止长 URL/巨型头耗尽内存。
/// 64KB 对本地管理页绰绰有余（正常请求 < 2KB）。
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// 并发连接上限。本地管理页正常并发个位数；设上限防止本地洪水耗尽线程。
const MAX_CONCURRENT_CONNS: usize = 32;

/// CSRF token 请求头名（前端由 `window.fetch` 包装统一附加）。
const CSRF_HEADER: &str = "x-dsh-come-token";

/// CSRF token 占位符：服务端返回管理页 HTML 时替换为真实 token。
/// token 只内嵌在同源 HTML 中，跨域脚本无法读取本页内容，因此无法窃取。
const CSRF_PLACEHOLDER: &str = "__DSH_CSRF_TOKEN__";

/// 运行期实际管理页端口（固定端口被占时回退为随机端口，见 `bind_any`）。
/// 启动时绑定成功后写入；托盘菜单 / 向导据此打开管理页，不依赖配置里的期望值。
static ADMIN_PORT: OnceLock<Mutex<Option<u16>>> = OnceLock::new();

/// 记录实际管理页端口（None = 管理页关闭/未启动）。
pub fn set_admin_port(p: Option<u16>) {
    if let Ok(mut g) = ADMIN_PORT.get_or_init(|| Mutex::new(None)).lock() {
        *g = p;
    }
}

/// 当前实际管理页端口；None = 关闭（status_port=0）或尚未绑定成功。
pub fn admin_port() -> Option<u16> {
    ADMIN_PORT
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| *g)
}

/// 绑定管理页监听：先试期望端口（status_port），被占则回退随机端口（bind 0）。
/// 返回 (listener, 实际端口)。防与其他应用端口冲突导致管理页不可用。
/// port=0 时直接要 ephemeral 端口。
pub fn bind_any(port: u16) -> std::io::Result<(TcpListener, u16)> {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) => TcpListener::bind(("127.0.0.1", 0))?,
    };
    let actual = listener.local_addr()?.port();
    Ok((listener, actual))
}

/// 服务循环（监听器已由调用方 bind 好）：阻塞处理连接，失败静默（不阻塞主流程）。
/// 当前活跃连接数（配合 `MAX_CONCURRENT_CONNS` 做并发闸门）。
/// 用 AtomicUsize 而非 Semaphore：`std::sync::Semaphore` 至今仍是 unstable。
static CONN_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn serve_listener(listener: TcpListener, cfg: AppConfig) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        // 并发闸门：本地工具也设上限，避免异常情况下无限 spawn 线程耗尽资源。
        // 满额时直接关闭连接（背压），既不排队也不崩。
        let n = CONN_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n >= MAX_CONCURRENT_CONNS {
            CONN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            let _ = stream.shutdown(std::net::Shutdown::Both);
            continue;
        }
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            handle(&mut stream, &cfg);
            CONN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        });
    }
}

fn handle(stream: &mut TcpStream, cfg: &AppConfig) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    let (status, ctype, body) = match read_request_head(stream) {
        Ok(head) => {
            let (method, path, headers) = parse_head(&head);
            // 写操作（非 GET/HEAD）必须同时通过 Host + Origin + CSRF token 校验。
            // localhost 不是安全边界：浏览器对 127.0.0.1:port 的**简单请求**
            //（POST 且无自定义头）不触发 preflight，请求会直接发出去。
            // 响应读不到也无所谓——副作用（卸载 dsh / 删 ~/.dsh / 停引擎）已经发生。
            if !is_safe_method(method)
                && !is_same_origin_local(admin_port().unwrap_or(cfg.status_port), &headers)
            {
                (
                    "403 Forbidden",
                    "application/json; charset=utf-8",
                    err_json(crate::i18n::tr(
                        "已拒绝跨站请求：仅接受来自本管理页的操作",
                        "Cross-site request rejected: only requests from this admin page are accepted",
                    )),
                )
            } else {
                route(method, path, cfg)
            }
        }
        Err(HeadError::TooLarge) => (
            "431 Request Header Fields Too Large",
            "text/plain; charset=utf-8",
            "request header too large".to_string(),
        ),
        Err(HeadError::Incomplete) => (
            "400 Bad Request",
            "text/plain; charset=utf-8",
            "bad request".to_string(),
        ),
    };

    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nVary: Origin\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// 请求头读取失败原因。
enum HeadError {
    /// 头超过 `MAX_HEADER_BYTES`
    TooLarge,
    /// 连接关闭 / 读错，且未收到任何数据
    Incomplete,
}

/// 循环读到请求头结束标记 `\r\n\r\n`。
/// 单次 `read` 不可靠：TCP 可能分片，长 URL（如 `/api/plugin/install?src=<长路径>`）
/// 会被截断成半个路径，表现为莫名其妙的 404。
fn read_request_head(stream: &mut TcpStream) -> Result<String, HeadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                // 分隔符最多跨越「上一轮尾部 3 字节 + 本轮开头」，只需回看 3 字节
                let scan_from = buf.len().saturating_sub(3);
                buf.extend_from_slice(&chunk[..n]);
                if buf[scan_from..].windows(4).any(|w| w == b"\r\n\r\n") {
                    return Ok(String::from_utf8_lossy(&buf).into_owned());
                }
                if buf.len() > MAX_HEADER_BYTES {
                    return Err(HeadError::TooLarge);
                }
            }
            Err(_) => break,
        }
    }
    if buf.is_empty() {
        Err(HeadError::Incomplete)
    } else {
        // 对端发完就关（无空行）：尽力解析，让 route 给出 404 而非静默断连
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

/// 解析请求头 → (method, path, headers)。header 名统一小写便于查找。
fn parse_head(head: &str) -> (&str, &str, Vec<(String, &str)>) {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim()));
        }
    }
    (method, path, headers)
}

fn is_safe_method(m: &str) -> bool {
    matches!(m, "GET" | "HEAD")
}

fn header<'a>(headers: &'a [(String, &str)], key: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

/// 非安全方法的来源校验：Host + Origin + CSRF token 三者全中才放行。
/// - **Host**：防 DNS rebinding。攻击者域名解析到 127.0.0.1 时，浏览器填的 Host
///   是攻击者域名而非 `127.0.0.1:port`，据此拒绝。
/// - **Origin**：必须**存在**且匹配。现代浏览器同源 fetch 必带 Origin；
///   缺失即视为非本页发起（`<img>`/`<script>` 等标签的 GET 本就无 Origin）。
/// - **CSRF token**：即使前两项被绕过（如某些代理/扩展改写头），没有 token 仍无法执行。
fn is_same_origin_local(port: u16, headers: &[(String, &str)]) -> bool {
    let hosts = ["127.0.0.1", "localhost"];

    let host_ok = header(headers, "host")
        .map(|v| {
            hosts
                .iter()
                .any(|h| v.eq_ignore_ascii_case(&format!("{h}:{port}")))
        })
        .unwrap_or(false);
    let origin_ok = header(headers, "origin")
        .map(|v| {
            hosts
                .iter()
                .any(|h| v.eq_ignore_ascii_case(&format!("http://{h}:{port}")))
        })
        .unwrap_or(false);
    let token_ok = header(headers, CSRF_HEADER)
        .map(|v| v == csrf_token())
        .unwrap_or(false);

    host_ok && origin_ok && token_ok
}

/// 进程级 CSRF token：注入管理页 HTML，非安全方法需回传比对。
/// 由 pid + 纳秒时间戳 + 管理页端口混合后取 SHA256；本机单机场景下不可预测。
fn csrf_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut h = Sha256::new();
        h.update(
            format!(
                "dsh-come:{}:{}:{}",
                std::process::id(),
                nanos,
                admin_port().unwrap_or(0)
            )
            .as_bytes(),
        );
        crate::updater::hex_lower(&h.finalize())
    })
}
fn route(method: &str, path: &str, cfg: &AppConfig) -> (&'static str, &'static str, String) {
    match (method, path) {
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", admin_html()),
        ("GET", "/api/status") => (
            "200 OK",
            "application/json; charset=utf-8",
            status_json(cfg),
        ),
        ("GET", "/api/install/status") => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_string(&crate::installer::install_state())
                .unwrap_or_else(|_| "{}".into()),
        ),
        // ---------- dsh 版本管理（2026-09-01 恢复：收敛轮误删——dsh 是 npm 包，版本切换只能走 npm，dsh web 无此能力） ----------
        ("GET", "/api/dsh/versions") => (
            "200 OK",
            "application/json; charset=utf-8",
            crate::installer::dsh_versions_json().to_string(),
        ),
        ("POST", "/api/dsh/update") => match crate::installer::dsh_latest() {
            Some(v) => match crate::installer::start_dsh_install(&v) {
                Ok(()) => triggered_json(&format!(
                    "{} {v}",
                    crate::i18n::tr("已触发更新到", "Update to")
                )),
                Err(e) => conflict(&e),
            },
            None => (
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                err_json(&crate::i18n::tr(
                    "无法查询 dsh 最新版本（网络或 npm 异常），更新失败",
                    "Cannot query the latest dsh version (network or npm issue); update failed",
                )),
            ),
        },
        ("POST", path) if path.starts_with("/api/dsh/install-version/") => {
            let ver = &path["/api/dsh/install-version/".len()..];
            if ver.is_empty() {
                (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    err_json(&crate::i18n::tr("缺少版本号", "Missing version")),
                )
            } else {
                match crate::installer::start_dsh_install(ver) {
                    Ok(()) => triggered_json(&format!(
                        "{} dsh@{ver}",
                        crate::i18n::tr("已触发安装", "Install triggered for")
                    )),
                    Err(e) => conflict(&e),
                }
            }
        }
        // ---------- HLP 插件 + LocalApp 生态（v1.4.0 P0-2/P0-3） ----------
        ("GET", "/api/hlp/status") => (
            "200 OK",
            "application/json; charset=utf-8",
            crate::hlp_plugin::status_json().to_string(),
        ),
        ("POST", "/api/hlp/install") => match crate::installer::spawn_task("hlp-plugin", || {
            match crate::hlp_plugin::install() {
                Ok(m) => (true, m),
                Err(e) => (false, e),
            }
        }) {
            Ok(()) => ok_json(&crate::i18n::tr(
                "已触发 HLP 插件安装（异步进行，稍后刷新查看结果）",
                "HLP plugin install triggered (async; refresh to see the result)",
            )),
            Err(e) => conflict(&e),
        },
        ("POST", "/api/hlp/repair") => {
            match crate::installer::spawn_task("hlp-repair", || match crate::hlp_plugin::repair() {
                Ok(m) => (true, m),
                Err(e) => (false, e),
            }) {
                Ok(()) => ok_json(&crate::i18n::tr(
                    "已触发 HLP 插件修复（备份旧目录后重装，异步进行）",
                    "HLP plugin repair triggered (backup + reinstall, async)",
                )),
                Err(e) => conflict(&e),
            }
        }
        ("GET", "/api/hlp/apps") => (
            "200 OK",
            "application/json; charset=utf-8",
            hlp_apps_json(cfg),
        ),
        ("GET", "/api/hlp/data-dir") => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::json!({
                "path": crate::runtime::hlp_plugin_dir().join("data").display().to_string(),
            })
            .to_string(),
        ),
        ("POST", "/api/hlp/open-data-dir") => {
            let dir = crate::runtime::hlp_plugin_dir().join("data");
            let _ = std::fs::create_dir_all(&dir);
            #[cfg(target_os = "windows")]
            let opener = "explorer";
            #[cfg(target_os = "macos")]
            let opener = "open";
            #[cfg(all(unix, not(target_os = "macos")))]
            let opener = "xdg-open";
            match std::process::Command::new(opener).arg(&dir).spawn() {
                Ok(_) => ok_json(&format!("已打开 {}", dir.display())),
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json; charset=utf-8",
                    err_json(&format!("打开目录失败：{e}")),
                ),
            }
        }
        // ---------- 启动器配置（更新通道等） ----------
        ("GET", "/api/config") => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::json!({
                "update_channel": cfg.update_channel,
                "lang": cfg.lang,
                "exit_close_engine": cfg.exit_close_engine,
                "status_port": cfg.status_port,
            })
            .to_string(),
        ),
        ("POST", path) if path.starts_with("/api/config/update-channel/") => {
            let channel = &path["/api/config/update-channel/".len()..];
            if channel != "latest" && channel != "next" {
                (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    err_json("通道必须是 latest 或 next"),
                )
            } else {
                let mut new_cfg = cfg.clone();
                new_cfg.update_channel = channel.to_string();
                crate::config::save(&new_cfg);
                crate::updater::set_available(None);
                ok_json(&format!(
                    "{} {}（{}）",
                    crate::i18n::tr("更新通道已切换为", "Update channel switched to"),
                    channel,
                    crate::i18n::tr("下次检查更新生效", "takes effect on next update check")
                ))
            }
        }
        // ---------- 插件管理（2026-09-01 恢复：dsh-plugin-guide §发布期「进壳的插件市场一键装/卸」本就是壳的职责；收敛轮误删） ----------
        ("GET", "/api/plugins") => ("200 OK", "application/json; charset=utf-8", plugins_json()),
        ("POST", path)
            if path == "/api/plugin/install" || path.starts_with("/api/plugin/install?") =>
        {
            let src = query_str(path, "src");
            let id = query_str(path, "id");
            match install_plugin(src.as_deref(), id.as_deref()) {
                Ok(()) => ok_json(&crate::i18n::tr(
                    "已触发插件安装（异步进行，稍后刷新查看结果；装完需重启引擎生效）",
                    "Plugin install triggered (async; refresh to see the result; restart the engine after it finishes)",
                )),
                Err(e) => ("400 Bad Request", "application/json; charset=utf-8", err_json(&e)),
            }
        }
        ("POST", path) if path.starts_with("/api/plugin/uninstall/") => {
            let id = &path["/api/plugin/uninstall/".len()..];
            if id.is_empty() {
                (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    err_json(&crate::i18n::tr("缺少插件 id", "Missing plugin id")),
                )
            } else {
                match uninstall_plugin(id) {
                    Ok(msg) => ok_json(&msg),
                    Err(e) => (
                        "400 Bad Request",
                        "application/json; charset=utf-8",
                        err_json(&e),
                    ),
                }
            }
        }
        // 纯净卸载 dsh（不动壳）：keepData=0 → 连 %USERPROFILE%\.dsh 一起删（默认保数据）；
        // cleanShim=1 → 连 PATH 残留 shim 一起删（默认不删）。同步执行，返回完整卸载报告。
        // 注意：前端会带 query（?keepData=…&cleanShim=…），必须 starts_with 匹配而非精确匹配。
        ("POST", path)
            if path == "/api/dsh/uninstall" || path.starts_with("/api/dsh/uninstall?") =>
        {
            let keep_data = query_flag(path, "keepData", true);
            let clean_shim = query_flag(path, "cleanShim", false);
            let report = crate::uninstall::run_uninstall(keep_data, clean_shim);
            let body = serde_json::to_string(&report).unwrap_or_else(|_| err_json(&report.msg));
            if report.ok {
                ("200 OK", "application/json; charset=utf-8", body)
            } else {
                ("409 Conflict", "application/json; charset=utf-8", body)
            }
        }
        ("POST", "/api/install/node") => install_json("node"),
        ("POST", "/api/install/dsh") => install_json("dsh"),
        ("POST", "/api/start") => match crate::supervisor::start(cfg) {
            Ok(()) => ok_json(crate::i18n::tr("启动指令已下发", "Start command sent")),
            Err(e) => (
                "500 Internal Server Error",
                "application/json; charset=utf-8",
                err_json(&e),
            ),
        },
        ("POST", "/api/stop") => match crate::supervisor::stop() {
            Ok(()) => ok_json(crate::i18n::tr("关闭指令已下发", "Stop command sent")),
            Err(e) => (
                "500 Internal Server Error",
                "application/json; charset=utf-8",
                err_json(&e),
            ),
        },
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".to_string(),
        ),
    }
}

fn install_json(kind: &str) -> (&'static str, &'static str, String) {
    match crate::installer::start_install(kind) {
        Ok(()) => triggered_json(&format!(
            "{} {kind}",
            crate::i18n::tr("已触发安装", "Install triggered for")
        )),
        Err(e) => conflict(&e),
    }
}

// ---------- 插件管理（web profile 挂载：bundles + patches） ----------

/// 核心 bundle：卸载会破坏引擎，一律禁止。管理页据此不给卸载按钮（而不是点了才被拒）。
const CORE_BUNDLES: &[&str] = &["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"];

/// dsh profile 目录：壳启动的是 `dsh web` → 读 web profile 的挂载配置。
/// （P1-5 后 dsh 数据在 DSH_HOME/~/.dsh，profile 子目录结构不变）
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
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// 本地 patch 插件：解析 profile/cordis.patch.yml 的顶层条目。
/// 统一走 `patchyml::parse_entries`——此前这里的手写逐行扫描会把**嵌套** `- id:` 误当
/// 顶层条目、漏掉 `- name:` 在前 `id:` 换行的写法，与 doctor 对同一文件给出矛盾结论
/// （doctor 早已迁移，本函数是最后一个旧实现）。
fn parse_patches(profile_dir: &Path) -> Vec<serde_json::Value> {
    let Ok(content) = std::fs::read_to_string(profile_dir.join("cordis.patch.yml")) else {
        return Vec::new();
    };
    crate::patchyml::parse_entries(&content)
        .into_iter()
        .map(|e| {
            let n = e.name.unwrap_or_default();
            serde_json::json!({
                "id": e.id.unwrap_or_default(),
                "source": n,
                "local": n.starts_with("file:"),
            })
        })
        .collect()
}

/// 内置已验证插件清单（dsh-plugin-guide §发布期：「进壳的插件市场（内置 ✓已验证 清单）一键装/卸」）。
/// **从磁盘派生**：扫描 plugins/ 根下每个含 package.json 的子目录，id = 目录名，
/// desc = package.json 的 description 字段——壳不再硬编码清单，加插件只需把目录
/// 放进 plugins/（此前四行硬编码，加插件要改壳代码）。返回 {id, name, desc, src}。
fn builtin_marketplace() -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for root in plugin_root_candidates() {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join("package.json").is_file())
            .collect();
        if dirs.is_empty() {
            continue; // 这个锚点没有插件（目录不存在/为空）→ 试下一个锚点
        }
        dirs.sort(); // 按目录名稳定排序（原硬编码清单也是字母序）
        for dir in dirs {
            let id = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let desc = plugin_description(&dir).unwrap_or_else(|| id.clone());
            out.push(serde_json::json!({
                "id": id,
                "name": id,
                "desc": desc,
                "src": dir.display().to_string(),
            }));
        }
        break; // 首个有插件锚点生效（与 resolve_plugin_dir 的候选序语义一致）
    }
    out
}

/// 读插件目录 package.json 的 description（缺省/为空 → None，调用方回落目录名）。
fn plugin_description(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("description")?
        .as_str()
        .map(String::from)
        .filter(|s| !s.is_empty())
}

/// plugins/ 根目录候选锚点（resolve_plugin_dir 与 builtin_marketplace 共用）：
///   1. 壳数据目录 `root_dir()/plugins`（安装器/自更新可能把插件放这儿）；
///   2. exe 所在目录**向上逐层**找 `plugins/`（最多 4 层）：
///      - 生产：安装目录下 `dsh-come.exe` 旁就是 `plugins/`；
///      - 开发：`target/release/dsh-come.exe` 向上两层即仓库根 `plugins/`；
///   3. 当前工作目录（cargo run 时 cwd = 仓库根）。
fn plugin_root_candidates() -> Vec<PathBuf> {
    const UP_LEVELS: usize = 4;
    let mut cands: Vec<PathBuf> = Vec::new();
    cands.push(crate::runtime::root_dir().join("plugins"));
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..UP_LEVELS {
            let Some(d) = dir else { break };
            cands.push(d.join("plugins"));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("plugins"));
    }
    cands
}

/// 定位内置插件目录：在候选锚点下找 `<root>/<id>`。
/// 找不到返回 None → 前端把该项置灰（比给个点了必然失败的路径诚实）。
fn resolve_plugin_dir(id: &str) -> Option<PathBuf> {
    plugin_root_candidates()
        .into_iter()
        .map(|r| r.join(id))
        .find(|p| p.join("cordis.yml").is_file() || p.join("package.json").is_file())
}

/// 插件清单 JSON：bundle（市场/NPM）+ patch（本地 file://）+ 内置市场。
fn plugins_json() -> String {
    let dir = profile_dir();
    serde_json::json!({
        "profile": "web",
        "dir": dir.display().to_string(),
        "exists": dir.is_dir(),
        "bundles": parse_bundles(&dir),
        "patches": parse_patches(&dir),
        "market": builtin_marketplace(),
        // 下发给前端：核心包不给卸载按钮，而不是让用户点了才被后端拒绝
        "core": CORE_BUNDLES,
    })
    .to_string()
}

// ---------- 插件安装 / 卸载 ----------

/// 插件安装：`dsh plugin --profile web add <src>`（C5 契约，转发 pnpm add）。
/// src 缺失时用内置清单 id 解析路径；两者都无 → 报错。
/// 异步执行（npm/pnpm 下载可能较慢）；装完需重启引擎生效。
fn install_plugin(src: Option<&str>, id: Option<&str>) -> Result<(), String> {
    let src = match src {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => match id {
            Some(i) => resolve_plugin_dir(i)
                .map(|p| p.display().to_string())
                .ok_or_else(|| {
                    format!(
                        "未找到内置插件「{i}」的本地目录（候选：root_dir/plugins、exe 同级或上一级/plugins）。可改用 src 参数指定本地插件目录。"
                    )
                })?,
            None => return Err("缺少插件来源：请提供 src（本地目录路径）或 id（内置清单）".to_string()),
        },
    };
    if !Path::new(&src).is_dir() {
        return Err(format!("插件目录不存在: {src}"));
    }
    // 后台线程执行，结果写入安装状态（管理页轮询 /api/install/status）
    crate::installer::spawn_task("plugin", move || {
        // 带超时：pnpm 首次解析可能慢，给 5 分钟
        match run_dsh_capture(
            &["plugin", "--profile", "web", "add", src.as_str()],
            Duration::from_secs(300),
        ) {
            Ok(tail) => (true, format!("插件安装成功（{src}）。{tail}")),
            Err(e) => (false, format!("插件安装失败：{e}")),
        }
    })?;
    Ok(())
}

/// 插件卸载：先按本地 patch（cordis.patch.yml 条目）匹配，否则按市场 bundle（dsh plugin remove）。
/// 核心 bundle（dsh-base / dsh-web-app）禁止卸载——卸了引擎就废了。
fn uninstall_plugin(id: &str) -> Result<String, String> {
    let dir = profile_dir();
    if parse_patches(&dir)
        .iter()
        .any(|p| p["id"].as_str() == Some(id))
    {
        return uninstall_patch(&dir, id);
    }
    if parse_bundles(&dir).iter().any(|b| b == id) {
        if CORE_BUNDLES.contains(&id) {
            // 动态部分（id）用 format! 拼，静态模板走 i18n::tr（它只吃 &'static str）
            return Err(format!(
                "{id} {}",
                crate::i18n::tr(
                    "是 dsh 核心包，卸载会破坏引擎，已禁止",
                    "is a dsh core package; uninstalling it would break the engine"
                )
            ));
        }
        return uninstall_bundle(id);
    }
    Err(format!(
        "{} {id}",
        crate::i18n::tr("未找到插件：", "plugin not found:")
    ))
}

/// 本地 patch 卸载：备份后从 cordis.patch.yml 移除 `- id: <target>` 条目及其子行。
/// 重启引擎后生效（patch overlay 是启动时组装的）。
fn uninstall_patch(dir: &Path, target: &str) -> Result<String, String> {
    let path = dir.join("cordis.patch.yml");
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    // 统一保序编辑（patchyml::remove_entry）：只动目标条目行区间，注释与其余条目原样保留；
    // 旧手写循环只认 `- id:` 紧邻写法，`- name:` 在前 `id:` 换行的条目删不掉
    let new_text = crate::patchyml::remove_entry(&content, target)
        .ok_or_else(|| format!("cordis.patch.yml 中未找到条目: {target}"))?;
    // 备份原文件（可回滚）
    let bak = path.with_extension("patch.yml.bak");
    let _ = std::fs::copy(&path, &bak);
    // 已无有效条目 → 写注释空 patch（保持合法 YAML，dsh 读作空 overlay）
    let has_entry = crate::patchyml::parse_entries(&new_text).iter().any(|e| {
        let t = e.text.trim_start();
        t.starts_with("- insert:") || t.starts_with("- replace:")
    });
    let new_content = if has_entry {
        new_text + "\n"
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
    // 带超时：dsh plugin 可能慢（pnpm remove 解析），防挂死请求线程
    run_dsh_capture(
        &["plugin", "--profile", "web", "remove", id],
        Duration::from_secs(120),
    )
    .map(|tail| format!("已卸载 bundle「{id}」（重启引擎后生效）。{tail}"))
    .map_err(|e| format!("卸载 {id} 失败：{e}"))
}

/// 同步运行 `dsh <args>`（隐藏窗口 + 超时强杀）：Ok = stdout 摘要（tail），
/// Err = 运行器缺失/非零退出/超时。install_plugin 与 uninstall_bundle 的共用骨架
/// （此前两份各写一遍，错误文案还不一致）。
fn run_dsh_capture(args: &[&str], timeout: Duration) -> Result<String, String> {
    let Some(runner) = crate::runtime::dsh_runner() else {
        return Err("未找到系统 dsh 命令".to_string());
    };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut cmd = crate::runtime::dsh_command(&runner, &args);
    crate::supervisor::hide_window(&mut cmd);
    match crate::supervisor::capture_timeout(&mut cmd, timeout) {
        Some(out) => {
            let tail = crate::installer::tail_text(&out.stdout, &out.stderr);
            if out.status.success() {
                Ok(tail)
            } else {
                Err(format!("退出码 {:?}。{tail}", out.status.code()))
            }
        }
        None => Err(format!("超时（{} 秒）", timeout.as_secs())),
    }
}

/// 从请求 path 的 query 里解析布尔参数：`?keepData=0` / `?cleanShim=1`。
/// 缺失或无法解析 → 用 default。
fn query_flag(path: &str, key: &str, default: bool) -> bool {
    match query_str(path, key).as_deref() {
        Some("1" | "true" | "yes") => true,
        Some("0" | "false" | "no") => false,
        _ => default,
    }
}

/// 从请求 path 的 query 里取字符串参数（URL 解码）：`?src=<path>` / `?id=<id>`。
/// 缺失或空 → None。
fn query_str(path: &str, key: &str) -> Option<String> {
    let qi = path.find('?')?;
    for pair in path[qi + 1..].split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next() == Some(key) {
            let v = it.next().unwrap_or("");
            if v.is_empty() {
                return None;
            }
            return Some(url_decode(v));
        }
    }
    None
}

/// 简单 URL 解码（百分号解码）。src 路径可能含 %20 等转义（Windows 路径空格）。
/// 仅处理 %XX；+ 保持原样（query 里 + 才是空格，但路径里 + 更常见，宁缺勿错）。
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_flag_parses() {
        assert!(query_flag(
            "/api/dsh/uninstall?cleanShim=1",
            "cleanShim",
            false
        ));
        assert!(query_flag(
            "/api/dsh/uninstall?keepData=0&cleanShim=1",
            "cleanShim",
            false
        ));
        assert!(!query_flag(
            "/api/dsh/uninstall?keepData=0",
            "keepData",
            true
        ));
        assert!(!query_flag(
            "/api/dsh/uninstall?cleanShim=0",
            "cleanShim",
            true
        ));
        // 缺失 → 默认值
        assert!(query_flag("/api/dsh/uninstall", "keepData", true));
        assert!(!query_flag(
            "/api/dsh/uninstall?keepData=1",
            "cleanShim",
            false
        ));
        // 非法值 → 默认值
        assert!(query_flag(
            "/api/dsh/uninstall?keepData=maybe",
            "keepData",
            true
        ));
        assert!(!query_flag(
            "/api/dsh/uninstall?cleanShim=maybe",
            "cleanShim",
            false
        ));
    }

    /// `query_str` 读取 query 中的字符串值并做百分号解码。
    /// Windows 插件目录常带空格（如 `C:/Program Files/...`），前端会编码成 %20。
    #[test]
    fn query_str_reads_and_decodes() {
        assert_eq!(
            query_str("/api/plugin/install?src=C%3A%2Ftmp%2Fmy%20plug", "src").as_deref(),
            Some("C:/tmp/my plug")
        );
        // 多参数：取目标键，不受顺序影响
        assert_eq!(
            query_str("/api/plugin/install?id=recruit-tools&src=x", "id").as_deref(),
            Some("recruit-tools")
        );
        // 空值 / 无 query / 键不存在 → None（不返回空串，避免后端把空路径当有效输入）
        assert_eq!(query_str("/api/plugin/install?src=", "src"), None);
        assert_eq!(query_str("/api/plugin/install", "src"), None);
        assert_eq!(query_str("/api/plugin/install?id=a", "src"), None);
        // 路径里的 + 不解码（宁缺勿错：+ 在路径中是合法字符）
        assert_eq!(
            query_str("/api/plugin/install?src=a+b", "src").as_deref(),
            Some("a+b")
        );
    }

    /// 防回归（2026-09-01）：2026-08-29 收敛轮把版本管理与插件管理当「日常面」删掉，
    /// 导致管理页版本选择/插件装卸消失。dsh 换版本只能走 npm、profile 插件挂载本就是
    /// 壳的职责，二者都不属于「dsh 正常运行时能做的事」。这里锁住路由不许再消失。
    #[test]
    fn route_serves_version_and_plugin_apis() {
        let cfg = AppConfig::default();
        // 版本管理：200 即可（内容依赖 npm 网络，只保证路由存在且不是 404）
        let (code, _, _) = route("GET", "/api/dsh/versions", &cfg);
        assert_eq!(code, "200 OK", "GET /api/dsh/versions 不应 404");
        // 插件清单：本地文件读，可解析结构
        let (code, _, body) = route("GET", "/api/plugins", &cfg);
        assert_eq!(code, "200 OK", "GET /api/plugins 不应 404");
        let v: serde_json::Value = serde_json::from_str(&body).expect("plugins 应返回合法 JSON");
        for k in ["profile", "dir", "exists", "bundles", "patches", "market"] {
            assert!(v.get(k).is_some(), "plugins JSON 缺少字段 {k}");
        }
        assert!(v["market"].is_array(), "market 应是内置插件清单数组");
    }

    /// 防回归（2026-09-01）：后端 API 还在、前端入口没了，用户看到的就是「功能消失」。
    /// 这里锁住管理页必须保留版本卡片与插件卡片的 DOM 锚点。
    #[test]
    fn admin_page_exposes_version_and_plugin_controls() {
        let html = admin_html();
        for id in [
            "dshver",          // 版本状态行
            "ver-sel",         // 版本下拉
            "btn-upd",         // 更新到最新
            "btn-ver-install", // 安装所选版本
            "pl-sel",          // 内置插件清单
            "btn-pl-install",  // 安装所选插件
            "pl-src",          // 本地路径输入
            "btn-pl-src",      // 从路径安装
            "plugins",         // 已装插件列表
        ] {
            assert!(
                html.contains(&format!("id=\"{id}\"")),
                "管理页缺少控件 id=\"{id}\"（版本/插件入口被删？）"
            );
        }
    }

    /// 内置清单要能落到真实目录，否则前端只能把每一项都置灰、这条安装路径形同虚设。
    /// 仓库内 `plugins/` 可能因体积（node_modules）不入库，目录不在时跳过，避免 CI 误红。
    #[test]
    fn resolve_plugin_dir_finds_builtin_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins");
        if !root.is_dir() {
            eprintln!("skip: 工作区没有 plugins/ 目录（未入库？）");
            return;
        }
        let mut checked = 0;
        for id in [
            "recruit-tools",
            "recruit-workbench",
            "mcp-apps-host",
            "workbench",
        ] {
            if !root.join(id).is_dir() {
                continue;
            }
            assert!(
                resolve_plugin_dir(id).is_some(),
                "内置插件 {id} 在仓库里存在，却解析不到目录"
            );
            checked += 1;
        }
        assert!(checked > 0, "plugins/ 存在但没有任何已知内置插件目录");
    }

    /// 内置插件清单：每项都要有 id/name，供管理页下拉直接用。
    #[test]
    fn builtin_marketplace_items_are_complete() {
        let m = builtin_marketplace();
        assert!(
            !m.is_empty(),
            "内置清单不应为空（cargo test 的 cwd=仓库根，plugins/ 有 4 个插件）"
        );
        for item in m {
            assert!(item["id"].as_str().is_some(), "清单项缺 id: {item}");
            assert!(item["name"].as_str().is_some(), "清单项缺 name: {item}");
            // 磁盘派生：目录来自实际扫描，src 恒为存在的路径
            assert!(
                item["desc"].as_str().is_some_and(|s| !s.is_empty()),
                "清单项缺 desc: {item}"
            );
            assert!(item["src"].as_str().is_some(), "清单项缺 src: {item}");
        }
    }

    /// 插件清单从磁盘派生而非硬编码：目录名 → id，package.json.description → desc。
    /// 这里锚定 workbench（md-studio）确认派生链路成立。
    #[test]
    fn builtin_marketplace_derives_from_plugins_dir() {
        let m = builtin_marketplace();
        let ids: Vec<&str> = m.iter().filter_map(|i| i["id"].as_str()).collect();
        assert!(
            ids.contains(&"workbench"),
            "派生清单应含 workbench，实际 {ids:?}"
        );
        let wb = m.iter().find(|i| i["id"] == "workbench").unwrap();
        assert!(
            wb["desc"].as_str().unwrap().contains("工作台"),
            "desc 应来自 package.json.description，实际 {}",
            wb["desc"]
        );
    }

    /// bind_any(0)：OS 分配随机端口，返回的端口应 >0。
    #[test]
    fn bind_any_returns_ephemeral() {
        let (l, p) = bind_any(0).unwrap();
        assert!(p > 0, "随机端口应 >0，实际 {p}");
        drop(l);
    }

    /// 期望端口被占 → 自动回退到另一个端口（防与其他应用冲突）。
    #[test]
    fn bind_any_falls_back_when_taken() {
        let (l1, p1) = bind_any(0).unwrap(); // 占住 p1
        let (l2, p2) = bind_any(p1).unwrap(); // 请求 p1（被占）→ 应回退
        assert_ne!(p1, p2, "被占端口应回退到别的端口");
        assert!(p2 > 0);
        drop(l1);
        drop(l2);
    }

    /// 期望端口空闲 → 直接用期望端口（不回退）。
    #[test]
    fn bind_any_keeps_free_port() {
        let (l1, p1) = bind_any(0).unwrap();
        drop(l1); // 释放后 p1 空闲
        let (l2, p2) = bind_any(p1).unwrap();
        assert_eq!(p1, p2, "空闲端口应直接用期望值");
        drop(l2);
    }

    // ---------- P0-1：写请求的来源校验（Host + Origin + CSRF token） ----------

    fn hdrs<'a>(items: &'a [(&'a str, &'a str)]) -> Vec<(String, &'a str)> {
        items.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// 基准：本管理页发出的、带正确 token 的写请求应放行。
    #[test]
    fn write_request_from_admin_page_is_allowed() {
        let tok = csrf_token().to_string();
        for host in ["127.0.0.1", "localhost"] {
            let host_h = format!("{host}:3081");
            let origin_v = format!("http://{host}:3081");
            let raw = [
                ("host", host_h.as_str()),
                ("origin", origin_v.as_str()),
                (CSRF_HEADER, tok.as_str()),
            ];
            let h = hdrs(&raw);
            assert!(is_same_origin_local(3081, &h), "{host} 来源应放行");
        }
    }

    /// 任意网页发起的 POST（无 Origin/Host/token）必须被拒绝——这是 P0-1 的核心场景。
    #[test]
    fn cross_site_write_without_headers_is_rejected() {
        // 浏览器对 127.0.0.1 的简单 POST 不触发 preflight，请求会直接到达。
        // 三者缺一即拒，不能因为「都在本机」就放行。
        let empty: [(&str, &str); 0] = [];
        assert!(!is_same_origin_local(3081, &hdrs(&empty)));

        let only_host = [("host", "127.0.0.1:3081")];
        assert!(!is_same_origin_local(3081, &hdrs(&only_host)));

        let only_origin = [("origin", "http://127.0.0.1:3081")];
        assert!(!is_same_origin_local(3081, &hdrs(&only_origin)));

        let only_token = [(CSRF_HEADER, csrf_token())];
        assert!(!is_same_origin_local(3081, &hdrs(&only_token)));
    }

    /// DNS rebinding：攻击者域名解析到 127.0.0.1，浏览器填的 Host 仍是攻击者域名。
    #[test]
    fn dns_rebinding_host_is_rejected() {
        let tok = csrf_token().to_string();
        let evil = [
            ("host", "evil.example.com:3081"),
            ("origin", "http://evil.example.com:3081"),
            (CSRF_HEADER, tok.as_str()),
        ];
        assert!(
            !is_same_origin_local(3081, &hdrs(&evil)),
            "外部 Host 必须拒绝"
        );

        // 混合：Origin 对但 Host 不对（代理改写场景）同样拒绝
        let mixed = [
            ("host", "evil.example.com:3081"),
            ("origin", "http://127.0.0.1:3081"),
            (CSRF_HEADER, tok.as_str()),
        ];
        assert!(!is_same_origin_local(3081, &hdrs(&mixed)));
    }

    /// 缺 token 或 token 不匹配：即使来源正确也拒绝（第二道防线）。
    #[test]
    fn missing_or_wrong_csrf_token_is_rejected() {
        let base = [
            ("host", "127.0.0.1:3081"),
            ("origin", "http://127.0.0.1:3081"),
        ];
        assert!(!is_same_origin_local(3081, &hdrs(&base)), "缺 token 应拒绝");

        let wrong = [
            ("host", "127.0.0.1:3081"),
            ("origin", "http://127.0.0.1:3081"),
            (CSRF_HEADER, "deadbeef"),
        ];
        assert!(
            !is_same_origin_local(3081, &hdrs(&wrong)),
            "错 token 应拒绝"
        );
    }

    /// 端口不匹配（管理页实际端口与请求 Host 端口不一致）→ 拒绝。
    #[test]
    fn port_mismatch_is_rejected() {
        let tok = csrf_token().to_string();
        let h = [
            ("host", "127.0.0.1:9999"),
            ("origin", "http://127.0.0.1:9999"),
            (CSRF_HEADER, tok.as_str()),
        ];
        assert!(!is_same_origin_local(3081, &hdrs(&h)));
    }

    /// token 在进程内稳定（每次请求比对的是同一个值，否则正常操作会被自己拒绝）。
    #[test]
    fn csrf_token_is_stable_within_process() {
        assert_eq!(csrf_token(), csrf_token());
        assert_eq!(csrf_token().len(), 64, "SHA256 十六进制应为 64 字符");
    }

    // ---------- P1-4 / 请求解析健壮性 ----------

    /// 长 URL 不应被截断：修复前 4096 缓冲只读一次会截断成半个路径。
    #[test]
    fn parse_head_keeps_long_url_intact() {
        let long = "/api/plugin/install?src=".to_string() + &"x".repeat(300);
        let raw = format!("POST {long} HTTP/1.1\r\nHost: 127.0.0.1:3081\r\n\r\n");
        let (method, path, _) = parse_head(&raw);
        assert_eq!(method, "POST");
        assert_eq!(path, long, "长 URL 不应被截断");
    }

    /// header 名统一小写（HTTP 头大小写不敏感，查找时按小写匹配）。
    #[test]
    fn parse_head_lowercases_header_names() {
        let raw = "POST /api/stop HTTP/1.1\r\nHost: 127.0.0.1:3081\r\nOrigin: http://127.0.0.1:3081\r\nX-DSH-Come-Token: abc123\r\n\r\n";
        let (method, path, headers) = parse_head(raw);
        assert_eq!(method, "POST");
        assert_eq!(path, "/api/stop");
        assert_eq!(header(&headers, "host"), Some("127.0.0.1:3081"));
        assert_eq!(header(&headers, "origin"), Some("http://127.0.0.1:3081"));
        assert_eq!(header(&headers, CSRF_HEADER), Some("abc123"));
        assert_eq!(header(&headers, "x-dsh-come-token"), Some("abc123"));
    }

    /// GET/HEAD 视为安全方法（不要求来源校验），其余一律校验。
    #[test]
    fn only_get_and_head_are_safe_methods() {
        assert!(is_safe_method("GET"));
        assert!(is_safe_method("HEAD"));
        for m in ["POST", "PUT", "DELETE", "PATCH", "get", "OPTIONS"] {
            assert!(!is_safe_method(m), "{m} 不应被视为安全方法");
        }
    }

    // ---------- 端到端：真实 HTTP 请求打到管理页 ----------

    /// 发一个原始 HTTP 报文，返回 (状态码, 完整响应)。
    fn raw_request(port: u16, raw: &str) -> (u16, String) {
        use std::io::{Read as _, Write as _};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("连接管理页失败");
        s.write_all(raw.as_bytes()).unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let code = text
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (code, text)
    }

    /// 端到端验证 P0-1：起真实服务，模拟攻击报文，确认被 403 拦下。
    ///
    /// 用 `POST /api/stop`（保留的应急端点；无引擎运行时 `stop()` 只改进程内状态、
    /// 不落盘、不杀任何进程，返回 200）作为探针：
    /// - 403 = 被来源校验拦住
    /// - 200 = 通过了来源校验、进入 route 正常处理（同源正常路径）
    /// 这样既验证了拦截，又不会在测试里真的卸载 dsh 或删数据。
    #[test]
    fn end_to_end_cross_site_write_is_rejected() {
        let (listener, port) = bind_any(0).unwrap();
        set_admin_port(Some(port));
        let cfg = AppConfig {
            status_port: port,
            ..Default::default()
        };
        std::thread::spawn(move || serve_listener(listener, cfg));

        // 1) 攻击报文：浏览器对 127.0.0.1 的简单 POST 不发 preflight，裸 POST 直达
        let evil = format!(
            "POST /api/stop HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n\r\n"
        );
        let (code, _) = raw_request(port, &evil);
        assert_eq!(
            code, 403,
            "无 Origin/token 的跨站写请求必须 403，实际 {code}"
        );

        // 2) DNS rebinding：Host 是攻击者域名
        let rebound = format!(
            "POST /api/stop HTTP/1.1\r\nHost: evil.example.com:{port}\r\nOrigin: http://evil.example.com:{port}\r\nX-DSH-Come-Token: {}\r\nContent-Length: 0\r\n\r\n",
            csrf_token()
        );
        let (code, _) = raw_request(port, &rebound);
        assert_eq!(code, 403, "DNS rebinding 报文必须 403，实际 {code}");

        // 3) 本管理页的正常写请求：应通过校验并正常处理（stop 空闲引擎 → 200）
        let good = format!(
            "POST /api/stop HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nX-DSH-Come-Token: {}\r\nContent-Length: 0\r\n\r\n",
            csrf_token()
        );
        let (code, _) = raw_request(port, &good);
        assert_eq!(code, 200, "本页写请求应通过校验并正常处理，实际 {code}");

        // 4) GET 不受影响（无来源校验，否则管理页自身都打不开）
        let get = format!("GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        let (code, body) = raw_request(port, &get);
        assert_eq!(code, 200, "GET 应正常返回，实际 {code}");
        assert!(body.contains("nosniff"), "响应应带安全头");

        // 5) 分片发送：请求头跨 TCP 段也应正确解析（P1-4）
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"POST /api/stop HTTP/1.1\r\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        s.write_all(format!("Host: 127.0.0.1:{port}\r\n").as_bytes())
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        s.write_all(
            format!(
                "Origin: http://127.0.0.1:{port}\r\nX-DSH-Come-Token: {}\r\nContent-Length: 0\r\n\r\n",
                csrf_token()
            )
            .as_bytes(),
        )
        .unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let code: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        assert_eq!(code, 200, "分片请求应被完整读取并正常处理，实际 {code}");

        set_admin_port(None);
    }
}

fn ok_json(msg: &str) -> (&'static str, &'static str, String) {
    (
        "200 OK",
        "application/json; charset=utf-8",
        serde_json::json!({ "ok": true, "msg": msg }).to_string(),
    )
}

/// 「已触发 X」类响应的统一拼装：`{msg}（异步进行，稍后刷新查看结果）`。
/// 此前 4 处路由各自重复同一 i18n 拼接；后缀不同的站点（如插件安装）不强行套用。
fn triggered_json(msg: &str) -> (&'static str, &'static str, String) {
    ok_json(&format!(
        "{msg}（{}）",
        crate::i18n::tr(
            "异步进行，稍后刷新查看结果",
            "running asynchronously; refresh to see the result"
        )
    ))
}

/// 409 统一形态（安装互斥/状态冲突时，任务已在跑不能重复触发）。
fn conflict(e: &str) -> (&'static str, &'static str, String) {
    (
        "409 Conflict",
        "application/json; charset=utf-8",
        err_json(e),
    )
}

/// v1.4.0 P0-3：代理 dsh web 的 /hlp/apps/registry → 管理页 LocalApp 列表。
/// dsh 未启动/超时（5s）→ { error, apps: [] }（管理页显示友好提示，不报错）。
fn hlp_apps_json(cfg: &AppConfig) -> String {
    let url = format!("http://127.0.0.1:{}/hlp/apps/registry", cfg.port);
    let apps = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()
        .and_then(|c| c.get(&url).send().ok())
        .and_then(|r| r.json::<serde_json::Value>().ok())
        .and_then(|v| {
            v.get("apps").and_then(|a| a.as_array()).map(|arr| {
                arr.iter()
                    .map(|a| {
                        serde_json::json!({
                            "id": a.get("id").cloned().unwrap_or_default(),
                            "title": a.get("title").cloned().unwrap_or_default(),
                            "icon": a.get("icon").cloned().unwrap_or_default(),
                            "builtin": a.get("builtin").cloned().unwrap_or(serde_json::json!(false)),
                        })
                    })
                    .collect::<Vec<_>>()
            })
        });
    match apps {
        Some(list) => serde_json::json!({ "ok": true, "port": cfg.port, "apps": list }).to_string(),
        None => serde_json::json!({
            "ok": false,
            "error": crate::i18n::tr("dsh 未运行或未就绪，启动 dsh 后查看 App 列表", "dsh is not running; start dsh to see the app list"),
            "apps": [],
        })
        .to_string(),
    }
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "msg": msg }).to_string()
}

/// 组合状态：守护状态 + 环境探测 + 安装状态 + 界面语言（管理页 JS 据此切换文案）。
fn status_json(cfg: &AppConfig) -> String {
    let st = crate::supervisor::status();
    let eng = serde_json::to_value(&st).unwrap_or(serde_json::Value::Null);
    let env = crate::installer::probe();
    let install = crate::installer::install_state();
    serde_json::json!({ "eng": eng, "env": env, "install": install, "lang": cfg.lang }).to_string()
}

fn admin_html() -> String {
    // 开发期（仅 debug 构建）：若 exe 同级有 admin.html，读它（改完刷新即见，不重编译）。
    //
    // release 构建**绝不**读外部文件：否则任何人往 exe 目录放一个 admin.html
    // 就能往管理页注入任意 JS，而管理页能调卸载 dsh / 删数据 / 启停接口
    // ——那是一条完整的本地提权链，且行为随环境残留文件而不可预测。
    #[cfg(debug_assertions)]
    {
        if let Ok(exe) = std::env::current_exe() {
            let dev = exe.with_file_name("admin.html");
            if dev.is_file() {
                if let Ok(s) = std::fs::read_to_string(&dev) {
                    return s.replace(CSRF_PLACEHOLDER, csrf_token());
                }
            }
        }
    }
    // 生产：编译期内嵌（单文件 exe）+ 注入 CSRF token
    include_str!("../resources/admin.html").replace(CSRF_PLACEHOLDER, csrf_token())
}
