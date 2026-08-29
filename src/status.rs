//! 管理页（轻量 HTTP 服务，std 零依赖）：状态展示 + 安装/启停操作。
//!
//! 路由：
//! - `GET /`                     → 内嵌 HTML 管理页（状态卡片 + 按钮，JS 每 2s 轮询）
//! - `GET /api/status`           → { eng: 守护状态, env: node/npm/dsh/winget 探测, install: 安装状态 }
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
        .map(|v| hosts.iter().any(|h| v.eq_ignore_ascii_case(&format!("{h}:{port}"))))
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
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    })
}

fn route(method: &str, path: &str, cfg: &AppConfig) -> (&'static str, &'static str, String) {
    match (method, path) {
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", admin_html()),
        ("GET", "/api/status") => ("200 OK", "application/json; charset=utf-8", status_json(cfg)),
        ("GET", "/api/install/status") => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_string(&crate::installer::install_state()).unwrap_or_else(|_| "{}".into()),
        ),
        // 纯净卸载 dsh（不动壳）：keepData=0 → 连 %USERPROFILE%\.dsh 一起删（默认保数据）；
        // cleanShim=1 → 连 PATH 残留 shim 一起删（默认不删）。同步执行，返回完整卸载报告。
        // 注意：前端会带 query（?keepData=…&cleanShim=…），必须 starts_with 匹配而非精确匹配。
        ("POST", path) if path == "/api/dsh/uninstall" || path.starts_with("/api/dsh/uninstall?") => {
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
            Err(e) => ("500 Internal Server Error", "application/json; charset=utf-8", err_json(&e)),
        },
        ("POST", "/api/stop") => match crate::supervisor::stop() {
            Ok(()) => ok_json(crate::i18n::tr("关闭指令已下发", "Stop command sent")),
            Err(e) => ("500 Internal Server Error", "application/json; charset=utf-8", err_json(&e)),
        },
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".to_string()),
    }
}

fn install_json(kind: &str) -> (&'static str, &'static str, String) {
    match crate::installer::start_install(kind) {
        Ok(()) => ok_json(&format!(
            "{} {kind}（{}）",
            crate::i18n::tr("已触发安装", "Install triggered for"),
            crate::i18n::tr("异步进行，稍后刷新查看结果", "running asynchronously; refresh to see the result")
        )),
        Err(e) => ("409 Conflict", "application/json; charset=utf-8", err_json(&e)),
    }
}

/// 从请求 path 的 query 里解析布尔参数：`?keepData=0` / `?cleanShim=1`。
/// 缺失或无法解析 → 用 default。
fn query_flag(path: &str, key: &str, default: bool) -> bool {
    let Some(qi) = path.find('?') else {
        return default;
    };
    for pair in path[qi + 1..].split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next() == Some(key) {
            match it.next() {
                Some("1" | "true" | "yes") => return true,
                Some("0" | "false" | "no") => return false,
                _ => return default,
            }
        }
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_flag_parses() {
        assert!(query_flag("/api/dsh/uninstall?cleanShim=1", "cleanShim", false));
        assert!(query_flag("/api/dsh/uninstall?keepData=0&cleanShim=1", "cleanShim", false));
        assert!(!query_flag("/api/dsh/uninstall?keepData=0", "keepData", true));
        assert!(!query_flag("/api/dsh/uninstall?cleanShim=0", "cleanShim", true));
        // 缺失 → 默认值
        assert!(query_flag("/api/dsh/uninstall", "keepData", true));
        assert!(!query_flag("/api/dsh/uninstall?keepData=1", "cleanShim", false));
        // 非法值 → 默认值
        assert!(query_flag("/api/dsh/uninstall?keepData=maybe", "keepData", true));
        assert!(!query_flag("/api/dsh/uninstall?cleanShim=maybe", "cleanShim", false));
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
        assert!(!is_same_origin_local(3081, &hdrs(&evil)), "外部 Host 必须拒绝");

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
        assert!(!is_same_origin_local(3081, &hdrs(&wrong)), "错 token 应拒绝");
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
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port))
            .expect("连接管理页失败");
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
        assert_eq!(code, 403, "无 Origin/token 的跨站写请求必须 403，实际 {code}");

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
        let code: u16 = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
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
