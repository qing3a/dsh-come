# dsh-come｜DSH 伴侣 — 设计文档

> 把 DeepSeek Harness（dsh）变成「双击即用」的 Windows 桌面 App：捆绑 Node、钉版安装、
> 自动更新（验证通过才切换）、崩溃自愈。面向懂点开发但不想碰 Node/终端的人。

状态：✅ 设计定案（2026-08-14；2026-08-16 增补：市场工作台化）。Rust 骨架、市场/向导/壳管理页均已实现。

> ⚠️ **2026-08-17 瘦身定案（当前方向）**：本文主体（§1–§7.6）记录的是 **v1 完整版设计**
> （捆绑 Node / 版本管理 / 内置市场 / 状态页 / 向导页 / `--app` 独立窗口），这些能力已按
> `docs/slimming-plan.md` **全部移除**——壳现在只做托盘 + 进程守护 + 极简启停（6 文件约 1.2k 行）。
> 实际架构与契约以 `docs/cli-contract.md`（v2：跟随系统 dsh、不管理版本、`--patch` overlay）
> 和 `docs/slimming-plan.md` 为准；本文 §3/§5/§6/§7.6/§8 中与实现不符的描述视为**历史决策记录**，
> 需要追溯动机时查阅，不再作为当前实现的依据。

---

## 1. 定位与命名（决策记录）

| 项 | 决定 | 理由 |
|---|---|---|
| 仓库名 | `dsh-come`（2026-08-16 更名：`dsh-desktop` → `dsh-companion` → `dsh-come`） | 保留 `dsh-` 前缀；`dsh-desktop` 三方撞车（qing3a / dataelement / SnowCrescenter-tech 同名）故弃；`companion` 与中文「伴侣」对应但偏长，二次定名 `come`；改名均发生在任何公开引用前，无迁移代价 |
| 产品显示名 | **DSH 伴侣**（2026-08-14 用户由「DSH 桌面版」更名） | 小白看到的托盘/窗口/README 首屏用中文；不用纯「DeepSeek」规避官方误导；「伴侣」贴切「伴随 DSH 的管家/入口」语义 |
| 目标用户 | 定位 A：懂点开发、被 CLI/Node 劝退的人 | 真正纯小白需要模板化 workspace（harness 侧问题），不是启动器能补的 |
| 生态角色 | **进程外 supervisor**，不是插件 | 不做任何进程内逻辑（waterfall / settings 热改 / ctx.inject 全留给 TS 插件） |
| 依赖方向 | launcher → verify（v2 收编验证引擎） | 单向依赖，不跨语言合并 |

命名备选（查证过，未选）：`dsh-box`（品牌化但需解释）、`dsh-go`（抽象且撞 Go 语言）、
`dsh-desktop` npm 名已被占用（SnowCrescenter-tech/dsh-desktop 发布，0.2.0）；本项目不发 npm（走 GitHub Releases 分发 exe），定名 `dsh-come`（npm/GitHub 均无占用，待占位注册）。运行时数据路径保留 `%LOCALAPPDATA%\dsh-desktop` 不变（已有用户数据不迁移）。

## 2. 为什么不做「npx 自动最新」

`npx @deepseek-ai/dsh web` 是官方分发渠道，但对小白是坑：

1. **每次都是 latest**：README 明确 `THERE WILL BE COMPATIBILITY-BREAKING CHANGES`，upstream 一 break 桌面应用直接打不开，且无回滚心智。
2. **无法预先验证**：我们的差异化资产是运行时验证（dsh-plugin-verify / mock-llm 方法论），npx 把「先验证再给小白」这一步跳过了。
3. **依赖网络**：npx 每次查 registry；国内网络下这是真实痛点（见 dsh-radar 记录）。

**结论：npx 是分发渠道，不是更新策略。** 借用 npm registry 当更新源，但由启动器做「单包版本管理器」：
验证通过才切换，否则自动回滚。

## 3. 架构总览（v1 历史设计；当前实现见 slimming-plan §3：6 文件 / 1.2k 行，无 updater/plugins/status_page）

