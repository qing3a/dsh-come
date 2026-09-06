/**
 * spike 用的宿主 HTTP 层：
 * - hostServer（端口 A）：宿主页（iframe→沙箱）+ /rpc 桥（app 消息 → AppsHostServer）+ /app-html
 * - sandboxServer（端口 B）：沙箱 proxy 页（**不同 origin**，spec 强制要求）
 *
 * 真实 dsh 集成时（Phase 1）：宿主页 = dsh web 路由（3080），沙箱 = 插件自起随机端口——
 * 结构不变，只换挂载点。
 */
import http from 'node:http';
import type { AppsHostServer } from './host.js';

export interface HttpServers {
  hostPort: number;
  sandboxPort: number;
  close(): void;
}

/**
 * 宿主页：沙箱 iframe + JSON-RPC 桥（app 消息 → POST /rpc → 回包给沙箱）。
 * 嵌入 JS 刻意不用模板字符串（避免与宿主模板字面量冲突）。
 */
const HOST_PAGE = `<!doctype html><html><head><meta charset="utf-8"><title>mcp-apps-host · spike host</title></head>
<body style="font-family:system-ui;padding:16px">
<h3>MCP-Apps Host（spike）</h3>
<p>宿主 origin: 本页 · 沙箱 origin: <span id="sbo">…</span></p>
<iframe id="sandbox" style="width:680px;height:460px;border:1px solid #ccc" sandbox="allow-scripts"></iframe>
<script>
var SANDBOX = document.getElementById('sandbox');
var sbo = document.getElementById('sbo');
var initialized = false;
fetch('/sandbox-port').then(function (r) { return r.text(); }).then(function (p) {
  sbo.textContent = 'http://127.0.0.1:' + p;
  SANDBOX.src = 'http://127.0.0.1:' + p + '/sandbox';
});
window.addEventListener('message', function (event) {
  var data = event.data;
  if (!data || typeof data !== 'object') return;
  if (event.source !== SANDBOX.contentWindow) return;
  if (data.method === 'ui/notifications/sandbox-proxy-ready' && !initialized) {
    initialized = true;
    fetch('/app-html').then(function (r) { return r.text(); }).then(function (html) {
      SANDBOX.contentWindow.postMessage({ jsonrpc: '2.0', method: 'ui/notifications/sandbox-resource-ready', params: { html: html } }, '*');
    });
    return;
  }
  if (data.method && data.method.indexOf('ui/notifications/sandbox-') === 0) return;
  // JSON-RPC 桥：转发到后端，响应回给沙箱（沙箱再转发给 view）
  fetch('/rpc', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ message: data })
  }).then(function (r) { return r.json(); }).then(function (resp) {
    if (resp && resp.response) SANDBOX.contentWindow.postMessage(resp.response, '*');
  });
});
</script>
</body></html>`;

/**
 * 沙箱 proxy 页（不同 origin）：收 host 的 sandbox-resource-ready（原始 HTML），
 * 渲染内层 view iframe（srcdoc），透明中继 view ↔ host 的非 sandbox-* 消息。
 * CSP：spike 用宽松的 script/style unsafe-inline（srcdoc 继承）；Phase 1 按 ui.csp 元数据构造。
 */
const SANDBOX_PAGE = `<!doctype html><html><head><meta charset="utf-8"><title>sandbox proxy</title></head>
<body style="margin:0;font-family:system-ui">
<script>
var parentWin = window.parent;
var view = null;
parentWin.postMessage({ jsonrpc: '2.0', method: 'ui/notifications/sandbox-proxy-ready' }, '*');
window.addEventListener('message', function (event) {
  var data = event.data;
  if (!data || typeof data !== 'object') return;
  if (event.source === view && view) {
    // 来自 view（内层 iframe）→ 转发给 host（sandbox-* 除外）
    if (data.method && String(data.method).indexOf('ui/notifications/sandbox-') === 0) return;
    parentWin.postMessage(data, '*');
    return;
  }
  // 来自 host
  if (data.method === 'ui/notifications/sandbox-resource-ready') {
    var html = data.params && data.params.html;
    if (typeof html !== 'string') return;
    view = document.createElement('iframe');
    view.sandbox = 'allow-scripts allow-same-origin';
    view.style.cssText = 'width:100%;height:100%;border:0';
    view.srcdoc = html;
    document.body.appendChild(view);
    return;
  }
  if (data.method && String(data.method).indexOf('ui/notifications/sandbox-') === 0) return;
  if (view) view.contentWindow.postMessage(data, '*');
});
</script>
</body></html>`;

export function startHttpServers(
  appHtml: string,
  host: AppsHostServer,
  opts: { hostPort?: number; sandboxPort?: number } = {},
): Promise<HttpServers> {
  return new Promise((resolve, reject) => {
    const sandbox = http.createServer((req, res) => {
      if (req.url === '/sandbox') {
        res.writeHead(200, {
          'content-type': 'text/html; charset=utf-8',
          // srcdoc 的 view 继承此 CSP；spike 允许内联脚本（真实实现按 ui.csp 元数据构造）
          'content-security-policy': "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'",
        });
        res.end(SANDBOX_PAGE);
        return;
      }
      res.writeHead(404);
      res.end();
    });
    sandbox.on('error', reject);
    sandbox.listen(opts.sandboxPort ?? 0, '127.0.0.1', () => {
      const sandboxPort = (sandbox.address() as { port: number }).port;
      const hostServer = http.createServer((req, res) => {
        try {
          if (!req.url) {
            res.writeHead(400);
            res.end();
            return;
          }
          if (req.method === 'GET' && req.url === '/') {
            res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
            res.end(HOST_PAGE);
            return;
          }
          if (req.method === 'GET' && req.url === '/app-html') {
            res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
            res.end(appHtml);
            return;
          }
          if (req.method === 'GET' && req.url === '/sandbox-port') {
            res.writeHead(200, { 'content-type': 'text/plain' });
            res.end(String(sandboxPort));
            return;
          }
          if (req.method === 'POST' && req.url === '/rpc') {
            let body = '';
            req.on('data', (c: Buffer) => {
              body += c.toString('utf8');
            });
            req.on('end', async () => {
              try {
                const parsed = JSON.parse(body || '{}') as { message?: unknown };
                const resp = parsed.message
                  ? await host.handleIncoming(parsed.message as never)
                  : undefined;
                res.writeHead(200, { 'content-type': 'application/json' });
                res.end(JSON.stringify({ response: resp ?? null }));
              } catch (e) {
                res.writeHead(500, { 'content-type': 'application/json' });
                res.end(JSON.stringify({ error: e instanceof Error ? e.message : String(e) }));
              }
            });
            return;
          }
          res.writeHead(404);
          res.end();
        } catch (e) {
          res.writeHead(500);
          res.end(String(e));
        }
      });
      hostServer.on('error', reject);
      hostServer.listen(opts.hostPort ?? 0, '127.0.0.1', () => {
        const hostPort = (hostServer.address() as { port: number }).port;
        resolve({
          hostPort,
          sandboxPort,
          close: () => {
            sandbox.close();
            hostServer.close();
          },
        });
      });
    });
  });
}
