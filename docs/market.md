# 市场策略（market）— dsh-market 协作 + 工作台收录

> 2026-08-17 定案：**单件工具目录交给 [dsh-market](https://github.com/dsh-market/dsh-market)
> （DSH 可视化插件市场）**，壳内置清单只保留**工作台**（kind=workbench，场景整包）。
> 市场是「清单 + 打开/装/卸」，不是商店（无评分/账号/支付，见 DESIGN.md 非目标）。

## 1. 分工

| 层 | 干什么 | 入口 |
|---|---|---|
| **dsh-market**（DSH 生态插件） | 浏览/搜索/安装 800+ 社区插件、主题、逐插件更新、备份恢复、诊断 | dsh web 的 Settings → Plugin Market |
| **壳（dsh-come）** | 引导安装 dsh-market（托盘「安装/打开插件市场」一键 `dsh plugin add dshmarket`）+ **工作台分组**（场景完整业务包，打开本地资产/URL 入口） | 托盘「市场」子菜单 + 壳管理页「工作台」卡片 |

工作台是 dsh-market 没有的商品形态（「工具 + 会话 UI + 业务规则」整包，需要场景分组与打开
语义），保留在壳内是差异化。dsh-market 安装后，其 detached 一键重启会被壳的 patch overlay
（`home\come.patch.yml`，`dsh-market.config.allowRestart: false`）禁用——重启归壳 supervisor 管
（崩溃自愈/退避/日志），防止绕过守护。

## 2. 工作台清单（PluginInfo）

| 字段 | 说明 |
|---|---|
| `id` | 工作台资产标识（如 `md-hr`） |
| `name` / `version` / `desc` / `repo` | 展示信息 |
| `verified` / `verify_evidence` | 验证通过标记 + 证据（e2e 通过数/验证报告链接） |
| `kind` | `workbench`（壳只收录工作台形态；远程清单若含 tool 条目仍兼容展示） |
| `scenario` | 工作台场景名（如「猎头协作」），市场第一层分组 |
| `entry` | 打开入口：`file://` 本地路径或 `http(s)` URL |
| `requires` | 依赖的外部服务（如 md-api 协作服务器），需用户自行启动 |

字段全部向后兼容：旧清单（无 `kind/scenario/entry`）反序列化按工具处理。

## 3. 收录标准（工作台）

1. **场景完整**：是"装完即用"的业务包（UI + 数据模型 + 业务规则），不是单件工具。
2. **验证有据**：`verify_evidence` 指向可复现的验证（e2e 全绿 / dsh-plugin-verify 报告）。
3. **形态不限**：工作台可以是 dsh 插件（client 插件，entry 留空），也可以是**本地资产**（单文件
   web 应用 + 外部服务，entry 指向入口）。是否升级为 dsh 插件是**商品项目自己的事**。
4. **资产归位**：本地资产工作台的项目文件放它自己的仓库/目录；本仓库清单只写引用路径，不拷贝资产。

## 4. 上架流程

1. 商品项目完成验证（e2e 等），产出一条 `PluginInfo`。
2. **两处同步登记**（改 dsh-come 仓库）：
   - `verified-plugins.json`（远程清单，GitHub raw，壳启动后拉取合并）
   - `src/plugins.rs::builtin_marketplace()`（内置兜底清单，离线可用）
3. 壳编译发版后：托盘「市场」出现工作台分组，条目 [打开] 直接进入口；
   壳管理页「工作台」卡片显示就绪状态与依赖提示。

> 若工作台同时做成 dsh 插件要进社区目录，走 awesome-dsh-plugin 注册表 PR
> （dsh-market 的目录来源，上架后自动进市场），与本流程互不影响。

## 5. 当前工作台

| id | 名称 | 场景 | 形态 | entry |
|---|---|---|---|---|
| `md-hr` | 猎头协作 | 猎头协作 | 本地资产（Desktop/md-hr）+ md-api MCP | `file:///.../md-hr/index.html` |

md-hr 是外部商品（`Desktop/md-hr`），其代码、构建、dsh 插件化升级均由该项目的用户负责；
dsh-come 只在本仓库清单里收录它的入口与验证信息。
