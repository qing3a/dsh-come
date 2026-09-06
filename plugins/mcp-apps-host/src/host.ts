/**
 * MCP-Apps 宿主核心（spike 子集）：对 app 充当 MCP server（postMessage 传输），
 * 对真实 MCP server 充当 client（代理 tools/resources）。
 *
 * 本文件是「协议层」的一部分（design doc §2）：升级 spec 版本只改这里与
 * protocol/ 下的消息处理，不改宿主接线。
 *
 * spike 范围（严格按能力协商）：
 * - 标准 MCP：initialize / ping / tools/list / tools/call / resources/read（代理到 server）
 * - MCP-Apps：ui/initialize（能力协商）/ ui/open-link / ui/download-file / ui/update-model-context
 * - sampling/createMessage：明确拒绝（hostCapabilities.sampling=false，app 应先查能力）
 * - notifications：tools/list_changed 记录后忽略（spike 无动态工具）
 */
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import {
  ErrorCode,
  type JSONRPCRequest,
  type JSONRPCResponse,
  type JSONRPCNotification,
  type Result,
} from '@modelcontextprotocol/sdk/types.js';

export interface AppsHostOptions {
  /** 已连接的（自管）MCP server client */
  serverClient: Client;
  log?: (line: string) => void;
}

export interface HostCapabilities {
  /** app 可否经宿主调用 server 工具（tools/call 代理） */
  serverTools: boolean;
  /** 宿主是否提供 sampling/createMessage（LLM 补全）——spike 为 false */
  sampling: boolean;
}

const PROTOCOL_VERSION = '2025-06-18';

export class AppsHostServer {
  private readonly log: (line: string) => void;

  constructor(private readonly opts: AppsHostOptions) {
    this.log = opts.log ?? (() => {});
  }

  /** 处理来自 app 的一条 JSON-RPC 消息；request 返回响应，notification 返回 undefined。 */
  async handleIncoming(raw: unknown): Promise<JSONRPCResponse | undefined> {
    if (!isRequest(raw)) {
      const n = raw as JSONRPCNotification;
      if (n && typeof n.method === 'string') {
        await this.relayNotification(n);
      }
      return undefined;
    }
    return this.handleRequest(raw);
  }

  private async handleRequest(req: JSONRPCRequest): Promise<JSONRPCResponse> {
    const { id, method, params } = req;
    const p = (params ?? {}) as Record<string, unknown>;
    try {
      switch (method) {
        case 'initialize': {
          // 标准 MCP 握手（spec：UI iframe 是 MCP client，postMessage 只是传输）
          this.log(`[apps] initialize client=${JSON.stringify(p.clientInfo ?? {})}`);
          return this.ok(id, {
            protocolVersion: typeof p.protocolVersion === 'string' ? p.protocolVersion : PROTOCOL_VERSION,
            capabilities: { tools: { listChanged: true } },
            serverInfo: { name: 'mcp-apps-host', version: '0.0.1-spike' },
          });
        }
        case 'ping':
          return this.ok(id, {});
        case 'ui/initialize': {
          // MCP-Apps 握手：app 声明 appCapabilities，host 如实声明 hostCapabilities + hostContext
          const appCaps = (p.appCapabilities ?? {}) as Record<string, unknown>;
          this.log(`[apps] ui/initialize appCapabilities=${JSON.stringify(appCaps)}`);
          return this.ok(id, {
            protocolVersion: PROTOCOL_VERSION,
            appCapabilities: appCaps,
            hostCapabilities: this.hostCapabilities(),
            hostContext: { locale: 'zh-CN', theme: 'system' },
          });
        }
        case 'tools/list':
          return this.ok(id, await this.opts.serverClient.listTools());
        case 'tools/call': {
          const name = String(p.name ?? '');
          const args = (p.arguments ?? {}) as Record<string, unknown>;
          this.log(`[apps] tools/call ${name}`);
          return this.ok(id, await this.opts.serverClient.callTool({ name, arguments: args }));
        }
        case 'resources/read':
          return this.ok(id, await this.opts.serverClient.readResource({ uri: String(p.uri ?? '') }));
        case 'ui/open-link': {
          const url = String((p as { url?: unknown }).url ?? '');
          this.log(`[apps] ui/open-link ${url}（spike：仅记录，不打开）`);
          return this.ok(id, {});
        }
        case 'ui/download-file':
          this.log('[apps] ui/download-file（spike：仅记录，不下载）');
          return this.ok(id, {});
        case 'ui/update-model-context':
          this.log('[apps] ui/update-model-context（spike：host 拒绝纳入模型上下文）');
          return this.ok(id, { ok: true, included: false });
        case 'sampling/createMessage':
          // hostCapabilities.sampling=false：按 spec app 应先查能力；这里明确拒绝
          return this.err(
            id,
            ErrorCode.MethodNotFound,
            'sampling/createMessage is not supported by this host (hostCapabilities.sampling=false)',
          );
        default:
          return this.err(id, ErrorCode.MethodNotFound, `unknown method: ${method}`);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      this.log(`[apps] ${method} failed: ${msg}`);
      return this.err(id, ErrorCode.InternalError, msg);
    }
  }

  private hostCapabilities(): HostCapabilities {
    // spike：代理 server 工具；不提供 sampling（Phase 2 接 ctx.llm 时打开）
    return { serverTools: true, sampling: false };
  }

  private async relayNotification(n: JSONRPCNotification): Promise<void> {
    this.log(`[apps] notification ${n.method}（spike：仅记录；Phase 1 中继 tools/list_changed 到 server）`);
  }

  private ok(id: JSONRPCRequest['id'], result: unknown): JSONRPCResponse {
    return { jsonrpc: '2.0', id, result: result as Result };
  }

  private err(id: JSONRPCRequest['id'], code: number, message: string): JSONRPCResponse {
    return { jsonrpc: '2.0', id, error: { code, message } };
  }
}

function isRequest(raw: unknown): raw is JSONRPCRequest {
  if (typeof raw !== 'object' || raw === null) return false;
  const d = raw as Record<string, unknown>;
  return d.jsonrpc === '2.0' && d.id !== undefined && typeof d.method === 'string';
}
