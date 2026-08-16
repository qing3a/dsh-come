# dsh-companion ↔ dsh CLI 契约（v1）

启动器只依赖以下稳定表面。任何一项被 upstream 破坏 → 显式升级本文件并 bump 启动器版本；
启动器**不**解析 CLI 输出、**不**读 dsh 内部文件、**不**碰插件 API（鸭子类型原则）。

| # | 契约 | 依赖方式 | 依据 |
|---|---|---|---|
| C1 | `dsh web --host <host> --port <port>` | spawn 子进程参数（web app flag 透传） | `apps/cli/tests/args.spec.ts`（官方测试锚定 `--host/--port`） |
| C2 | `GET http://127.0.0.1:<port>/` → HTTP 200 | 健康/就绪探测（v1 只看状态码；后续加版本指纹防 SW 陈旧 UI，见 DESIGN §7.4） | README「the command prints its URL」+ web profile 首屏 |
| C3 | 环境变量 `DSH_HOME=<home>` + 工作目录 `<home>` | spawn 时设置；profile/插件/配置全隔离在启动器 home | `apps/cli/README.md`「invoking directory 是默认工作区根」+ `$DSH_HOME/profiles/<name>` |
| C4 | `dsh --profile headless "job"` | v2 冒烟验证扩展（mock-llm waterfall） | `apps/cli/README.md` Entry modes |
| C5 | `dsh plugin --profile web <pnpm args>`（add/remove） | 插件市场安装/卸载；经 `node npx-cli.js @pkg@ver plugin ...` 调用，`dsh plugin` 转发给 profile 目录的 pnpm（PATH 注入捆绑 node 的 pnpm） | `apps/cli/README.md`「Manage a profile's plugins by forwarding to pnpm」 |

## 升级契约的显式步骤

1. 契约变化（flag 改名 / 端口默认值变 / DSH_HOME 语义变 / headless 退出码变）→ 先改本文件
2. bump 启动器版本（启动器与 DSH 版本解耦，见 DESIGN §6）
3. 旧版本标记 `known_bad`，新版本走「验证通过才切换」流程

## 已知稳定面（勿依赖）

- dsh 的 Web UI 内部路由 / API 结构（可能变，不解析）
- `dist/` 前端构建产物路径（SW 出现后可能有 `service-worker.js`，勿假设）
- CLI 的 stdout 文案（调试用，不解析）

## 变更记录

- 2026-08-14：v1 定稿（C1-C3 实现，C4 预留）。
