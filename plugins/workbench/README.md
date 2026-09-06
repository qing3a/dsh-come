# workbench｜可自定义工作台插件（MVP 骨架）

通用化工作台：items（待办/笔记/任意条目）为示例领域，证明「工具面（AI 对话）与
页面 API 双通道同源」跑通。业务逻辑封装在 Workbench 类，工具 execute 与页面 mutate
都调 `wb.run` → 一份逻辑、三处调用。

## 三件套

| 件 | 职责 |
|---|---|
| Workbench 类 | 业务核心 + store.json 原子写 + 审计；`run(op, args, actor)` 统一分发 |
| TOOLS 数组 | 声明式工具定义（3 个示例），循环注册进 ctx.tools |
| webServer 路由 | `/workbench` 页面 + `/api/workbench` state/mutate API |

## 能力（3 个示例工具）

| 工具 | 作用 |
|---|---|
| wb_create_item | 创建条目（title/content/tags/done） |
| wb_list_items | 列出条目（按关键词/标签/完成态过滤） |
| wb_get_item | 获取单个条目详情 |

数据落 `$DSH_HOME/workbench/store.json`（可用 dataDir 配置覆盖），JSON 明文、原子写、
可审计。对话里登记的数据立刻出现在工作台页面，反之亦然。

## 加载（开发）

```powershell
# 隔离 home + 临时端口（避免污染生产 dsh）
$env:DSH_HOME = '<临时隔离目录>\home'
dsh web --patch C:/Users/Administrator/Desktop/dsh-come/plugins/workbench/cordis.yml --host 127.0.0.1 --port 3199
```

启动后：
- 工作台页面：http://127.0.0.1:3199/workbench
- AI 对话：http://127.0.0.1:3199/
- 启动日志出现 `[workbench] plugin loaded!` 即加载成功

## 验证双通道同源

1. 在 AI 对话里说「创建一个条目，标题：测试」→ 工具 `wb_create_item` 被调用
2. 打开 /workbench 页面 → 该条目已出现在列表（3s 轮询）
3. 在页面勾选完成 → 对话里问「列出条目」→ 该条目已是完成态

## 远程 MCP（零代码）

在 profile 的 `cordis.patch.yml` 声明 `@deepseek-ai/dsh-mcp-client` 实例，远程工具自动
注册进 `ctx.tools`，AI 对话与工作台均可调用：

```yaml
- insert:
    - id: mcp-remote-a
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        transport: 'streamable-http'
        serverName: 'my-remote'
        url: 'https://mcp.example.com/sse'
        toolCallTimeoutMs: 30000
        failOnStartupError: false
```

## 被外部 agent 调用

工作台 API（`/api/workbench`）是普通 HTTP，任何能发 HTTP 的 agent 可直连（路径 A，零
MCP，用调用方自己的 token）。需要 MCP 标准接入时，另写一个 dsh 插件用
`@modelcontextprotocol/sdk` 暴露 stdio/sse server（路径 B），不写在本插件里。

## 发布（npm）

构建产物为编译后的 JS（`lib/`），TS 源码只在开发时直挂（node_modules 下的 TS
无法被 Node 类型剥离加载，必须发布编译产物）：

```powershell
# 1. 构建（tsc → lib/，并拷贝 workbench.html）
npm run build
# 2. 发布（prepack 自动重新构建；包内带 dsh.bundle.patch 声明）
npm publish
```

安装（管理页工作台卡片即此命令，固定走 npm 渠道）：

```bash
dsh plugin --profile web add md-studio
```

`dsh plugin add` 检测到包声明 `dsh.bundle.patch` 后，自动把 `md-studio` 追加进
`dsh.profile.bundles` 层栈——一条命令完成「安装 + 挂载」，无需手改
cordis.patch.yml。

> 旧本地挂载残留：若 profile 的 cordis.patch.yml 里还挂着
> `name: 'file:///…/plugins/workbench/src/index.ts'`，先删掉再切 npm 渠道，
> 避免同一插件双挂载。

## 自定义

本骨架用通用 items 示范。替换为你的领域时：
1. 改领域模型（Item → 你的实体类型）
2. 改 Workbench 类的方法 + `run` 的 ops 映射
3. 改 TOOLS 数组（加工具只改这里）
4. 改 workbench.html 的 UI

三件套同源结构不变——工具与页面始终共用 `wb.run`。
