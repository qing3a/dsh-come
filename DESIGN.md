# dsh-companion｜DSH 伴侣 — 设计文档

> 把 DeepSeek Harness（dsh）变成「双击即用」的 Windows 桌面 App：捆绑 Node、钉版安装、
> 自动更新（验证通过才切换）、崩溃自愈。面向懂点开发但不想碰 Node/终端的人。

状态：✅ 设计定案（2026-08-14；2026-08-16 增补：市场工作台化）。Rust 骨架、市场/向导/壳管理页均已实现。

---

## 1. 定位与命名（决策记录）

| 项 | 决定 | 理由 |
|---|---|---|
| 仓库名 | `dsh-companion`（2026-08-16 由 `dsh-desktop` 更名） | 保留 `dsh-` 家族前缀；`dsh-desktop` 已三方撞车（qing3a / dataelement / SnowCrescenter-tech 同名，其中一家定位/文案雷同），改名规避搜索混淆；`companion` 与产品名「DSH 伴侣」字面对应 |
| 产品显示名 | **DSH 伴侣**（2026-08-14 用户由「DSH 桌面版」更名） | 小白看到的托盘/窗口/README 首屏用中文；不用纯「DeepSeek」规避官方误导；「伴侣」贴切「伴随 DSH 的管家/入口」语义 |
| 目标用户 | 定位 A：懂点开发、被 CLI/Node 劝退的人 | 真正纯小白需要模板化 workspace（harness 侧问题），不是启动器能补的 |
| 生态角色 | **进程外 supervisor**，不是插件 | 不做任何进程内逻辑（waterfall / settings 热改 / ctx.inject 全留给 TS 插件） |
| 依赖方向 | launcher → verify（v2 收编验证引擎） | 单向依赖，不跨语言合并 |

命名备选（查证过，未选）：`dsh-box`（品牌化但需解释）、`dsh-go`（抽象且撞 Go 语言）、
`dsh-desktop` npm 名已被占用（SnowCrescenter-tech/dsh-desktop 发布，0.2.0）；本项目不发 npm（走 GitHub Releases 分发 exe），改名后 `dsh-companion` 在 npm/GitHub 均无占用。运行时数据路径保留 `%LOCALAPPDATA%\dsh-desktop` 不变（已有用户数据不迁移）。

## 2. 为什么不做「npx 自动最新」

`npx @deepseek-ai/dsh web` 是官方分发渠道，但对小白是坑：

1. **每次都是 latest**：README 明确 `THERE WILL BE COMPATIBILITY-BREAKING CHANGES`，upstream 一 break 桌面应用直接打不开，且无回滚心智。
2. **无法预先验证**：我们的差异化资产是运行时验证（dsh-plugin-verify / mock-llm 方法论），npx 把「先验证再给小白」这一步跳过了。
3. **依赖网络**：npx 每次查 registry；国内网络下这是真实痛点（见 dsh-radar 记录）。

**结论：npx 是分发渠道，不是更新策略。** 借用 npm registry 当更新源，但由启动器做「单包版本管理器」：
验证通过才切换，否则自动回滚。

## 3. 架构总览

```
dsh-companion.exe（Rust 单 exe，Windows 优先）
├── supervisor   spawn dsh web（npx 通道）；崩溃重启（退避+上限）；滚动日志
├── tray         托盘图标/菜单（打开界面/插件市场/检查更新/日志/退出）；自动开浏览器
├── updater      npm registry 版本检查 → 冒烟验证 → 切换/回滚（state.current）
└── plugins      市场：**工作台优先**（kind=workbench 按场景分组 + 打开入口）+ 单件工具（装/卸，契约 C5）；
                清单 = 内置兜底 + 远程 verified-plugins.json 合并（离线回退，见 §7.6）
```

运行时目录布局：

```
%LOCALAPPDATA%\dsh-desktop\
├── node\                    # 捆绑 portable Node ≥22（自带 npm/npx）
├── home\                    # 启动器自己的 $DSH_HOME（profile/插件/配置隔离）
├── state.json               # 当前锁定版本 / known_bad / 验证历史
└── logs\
```

### 分发通道（2026-08-14 决策：npx 通道取代版本目录）

DSH 包本体经 **npx 通道** 分发：`node npx-cli.js --yes @deepseek-ai/dsh@<ver> web ...`
（npx-cli.js 是 js 非 .cmd，node 直启免 cmd /C 包装）。下载/缓存/解析全交给 npm 生态，
壳只维护 state.current 一个版本号。**区别于盲用 npx**：`--yes` 自动确认 + 版本号钉死 +
验证通过才写入 state.current。回滚依赖 npx 缓存（保留旧版则离线可用），换取实现大幅简化。

