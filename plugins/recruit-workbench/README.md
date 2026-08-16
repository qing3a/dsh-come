# dsh-recruit-workbench｜猎头工作台插件（完整版）

DeepSeek Harness 官方插件模式实现的猎头工作台。领域模型继承 [md-agent headhunter 模板]
（`C:\Users\Administrator\Desktop\md-agent\src\templates\projects\headhunter\`，MIT 开源），
界面资产参考 `md-agent\kb\apps\ow-recruit`（21 屏三端）。

## 能力总览

| 面 | 内容 |
|---|---|
| 工具面 | `recruitwb_*` 共 **18 个工具**：客户公司 / 候选人 / 职位 / 推荐 7 态 / 沟通留痕 / 面试 / Offer / 删除(需确认) / 仪表盘 / 跨实体检索 |
| 浏览器面 | 会话「工作台」视图标签（对话/轨迹/工作台三 tab）：仪表盘 KPI + 漏斗、候选人管理、职位管理、推荐看板（7 列）、沟通留痕时间线、详情弹窗 |
| 数据 | 本地 JSON：`$DSH_HOME/recruit-workbench/store.json`（原子写、明文、可审计），可用 `dataDir` 配置覆盖 |
| 审计 | 每次写操作落审计（最近 500 条），`GET /api/recruit-workbench/audit` 可读 |

### 推荐 7 态状态机（对齐 md-agent）

```
recommended 已推荐 → pending_client 待客户反馈 → interviewing 面试中 → offer_sent 已发Offer → hired 已入职
rejected 已拒绝 / withdrawn 已撤回 —— 终态，任意阶段可直达
```

只允许推进链下一步或直达终态，不可跳级（工具与 API 双侧校验）。

### 业务规则（继承模板 RULES.md）

1. 职位与候选人事实基于输入，不编造；
2. 客户/候选人严格隔离，绝不串用；
3. 候选人隐私：只记必要事实，内容只存本机；
4. 薪资/Offer/联系方式默认 confidential=true（保密）；
5. 重要信息先确认再落盘；删除需显式 `confirm=true`。

## 安装（浏览器 UI 需要此步）

clientModules 只扫描 profile node_modules 里可解析的包，因此工作台 UI 必须安装进 profile：

```powershell
# 1. 安装插件包（symlink 到仓库目录，改代码即时生效）
dsh plugin --profile web add C:/Users/Administrator/Desktop/dsh-desktop/plugins/recruit-workbench

# 2. 在 profile 补丁加行（已加好）：
#    C:\Users\Administrator\.dsh\profiles\web\cordis.patch.yml
#    - insert:
#        - id: recruit-workbench
#          name: 'recruit-workbench'
```

3. **重启 GUI**（`dsh web`）生效。重启后会话视图环出现「工作台」标签；
   插件行含 `dsh.client` 声明 → clientModules 以 `/plugins/recruit-workbench/client.js` 提供 UI bundle。

## 开发 / 冒烟（不动线上 GUI）

```powershell
# host 半快速验证：--dump-config 组合正确
dsh web --patch C:/Users/Administrator/Desktop/dsh-desktop/plugins/recruit-workbench/cordis.yml --dump-config

# 隔离 DSH_HOME + 临时端口真实加载（host 工具 + API 路由）
$env:DSH_HOME = '<临时目录>\home'
dsh web --patch C:/Users/Administrator/Desktop/dsh-desktop/plugins/recruit-workbench/cordis.yml --host 127.0.0.1 --port 3199
```

启动日志出现 `[recruit-workbench] plugin loaded!` 即成功；之后可验证：

```powershell
# API 读写（与工具共用同一业务逻辑与审计）
curl http://127.0.0.1:3199/api/recruit-workbench/state
curl -X POST http://127.0.0.1:3199/api/recruit-workbench/mutate -H "content-type: application/json" -d '{"op":"register_candidate","args":{"name":"张三","title":"前端工程师"}}'
```

## 目录结构

```
plugins/recruit-workbench/
├── src/index.ts      # HOST 半：领域模型 + 存储 + 18 工具 + Web API + 审计（Node 24 原生 TS，无需编译）
├── lib/client.js     # CLIENT 半：手写 ModuleLoader 格式 bundle（无构建步骤），React.createElement UI
├── package.json      # dsh.client 声明 + exports["./client"]（clientModules 发现入口）
├── tsconfig.json     # host 类型检查（noEmit）
├── cordis.yml        # 开发 overlay（host 半；UI 需走安装）
└── README.md
```

## 说明与边界

- **无构建工具依赖**：host 走 Node 24 原生 TS 类型剥离；client 是手写 loader-format JS（`window.__ModuleLoader__.load`）。
- **数据模型**：companies / candidates / positions / referrals / activities / interviews / offers / audit 八个集合。
- **API 无鉴权**：仅监听本机回环（与 Web UI 同信任域），数据本地优先。
- 待办：合规模式（写前人工审核）、候选人门户端、与 md-agent 站内信互通。
