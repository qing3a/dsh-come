/**
 * MCP-Apps 宿主 spike 验证（Phase 0）：
 *   [1] 自管连接：SDK Client ↔ 测试 MCP server（内存传输，真实 stdio 同构）
 *   [2] 发现 ui:// 资源 + 读取原始 HTML
 *   [3] 宿主 HTTP：宿主页 + 沙箱页（不同 origin）
 *   [4-9] 进程内仿真：fake window 对跑通 app(SDK Client over postMessage) ↔ 宿主
 *         initialize / ui/initialize / tools/list / tools/call（打通 server）/ sampling 被拒
 * 浏览器手动验证（iframe 渲染 + 沙箱中继）：spike 打印的 http://127.0.0.1:<hostPort>/
 */
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { createAppServer } from './app-server.js';
import { AppsHostServer } from '../src/host.js';
import { PostMessageTransport, type MessageTarget } from '../src/transport.js';
import { startHttpServers } from '../src/http.js';

/** fake window 对：a.postMessage 投递给 b 的监听器，反之亦然（进程内仿真 postMessage 环境） */
function createWindowPair(): [MessageTarget, MessageTarget] {
  const make = (deliver: (m: unknown) => void): MessageTarget => {
    const listeners: Array<(e: { data: unknown }) => void> = [];
    return {
      postMessage(message: unknown) {
        deliver(message);
      },
      addEventListener(_type: 'message', listener: (e: { data: unknown }) => void) {
        listeners.push(listener);
      },
      // 内部：投递到本窗口的监听器
      deliver(m: unknown) {
        for (const l of listeners) l({ data: m });
      },
    } as MessageTarget & { deliver(m: unknown): void };
  };
  const a = make((m) => (b as MessageTarget & { deliver(m: unknown): void }).deliver(m));
  const b = make((m) => (a as MessageTarget & { deliver(m: unknown): void }).deliver(m));
  return [a, b];
}

async function main(): Promise<void> {
  // [1] 自管连接：测试 MCP server（内存传输）
  const [serverT, clientT] = InMemoryTransport.createLinkedPair();
  const server = createAppServer();
  const client = new Client({ name: 'spike-host-client', version: '0.0.1' });
  await server.connect(serverT);
  await client.connect(clientT);
  console.log('[1] MCP server 连接 OK（自管 client）');

  // [2] 发现 ui:// 资源 + 读取 HTML
  const res = await client.listResources();
  const ui = (res.resources ?? []).find((r) => r.uri.startsWith('ui://'));
  if (!ui) throw new Error('未发现 ui:// 资源');
  console.log(`[2] 发现 ui:// 资源: ${ui.uri} (${ui.mimeType})`);
  const read = await client.readResource({ uri: ui.uri });
  const html = (read.contents[0] && 'text' in read.contents[0] && read.contents[0].text) || '';
  if (!html.includes('spike app')) throw new Error('ui 资源 HTML 内容异常');
  console.log(`[3] ui 资源 HTML 读取 OK（${html.length} 字节）`);

  // [4] 宿主 HTTP（宿主页 + 沙箱页，不同 origin）
  const host = new AppsHostServer({ serverClient: client, log: (l) => console.log('   host:', l) });
  const httpServers = await startHttpServers(html, host);
  console.log(
    `[4] 宿主页 http://127.0.0.1:${httpServers.hostPort}/  ← 浏览器手动验证（iframe + 沙箱中继）`,
  );
  console.log(`    沙箱页 http://127.0.0.1:${httpServers.sandboxPort}/sandbox（不同 origin）`);

  // [5-9] 进程内仿真：app（SDK Client over postMessage）↔ 宿主
  const [viewWin, hostWin] = createWindowPair();
  const appClient = new Client({ name: 'spike-app-sim', version: '0.0.1' });
  const appTransport = new PostMessageTransport(viewWin);
  const hostTransport = new PostMessageTransport(hostWin);
  hostTransport.onmessage = async (msg) => {
    const resp = await host.handleIncoming(msg);
    if (resp) void hostTransport.send(resp);
  };
  await appTransport.start();
  await hostTransport.start();
  await appClient.connect(appTransport);
  console.log(`[5] app 仿真: initialize 握手 OK（协议 ${appClient.getServerVersion()}）`);

  // [6-9] 裸 JSON-RPC 通道（第二对 fake window）：apps 层消息（ui/*）与代理/拒绝逻辑
  // 用裸消息验证——更贴近 spec（apps 层消息本就不在基础 SDK 语义内）。
  // 注：SDK 1.30 的响应 schema 校验在 zod 4.4.3 下有兼容问题（zod-compat isZ4Schema 判定 +
  // zod/v4-mini），SDK Client 仅保留 initialize 兼容性证明（[5] 已通过）——
  // 这是 Phase 1 的已知项：固定 zod 版本或绕过响应校验。
  const [viewWin2, hostWin2] = createWindowPair();
  const hostTransport2 = new PostMessageTransport(hostWin2);
  hostTransport2.onmessage = async (msg) => {
    const resp = await host.handleIncoming(msg);
    if (resp) void hostTransport2.send(resp);
  };
  await hostTransport2.start();
  let rawId = 1;
  const rawSend = (method: string, params: unknown): Promise<Record<string, unknown>> =>
    new Promise((resolve, reject) => {
      const id = rawId++;
      const listener = (event: { data: unknown }) => {
        const d = event.data as { id?: number; result?: unknown; error?: { code?: number; message?: string } };
        if (d && d.id === id) {
          if (d.result) resolve(d.result as Record<string, unknown>);
          else reject(new Error(`${d.error?.code ?? ''} ${d.error?.message ?? 'rpc error'}`.trim()));
        }
      };
      viewWin2.addEventListener('message', listener);
      viewWin2.postMessage({ jsonrpc: '2.0', id, method, params }, '*');
    });

  const uiInit = await rawSend('ui/initialize', { appCapabilities: { tools: { listChanged: false } } });
  console.log(`[6] app 仿真: ui/initialize → hostCapabilities=${JSON.stringify(uiInit.hostCapabilities)}`);

  const tools = await rawSend('tools/list', {});
  const toolNames = ((tools.tools as Array<{ name: string }>) || []).map((t) => t.name).join(', ');
  console.log(`[7] app 仿真（裸 JSON-RPC）: tools/list → ${toolNames}`);

  const call = await rawSend('tools/call', { name: 'echo', arguments: { text: 'hello from app sim' } });
  const callText = ((call.content as Array<{ type?: string; text?: string }>) || [])
    .map((c) => c.text ?? JSON.stringify(c))
    .join('');
  console.log(`[8] app 仿真（裸 JSON-RPC）: tools/call echo → ${callText}`);

  try {
    await rawSend('sampling/createMessage', {
      messages: [{ role: 'user', content: { type: 'text', text: 'hi' } }],
    });
    throw new Error('sampling 应被拒绝（hostCapabilities.sampling=false）');
  } catch (e) {
    if (e instanceof Error && e.message.startsWith('sampling 应被拒绝')) throw e;
    console.log(`[9] app 仿真（裸 JSON-RPC）: sampling 被拒（预期）→ ${(e as Error).message.slice(0, 90)}`);
  }

  httpServers.close();
  await appClient.close();
  await client.close();
  await server.close();
  console.log('\nSPIKE PASS ✅  浏览器手动验证: http://127.0.0.1:' + httpServers.hostPort + '/（已关闭）');
}

main().catch((e) => {
  console.error('SPIKE FAIL:', e);
  process.exit(1);
});