## 4. 与 dsh 的契约面（最小化并钉死）

启动器只依赖以下稳定表面，不解析 CLI 输出、不读内部文件、不碰插件 API：

| # | 契约 | 说明 |
|---|---|---|
| C1 | `dsh web --host 127.0.0.1 --port <固定端口>` | flag 透传给 web app（已查证 `args.spec.ts`） |
| C2 | HTTP GET 该端口 → 200 | 健康探测 |
| C3 | `$DSH_HOME` 指向 `home\` | 数据隔离 + 多版本并存的基础 |
| C4 | `dsh --profile headless "job"` | 冒烟验证用（v1 轻量：启动+干净退出；v2 全 waterfall） |
| C5 | `dsh plugin --profile web <add/remove 包名>` | 市场装/卸（转发到 profile 的 pnpm；插件装进 home\，不碰 dsh 包本体） |

契约随代码版本化，独立成 `docs/cli-contract.md`（参考 landlock-run 的做法）：
upstream break 时是显式升级契约，而不是暗中碎掉。

## 5. 更新流程（验证通过才切换）

```
用户点「检查更新」或启动时后台静默检查
  → GET registry.npmjs.org/@deepseek-ai/dsh → dist-tags.latest
  → 有新版本 → 冒烟验证（npx 自动下载/缓存该版本）：
      v1: npx 起 dsh web 到临时端口 + HTTP 200 + 杀树干净退出
      v2: 复用 mock-llm 跑 waterfall 链（把 dsh-plugin-verify 引擎收编成库）
  → 通过 → state.json 切换 default + 标记「已验证」（下次启动生效；运行中提示重启）
  → 失败 → 标记「此版本不可用」，保留旧版本号回滚，小白无感知
```

### 可信版本通道（v2，差异化护城河）

- **channel A（小白默认）**：`verified.json`（GitHub raw）——我们验证通过的版本清单；小白只看到「✓ 已验证」。
- **channel B（进阶）**：registry 全量，手动指定任意版本。
- 已有资产直接喂给 verified.json：dsh-event-auditor 的「74 事件 / 12 waterfall」、dsh-plugin-verify 的报告。

## 6. 版本策略

- **启动器与 DSH 版本解耦**：exe 更新走 GitHub Releases（manifest + 原子替换）；DSH 的已验证清单走 GitHub raw JSON。launcher 发版不卡 DSH 验证，反之亦然。
- 小白默认跟随「已验证」版本，不追 latest。
- 上一版本保留 = 回滚通道（一键回滚按钮 v2）。

## 7. 对上游契约漂移的韧性

1. **接触面最小 + 契约文档化**（见 §4）
2. **鸭子类型而非类型绑定**：只依赖稳定表面（auditor 已验证 `ctx.inject` 思路）
3. **自更新与 DSH 版本解耦**（见 §6）
4. **PWA/SW 风险**（2026-08-14 查证）：官方已交付 manifest（`manifest.webmanifest` + e2e 钉死，`display: fullscreen`），
   **尚无 service worker**。若将来官方补 sw 离线缓存，会与「启动器切换 DSH 版本」冲突（陈旧 UI 缓存）。
   对策：健康探测不只看 HTTP 200，记录页面版本指纹（C2 契约演进），升级后若指纹变化则提示刷新/强制 reload。

## 7.5 PWA 定位（决策记录）

PWA 是「界面层」的事，不是「启动器层」的事，不改变本设计任何架构：

- **含义 A（启动器本身做成 PWA）**：❌ 架构矛盾——PWA 不能 spawn 子进程 / 托盘常驻 / 开机自启 / 崩溃自愈 / 本地版本管理，这些恰是 dsh-companion 的存在理由。
- **含义 B（Web UI 安装成独立窗口）**：✅ 官方已铺路（localhost 是 secure context，manifest 在），我们**消费**它而不是造它。
- **含义 C（补 sw 离线缓存）**：❌ 对 localhost 服务无意义且有害（陈旧 UI 缓存）。

启动器侧行为：v2 打开界面用 `--app` 独立窗口（`msedge --app=http://127.0.0.1:PORT` / chrome 同理，无地址栏+任务栏图标），
首次向导可选提示「安装为桌面应用」（引导浏览器安装按钮，纯文案）。

## 7.6 工作台市场（决策记录，2026-08-16）