```
dsh-come.exe（Rust 单 exe，Windows 优先）
├── supervisor   spawn dsh web（npx 通道）；崩溃重启（退避+上限）；滚动日志
├── tray         托盘图标/菜单（打开界面/插件市场/检查更新/日志/退出）；自动开浏览器
├── updater      npm registry 版本检查 → 冒烟验证 → 切换/回滚（state.current）
└── plugins      插件市场 dsh-market（托盘一键安装/打开 → Settings → Plugin Market，800+ 社区插件）
                 + 工作台（kind=workbench 场景分组 + 打开入口）；清单 = 内置兜底 + 远程合并（见 §7.6）
```

运行时目录布局（v1 历史；当前仅 config.json / logs / come.patch.yml 三个用途）：

```
%LOCALAPPDATA%\dsh-desktop\
├── node\                    # 捆绑 portable Node ≥22（自带 npm/npx）——v1；已移除
├── home\                    # 启动器自己的 $DSH_HOME（profile/插件/配置隔离）——v1；已移除
├── state.json               # 当前锁定版本 / known_bad / 验证历史——v1；已移除
└── logs\
```

### 分发通道（v1 历史决策：npx 通道；2026-08-17 改为「跟随系统 dsh」）

DSH 包本体经 **npx 通道** 分发：`node npx-cli.js --yes @deepseek-ai/dsh@<ver> web ...`
（npx-cli.js 是 js 非 .cmd，node 直启免 cmd /C 包装）。下载/缓存/解析全交给 npm 生态，
壳只维护 state.current 一个版本号。**区别于盲用 npx**：`--yes` 自动确认 + 版本号钉死 +
验证通过才写入 state.current。回滚依赖 npx 缓存（保留旧版则离线可用），换取实现大幅简化。
→ 2026-08-17：不再维护版本号，改由系统 `dsh` 直启、npx 仅作回退通道（cli-contract v2 C1）。

## 4. 与 dsh 的契约面（最小化并钉死；v1 表格，2026-08-17 起以 docs/cli-contract.md v2 为准——C3 由 DSH_HOME 隔离改为 `--patch` overlay，C5 由转发捆绑 pnpm 改为系统 dsh 直启）

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

## 5. 更新流程（v1 历史设计；2026-08-17 移除——不再检查 registry / 冒烟验证 / 切换回滚，见 cli-contract v2「不管理版本」）

```
启动延迟 10s 静默检查 或 用户点「检查更新」
  → GET registry.npmjs.org/@deepseek-ai/dsh → dist-tags.latest
  → 有新版本 → 冒烟验证（npx 自动下载/缓存该版本）：
      v1: npx 起 dsh web 到临时端口 + HTTP 200 + 杀树干净退出
      v2: 复用 mock-llm 跑 waterfall 链（把 dsh-plugin-verify 引擎收编成库）
  → 通过 → state.pending + 标记「已验证」（菜单「应用更新」由用户确认后切换；运行中提示重启）
  → 失败 → 标记「此版本不可用」（known_bad），保留旧版本号回滚，小白无感知
```

**静默检查与回滚（2026-08-17）**：启动 10s 后后台静默检查一次（不阻塞启动；首次引导跳过），
发现新版本且验证通过 → 托盘菜单「应用更新」+ 状态行 ⚑ 提示，**确认才切换**（不打断会话）；
离线/已最新/失败全静默仅记日志。应用更新时旧版本记入 `state.previous`，托盘「回滚到 vX」
一键切回（current ↔ previous 交换，可来回切换，切换后自动重启引擎）——升级后新版实际跑不起来
时，不用手动改 state.json。

### 可信版本通道（v2，差异化护城河）

- **channel A（小白默认）**：`verified.json`（GitHub raw）——我们验证通过的版本清单；小白只看到「✓ 已验证」。
- **channel B（进阶）**：registry 全量，手动指定任意版本。
- 已有资产直接喂给 verified.json：dsh-event-auditor 的「74 事件 / 12 waterfall」、dsh-plugin-verify 的报告。

## 6. 版本策略（v1 历史设计；2026-08-17 移除——launcher 发版走 GitHub Releases 不变，但 DSH 版本跟随系统 npm，见 cli-contract v2）

