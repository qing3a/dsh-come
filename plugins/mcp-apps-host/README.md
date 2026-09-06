# mcp-apps-host（spike）

MCP-Apps（[SEP-1865](https://modelcontextprotocol.io/seps/1865-mcp-apps-interactive-user-interfaces-for-mcp)）
宿主插件 spike：让 dsh 渲染 MCP server 分发的交互式应用界面（`ui://` 资源）。

设计文档：`docs/mcp-apps-host-design.md`（仓库根）。

## 运行 spike

```bash
npm install
npm run typecheck     # tsc 类型检查
npm run spike         # 进程内仿真验证（连 server → 发现 ui:// → 握手 → tools/call 打通）
```

spike 会打印宿主页 URL，浏览器打开可手动验证 iframe 渲染 + 沙箱中继（计数器 +
echo 工具调用 + sampling 拒绝）。

## 结构

```
src/
├── transport.ts   # postMessage Transport（SDK Transport 接口；浏览器/仿真通用）
├── host.ts        # 宿主核心：对 app 是 MCP server，对 server 是 client（spike 子集）
└── http.ts        # 宿主页 + 沙箱页（不同 origin）+ /rpc 桥
test/
├── app-server.ts  # 测试 MCP server（ui:// 资源 + echo 工具）
├── app-view.html  # 测试 app（裸 postMessage JSON-RPC，spec 示例实现）
└── spike.ts       # 验证入口
```

## 状态

- [x] Phase 0 spike（2026-08-27 PASS）：连 server → 发现 ui:// → 握手 → tools/call 打通
      （9 步全过，见 `npm run spike` 输出）
- [ ] Phase 1 核心宿主（发现/注册表/沙箱/CSP/生命周期/流式通知）
- [ ] Phase 2 agent 集成（app 工具进 ctx.tools、update-model-context、sampling→ctx.llm）
- [ ] Phase 3 打磨（主题/多 server/审计/断线恢复/上架 dsh-market）

## Spike 发现（Phase 1 已知项）

1. **SDK 1.30 响应校验 × zod 4.4.3 兼容问题**：`zod-compat` 的 `isZ4Schema` 判定 +
   `zod/v4-mini` 导入导致部分响应（ListToolsResultSchema、自定义方法）校验报
   `v3Schema.safeParse is not a function`。Phase 1 需固定 zod 版本或绕过响应校验层。
2. **apps 层消息走裸 JSON-RPC 更贴近 spec**：`ui/*` 方法不在基础 SDK 语义内，
   宿主用标准 JSON-RPC 处理即可；官方 SDK 兼容性由 initialize 握手证明（spike [5] 通过）。