**问题**：市场（托盘「市场」菜单 + 壳管理页）收什么、怎么定义商品？参照 dsh-market 类生态市场
（浏览端 + 策展清单，安装由用户/壳执行），但 dsh-market 是泛目录（按 star 排序），工作台这类
「工具 + 会话 UI + 业务规则」整包在泛目录里没有差异表达。

**决策**：市场升级为**工作台优先**的策展市场（收录制，不开放提交）：

| 项 | 决定 |
|---|---|
| 商品形态 | `workbench`（工作台：场景完整的业务包，可含 UI/工具/外部服务依赖）优先展示；`tool`（单件工具）保留次要入口 |
| 打开语义 | 工作台（有 entry）= 壳**直接打开入口**（file:// 本地资产 / http URL），不做 npm 安装；工具 = `dsh plugin` 装/卸（C5） |
| 清单字段 | `kind` / `scenario`（第一层分组）/ `entry` / `requires`（外部服务依赖，打开前提示）/ `verify_evidence`（验证证据） |
| 清单来源 | 内置 `builtin_marketplace()` 兜底 + 远程 `verified-plugins.json`（GitHub raw）启动后台拉取合并（离线/失败静默回退） |
| 收录标准 | 场景完整 + 验证有据（e2e 全绿 / dsh-plugin-verify 报告）+ 形态不限（dsh 插件或本地资产均可，升级是商品项目自己的事） |
| 首个工作台 | 猎头协作（`md-hr`，外部商品：仅收录清单条目，不读改其文件） |

**边界**：市场是「清单 + 打开/装/卸」，**不是商店**——不做评分/账号/支付/评论（见 §9）；
壳不代启动工作台的外部依赖服务（requires 打开前提示即可）。
收录/上架流程详见 `docs/market.md`。

## 8. MVP 范围

| 模块 | v1（已实现） | v2 |
|---|---|---|
| 捆绑 Node 自举安装 + npx 通道 | ✅ | |
| 固定端口 + HTTP 健康探测 | ✅ | |
| 崩溃重启（退避+健康期重置）+ 滚动日志 | ✅ | |
| registry 检查 + 冒烟验证 + 切换/回滚 | ✅ | |
| 市场（工作台优先 + 单件工具装/卸） | ✅（2026-08-16：workbench 形态/场景分组/打开入口/依赖提示） | 验证引擎增量产出 |
| 托盘（官方 logo 主题感知 / 悬停稳定 / 状态行分阶段） | ✅ | |
| 自动开浏览器 + `--app` 独立窗口（桌面 App 化） | ✅ | |
| 重启引擎 + 开机自启（HKCU Run） | ✅ | |
| 全 waterfall 冒烟验证（verify 引擎收编） | | ✅ |
| verified-plugins.json 远程清单 | ✅（2026-08-16：合并 + 缓存 + 回退） | 验证引擎增量产出 |
| 首次向导（API key/工作区写进 home\） | ✅ 实现（2026-08-16，待收编提交） | |
| launcher 自更新（GitHub Releases） | 手动替换 | ✅ |
| 壳管理页（/desktop 页面：版本/插件/日志/工作台） | ✅（2026-08-16） | |

## 9. 非目标（边界，永远不做）

- 不做任何进程内 harness 逻辑（waterfall / settings / 插件 API）
- 不自动追踪 latest
- 不做多 harness / 跨平台（Windows 优先；非 win32 可后续）
- 不冒充官方产品（名称、图标、文案都标明是社区分发层）
- **不做商店**：市场永远是「清单 + 打开/装/卸」，无评分/账号/支付/评论；工作台的外部依赖服务由用户自启（requires 打开前提示，壳不代启动）

## 10. 参考与复用

| 来源 | 复用点 |
|---|---|
| `md-agent`（`Desktop\md-agent`） | tray-icon + winit 桌面壳已验证；`dist/md-agent.exe` 打包模式 |
| `md-agent\src\engine.rs` | **supervisor 完整参考**（318 行）：`cmd /C` 包装、日志重定向防阻塞、1s 轮询 + 限次自动重启、`taskkill /T /F` 杀进程树、退出清理钩子、start 幂等。差异：dsh-companion 走 npx 通道（node 直启 npx-cli.js，免 cmd /C）+ 重启**退避**（md-agent 是固定 1s）+ HTTP 就绪探测 |
| `dsh-tray` | EBUSY / 双托盘 / iconPath ENOENT 踩坑；headless 降级 |
| `native/landlock-run` | cli-contract.md 模式；平台分包发布模式 |
| `dsh-plugin-verify` | 验证引擎（v2 收编为库 `@qing3a/dsh-runtime-verify`） |
