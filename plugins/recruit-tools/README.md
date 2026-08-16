# dsh-recruit-tools｜猎头工作台工具插件（骨架）

DeepSeek Harness 官方插件模式实现的猎头工作台工具集（第一个骨架），领域模型参考
md-agent 的 headhunter 模板：候选人 / 职位 / 推荐流水线，本地优先落盘。

## 猎头工作台界面（Web）

插件通过 dsh 的 `webServer` 服务注册 HTTP 路由，直接挂在 dsh web 进程上（与 AI 对话**共用同一份 store.json**）：

- **`GET /recruit`** —— 工作台页面：候选人 / 职位 / 推荐流水线（7 态看板，可推进/拒绝/撤回）
- **`GET|POST /recruit/api/*`** —— JSON 接口（状态 / candidates / positions / referrals，与 AI 工具同一套校验与写入逻辑）

在对话里登记的数据会立刻出现在工作台，反之亦然。示例（本机演示实例）：http://127.0.0.1:3198/recruit

## 能力（8 个工具 + Web 工作台）

| 工具 | 作用 |
|---|---|
| recruit_register_candidate | 登记/更新候选人（隐私：只记必要事实，默认保密） |
| recruit_list_candidates | 候选人台账（按阶段/关键词过滤） |
| recruit_register_position | 登记/更新职位需求（薪资默认保密） |
| recruit_list_positions | 职位列表（按客户/关键词过滤） |
| recruit_create_referral | 创建推荐（校验候选人/职位存在） |
| recruit_update_referral_stage | 推进推荐 7 态流水线 |
| recruit_list_referrals | 推荐流水线（带候选人/职位信息） |

数据落 $DSH_HOME/recruit/store.json（可用 dataDir 配置覆盖），JSON 明文、原子写，
本地优先、可审计；推荐 7 态对齐 md-agent：已推荐 → 待客户反馈 → 面试中 → 已发Offer → 已入职（推进链），终态 拒绝 / 撤回 可直达。

## 加载

```powershell
# 开发加载（本机验证过；dsh 来自 npx 缓存，--patch 后接本文件绝对路径）
dsh web --patch C:/Users/Administrator/Desktop/dsh-come/plugins/recruit-tools/cordis.yml --host 127.0.0.1 --port 3188
```

启动日志出现 `[recruit-tools] plugin loaded!` 即加载成功；之后在 Web UI 问
「把张三登记为候选人」即可调用。每个工具带 `presentCall`/`presentResult` 富卡片
（对话里渲染成结构化卡片而非裸文本）。

## 验证记录（2026-08-14）

- **真实对话 E2E（headless + 真实 DeepSeek 模型）✅**：隔离 DSH_HOME + 复用 GUI 模型配置，
  任务「登记候选人→登记职位→创建推荐→推进到面试中→列出推荐」全部由模型调用 `recruit_*`
  工具完成；推荐按新状态机**分两步推进**（已推荐→待客户反馈→面试中，未跳级）；数据落盘
  `$DSH_HOME/recruit/store.json` 核验正确。
- **加载冒烟 ✅**：`dsh web --patch <cordis.yml>` 隔离 home + 临时端口，日志
  `[recruit-tools] plugin loaded!` + HTTP 200（含 UI 增强版本）。
- **踩坑记录**：见 `docs/dsh-plugin-guide.md` §11（file:// URL、Duplicate export、junction、schema 布尔 additionalProperties）。

## 本地开发解析（重要）

插件文件在 dsh-come 仓库内，而 `@deepseek-ai/*` 依赖装在 dsh 的 node_modules 里。
开发时需让插件能解析到**同一份**依赖实例（避免 Cordis 双实例）：

```powershell
# 在本目录建 junction 指向 npx 缓存的 node_modules（已 gitignore，勿提交）
New-Item -ItemType Junction -Path .\node_modules -Target "C:\Users\Administrator\AppData\Local\npm-cache\_npx\1e7f6d9597241db0\node_modules"
```

正式发布前改为依赖安装（pnpm add）+ tsc 构建到 lib/，格式对齐官方
@deepseek-ai/dsh-tool-todo（见 docs/dsh-plugin-guide.md §9）。

## 待办（下一步）

- 与 md-agent 推荐状态机逐项对齐（确认 7 态命名）
- UI 插件：工作台视图（参考 ow-recruit 21 屏三端的界面资产）
- 人审/合规模式：写入先落待审区
- 远程清单进入壳的「插件市场」（✓已验证）