- **启动器与 DSH 版本解耦**：exe 更新走 GitHub Releases（manifest + 原子替换）；DSH 的已验证清单走 GitHub raw JSON。launcher 发版不卡 DSH 验证，反之亦然。
- 小白默认跟随「已验证」版本，不追 latest。
- 上一版本保留 = 回滚通道（一键回滚按钮 ✅ 2026-08-17，state.previous）。

**Windows PTY 回归的版本 pin（2026-08-17，临时策略，仅 npx 回退通道生效）**：官方 `0.1.0-rc.7` 把 node-pty 提升到
1.2.0-beta.15，Windows 上 persistent PTY shell 无法启动（`pid 0`，官方 discussion #2851；Linux/macOS
正常）。不 patch 上游、不 fork node-pty（红线），改为 **npx 回退通道 pin 版本**：`config.pin_dsh_version`
默认 `0.1.0-rc.6`（`#[serde(default)]`，置 null 手动关闭），`npx @deepseek-ai/dsh@<pin>` 启动。
系统 dsh 直启路径不锁版本（用户已装的版本由自己决定）。⚠️ v1 方案曾计划「启动后静默查 registry、
官方修复（> rc.7）自动解除 pin」，该 audit 逻辑随 version.rs 移除未实现——当前为**纯配置 pin**，
官方修复后手动把配置置 null 即可解除。

## 7. 对上游契约漂移的韧性

1. **接触面最小 + 契约文档化**（见 §4）
2. **鸭子类型而非类型绑定**：只依赖稳定表面（auditor 已验证 `ctx.inject` 思路）
3. **自更新与 DSH 版本解耦**（见 §6）
4. **PWA/SW 风险**（2026-08-14 查证）：官方已交付 manifest（`manifest.webmanifest` + e2e 钉死，`display: fullscreen`），
   **尚无 service worker**。若将来官方补 sw 离线缓存，会与「启动器切换 DSH 版本」冲突（陈旧 UI 缓存）。
   对策：健康探测不只看 HTTP 200，记录页面版本指纹（C2 契约演进），升级后若指纹变化则提示刷新/强制 reload。

## 7.5 PWA 定位（决策记录）

PWA 是「界面层」的事，不是「启动器层」的事，不改变本设计任何架构：

- **含义 A（启动器本身做成 PWA）**：❌ 架构矛盾——PWA 不能 spawn 子进程 / 托盘常驻 / 开机自启 / 崩溃自愈 / 本地版本管理，这些恰是 dsh-come 的存在理由。
- **含义 B（Web UI 安装成独立窗口）**：✅ 官方已铺路（localhost 是 secure context，manifest 在），我们**消费**它而不是造它。
- **含义 C（补 sw 离线缓存）**：❌ 对 localhost 服务无意义且有害（陈旧 UI 缓存）。

启动器侧行为：v2 打开界面用 `--app` 独立窗口（`msedge --app=http://127.0.0.1:PORT` / chrome 同理，无地址栏+任务栏图标），
首次向导可选提示「安装为桌面应用」（引导浏览器安装按钮，纯文案）。

## 7.6 工作台市场（v1 历史决策，2026-08-16；**2026-08-17 起仅保留「单件工具目录移交 dsh-market」结论**，壳内市场代码已随 plugins.rs 移除）

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

