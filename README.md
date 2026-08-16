# dsh-come｜DSH 伴侣

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 变成**双击即用的 Windows 桌面 App**——无需安装 Node、无需打开终端，首次运行自动完成一切。

> **面向谁**：想本地跑 DeepSeek agent，但被「装 Node + 敲命令行」劝退的人。开发者直接用官方 `npx @deepseek-ai/dsh web` 即可，本项目的价值在降低入门门槛。

> 🚀 **方向 v3（2026-08-16）**：只做 dsh-come，md-agent 的功能以 **dsh 官方插件形态**移植进本生态（猎头工作台 = `plugins/recruit-workbench` + `recruit-tools`）；Rust 壳专注托盘/守护/更新/市场。决策记忆见 `docs/memory.md`，插件模式速查见 `docs/dsh-plugin-guide.md`，市场收录见 `docs/market.md`。

## 三个卖点

- **双击即用**：下载即得，无需 Node/终端；首次运行自动下载 Node、安装 DSH、打开界面
- **更新稳妥**：DSH 目前迭代较快、兼容性会变——新版本先**冒烟验证**（临时端口 HTTP 200）再锁定切换，失败自动保留旧版，不影响使用
- **工作台市场**：**工作台优先**（场景完整的业务包，如猎头协作，点开即用）+ 单件工具（运行时验证 ✅，一键装/卸），不碰终端

## 快速开始

```bash
git clone https://github.com/qing3a/dsh-come
cd dsh-come
cargo run --release
```

首次运行自动完成：下载 portable Node（约 30MB）→ npx 安装 DSH → 启动 Web UI → **自动弹出独立窗口**（无地址栏，看起来就是桌面 App）。

之后：托盘图标常驻（官方 DSH logo），右键可打开界面 / 插件市场 / 检查更新 / 重启引擎 / 开机自启 / 退出。

## 它做什么

```
dsh-come.exe（Rust 单 exe，进程外 supervisor）
├── 自举安装   portable Node（官方源 + npmmirror 镜像兜底，纯 Rust 解压）
├── 引擎守护   spawn dsh web；崩溃自动重启（指数退避 + 健康期重置上限）；滚动日志
├── 版本管理   registry 检查 → 冒烟验证（临时端口 HTTP 200）→ 切换/回滚（known_bad）
├── 市场      工作台优先（场景分组 + 打开入口）+ 单件工具装/卸；内置兜底 + 远程清单合并
└── 托盘      官方 logo（浅/深色主题自适应）/ 自动开界面 / 开机自启（HKCU Run）
```

### 关键设计

| 决策 | 理由 |
|---|---|
| **npx 通道**（`npx @deepseek-ai/dsh@<ver>`） | 下载/缓存/解析交给 npm 生态，壳只维护一个版本号；`--yes` + 钉版号，不直接跟 latest |
| **验证通过才切换** | 新版本先冒烟（HTTP 200）再锁定，失败记 known_bad 并保留旧版 |
| **进程外 supervisor** | 崩溃自愈 / 托盘 / 日志全在壳里，DSH 更新不影响壳（参考 landlock-run 的 native 分发理念） |
| **数据隔离** | 全部落在 `%LOCALAPPDATA%\dsh-desktop`（含 `$DSH_HOME`），不污染系统安装 |

## 与 dsh-tray 的关系

[`dsh-tray`](https://github.com/qing3a/dsh-tray) 是 DSH **进程内**插件（托盘/气泡通知，随 DSH 生灭）；本项目的 **进程外** 壳（守护 DSH 进程）。两者互补不冗余：同一用户装了两边时，dsh-tray 检测到 dsh-come 会自动降级。

## 许可

MIT。托盘图标使用 DeepSeek Harness 官方 favicon（`apps/web/public/favicon.svg`，MIT 仓库资产）——**图标为 DeepSeek AI 商标，仅作引用，不暗示官方联名或支持**。

## Roadmap

- ✅ v1（当前）：自举安装 / 引擎守护 / 验证式更新 / 市场（工作台优先）/ 托盘（主题感知图标）/ `--app` 独立窗口 / 开机自启 / 首次向导 / 壳管理页
- 🔜 v2：全 waterfall 冒烟（收编 `dsh-plugin-verify` 引擎）、launcher 自更新、远程清单由验证引擎增量产出、公网市场浏览页（第二个工作台上架后）