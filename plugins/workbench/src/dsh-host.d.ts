/**
 * dsh 宿主注入服务的类型增强。
 *
 * `webServer` 由 dsh 宿主（@deepseek-ai/dsh-host-webserver）注入，不在上游
 * cordis 的 Context 上，也不在 @deepseek-ai/dsh-tools 的模块增强里。这里按
 * 运行时实际形状做结构化镜像（与 dsh-better-sidebar 的 context-types 一致），
 * 仅覆盖本插件用到的成员。
 */
import type { Context } from '@deepseek-ai/cordis'

/** 路由处理器看到的请求（node:http IncomingMessage 的结构子集） */
export interface WbHttpRequest {
  url?: string
  method?: string
  headers: Record<string, string | string[] | undefined>
  on(event: string, listener: (...args: any[]) => void): unknown
}

/** 路由处理器写入的响应（node:http ServerResponse 的结构子集） */
export interface WbHttpResponse {
  writeHead(status: number, headers?: Record<string, string>): void
  end(body?: string | Uint8Array): void
}

/** 一条具名 webServer 路由（镜像 host-webserver 的 WebRoute） */
export interface WbWebRoute {
  kind: 'exact' | 'prefix'
  path: string
  handler: (req: WbHttpRequest, res: WbHttpResponse) => void | Promise<void>
}

/** webServer 服务面（本插件用到的 register） */
export interface WbWebServer {
  register(route: WbWebRoute): () => void
}

declare module '@deepseek-ai/cordis' {
  interface Context {
    webServer: WbWebServer
  }
}

export type { Context }