**2026-08-17 更新：单件工具目录交给 dsh-market，壳保留工作台差异化（v1 设计；壳内清单/菜单已随瘦身移除）**。用户拍板改用
[dsh-market](https://github.com/dsh-market/dsh-market)（DSH 可视化插件市场，Settings → Plugin Market）
作为插件目录：它是 DSH 生态内的一等公民（`dsh plugin add dshmarket` 装进 web profile），已具备
浏览/搜索 800+ 插件、主题、逐插件更新、备份恢复、诊断等能力，远超壳内自研清单的性价比；
其目录来自 awesome-dsh-plugin 策展注册表（`awesome-dsh-plugin.com/plugins.json`，每日 CI 刷新），
上架走该注册表 PR。**壳侧变化**：托盘「市场」改为「安装/打开插件市场」引导（未装一键 `dsh plugin add
dshmarket`）；内置/远程清单只保留工作台（kind=workbench——dsh-market 没有工作台分组概念，场景整包
入口仍是壳的差异化）；spawn dsh 时经 `--patch` 挂壳 patch overlay（`home\come.patch.yml`）写
`dsh-market.config.allowRestart: false`——dsh-market 的 detached 一键重启默认开启，会绕过壳的
supervisor（崩溃自愈/退避/日志），必须禁掉由壳接管重启。

## 8. MVP 范围（表格为 v1 完整版；**已实现并保留**仅 3 项：进程守护/健康探测/托盘，见下）

| 模块 | v1（已实现） | v2 | 2026-08-17 后 |
|---|---|---|---|
| 捆绑 Node 自举安装 + npx 通道 | ✅ | | ❌ 移除（跟随系统 dsh） |
| 固定端口 + HTTP 健康探测 | ✅ | | ✅ 保留 |
| 崩溃重启（退避+健康期重置）+ 滚动日志 | ✅ | | ✅ 保留 |
| 自愈诊疗（doctor：扫描取证→分级处置→急救兜底） | | | ✅ 新增（2026-08-18，`src/doctor.rs`） |
| registry 检查 + 冒烟验证 + 切换/回滚 | ✅ | | ❌ 移除 |
| 市场（插件市场 dsh-market + 工作台分组） | ✅（2026-08-17：插件市场引导 + 工作台保留；原内置单件工具清单移交 dsh-market） | 验证引擎增量产出 | ❌ 移除（归 dsh-market 插件） |
| 托盘（官方 logo 主题感知 / 悬停稳定 / 状态行分阶段） | ✅ | | ✅ 保留（简化为 5 项菜单 + 代码生成图标） |
| 自动开浏览器 + `--app` 独立窗口（桌面 App 化） | ✅ | | ⚠️ 仅保留自动开浏览器，`--app` 未实现 |
| 重启引擎 + 开机自启（HKCU Run） | ✅ | | ⚠️ 重启保留；开机自启未实现 |
| 全 waterfall 冒烟验证（verify 引擎收编） | | ✅ | ❌ |
| verified-plugins.json 远程清单 | ✅（2026-08-16：合并 + 缓存 + 回退） | 验证引擎增量产出 | ❌ 移除 |
| 首次向导（API key/工作区写进 home\） | ✅ 实现（2026-08-16，待收编提交） | | ⚠️ 简化为静默启动（wizard.rs 53 行） |
| launcher 自更新（GitHub Releases） | 手动替换 | ✅ | — |
| 壳管理页（/desktop 页面：版本/插件/日志/工作台） | ✅（2026-08-16） | | ❌ 移除 |

**2026-08-18 增补：自愈诊疗（doctor.rs）**。瘦身后壳「越做越薄」，但守护职责升级为**证据驱动自愈**：
不写死检查（孤儿 file:// 入口 / 损坏 patch / 残缺下载 / 端口占用 / 孤儿进程，均来自实际扫描），
影响半径分级（🟢绿自动 / 🟡黄主治+ / 🔴红仅急救且先备份），模式阶梯（巡检→处置→主治→急救）随
崩溃次数逐级升级，上限耗尽急救兜底。这是守护的**延伸而非新业务**——不违背「薄」，它替代的是
v1「反复崩溃只能手查日志」的空白。安全边界见 docs/cli-contract.md「自愈诊疗例外」。

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
| `md-agent\src\engine.rs` | **supervisor 完整参考**（318 行）：`cmd /C` 包装、日志重定向防阻塞、1s 轮询 + 限次自动重启、`taskkill /T /F` 杀进程树、退出清理钩子、start 幂等。差异：dsh-come 走 npx 通道（node 直启 npx-cli.js，免 cmd /C）+ 重启**退避**（md-agent 是固定 1s）+ HTTP 就绪探测 |
| `dsh-tray` | EBUSY / 双托盘 / iconPath ENOENT 踩坑；headless 降级 |
| `native/landlock-run` | cli-contract.md 模式；平台分包发布模式 |
| `dsh-plugin-verify` | 验证引擎（v2 收编为库 `@qing3a/dsh-runtime-verify`） |
