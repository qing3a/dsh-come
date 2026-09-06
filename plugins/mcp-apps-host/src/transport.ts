/**
 * postMessage Transport：实现 @modelcontextprotocol/sdk 的 Transport 接口。
 *
 * 目标对象为鸭子类型（MessageTarget）：
 * - 浏览器：传 window.parent（view 侧）或 iframe.contentWindow（host 侧）；
 * - 进程内仿真（spike 测试）：传 fake window（EventTarget + postMessage 互投）。
 *
 * 这样 view 侧可以用标准 SDK Client + 本 transport 直连宿主——
 * 「最大化 MCP 兼容」的第一条：不手搓协议，postMessage 只是传输层。
 */
import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';
import type { JSONRPCMessage } from '@modelcontextprotocol/sdk/types.js';

export interface MessageTarget {
  postMessage(message: unknown, targetOrigin: string): void;
  addEventListener(type: 'message', listener: (event: { data: unknown }) => void): void;
}

export class PostMessageTransport implements Transport {
  onmessage?: (message: JSONRPCMessage) => void;
  onerror?: (error: Error) => void;
  onclose?: () => void;

  constructor(
    private readonly target: MessageTarget,
    private readonly origin = '*',
  ) {}

  start(): Promise<void> {
    this.target.addEventListener('message', (event) => {
      const data = event.data;
      if (isJsonRpcMessage(data)) {
        this.onmessage?.(data);
      }
    });
    return Promise.resolve();
  }

  send(message: JSONRPCMessage): Promise<void> {
    this.target.postMessage(message, this.origin);
    return Promise.resolve();
  }

  async close(): Promise<void> {
    this.onclose?.();
  }
}

function isJsonRpcMessage(data: unknown): data is JSONRPCMessage {
  if (typeof data !== 'object' || data === null) return false;
  const d = data as Record<string, unknown>;
  return d.jsonrpc === '2.0' && (d.id !== undefined || typeof d.method === 'string');
}
