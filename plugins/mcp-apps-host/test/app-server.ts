/**
 * spike 测试用 MCP server：声明一个 ui:// 资源（text/html;profile=mcp-app）+ 一个 echo 工具。
 * 用 SDK 的 Server 实现（与真实 server 同构），经 InMemoryTransport 与宿主 client 相连。
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import {
  CallToolRequestSchema,
  ListResourcesRequestSchema,
  ListToolsRequestSchema,
  ReadResourceRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';

export const UI_URI = 'ui://counter-app';

function readAppViewHtml(): string {
  const here = fileURLToPath(new URL('.', import.meta.url));
  return readFileSync(new URL('app-view.html', import.meta.url), 'utf8');
}

export function createAppServer(): Server {
  const server = new Server(
    { name: 'spike-app-server', version: '0.0.1' },
    { capabilities: { resources: {}, tools: {} } },
  );
  const uiHtml = readAppViewHtml();

  server.setRequestHandler(ListResourcesRequestSchema, async () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const resource: any = {
      uri: UI_URI,
      name: 'Counter App',
      description: 'spike 测试 app：计数器 + echo 工具（MCP-Apps ui:// 资源）',
      mimeType: 'text/html;profile=mcp-app',
      // spec：ui.csp 元数据声明（Phase 1 宿主据此构造 CSP；spike 空白名单）
      _meta: {
        ui: {
          csp: { resourceDomains: [], connectDomains: [], frameDomains: [], baseUriDomains: [] },
        },
      },
    };
    return { resources: [resource] };
  });

  server.setRequestHandler(ReadResourceRequestSchema, async (req) => {
    if (req.params.uri !== UI_URI) {
      throw new Error(`unknown resource: ${req.params.uri}`);
    }
    return { contents: [{ uri: UI_URI, mimeType: 'text/html;profile=mcp-app', text: uiHtml }] };
  });

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
      {
        name: 'echo',
        description: '回显一段文本（spike 用）',
        inputSchema: {
          type: 'object',
          properties: { text: { type: 'string' } },
          required: ['text'],
        },
      },
    ],
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const args = (req.params.arguments ?? {}) as Record<string, unknown>;
    const text = String(args.text ?? '');
    return {
      content: [{ type: 'text', text: `echo: ${text}` }],
      structuredContent: { echo: text },
    };
  });

  return server;
}
