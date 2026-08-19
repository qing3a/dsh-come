# dsh-come 当前架构（2026-08-19）

> 本文档是**当前实现**的唯一权威描述。`DESIGN.md` 主体是 v1 历史设计（捆绑 Node/版本管理/市场/
> 状态页/向导页，均已移除），`docs/integration-plan.md` 部分条目已标注不可落地——追溯动机时查阅
> 旧文档，判断行为以本文 + `docs/cli-contract.md` 为准。

## 定位

进程外 supervisor：守护 DeepSeek Harness（dsh）的 Windows 托盘壳。**越做越薄，只碰门把手**
（启动命令 / 端口探测 / 进程管理）。与 dsh-tray（进程内插件）互补不冗余。

## 模块（10 文件）

| 文件 | 职责 |
|------|------|
| main.rs | 入口：子命令（status/stop/config edit/doctor）+ 单实例锁 + Job Object + 管理页 + 无头/托盘降级 |
| supervisor.rs | spawn dsh web + 指数退避重启 + 三层健康探测 + 崩溃逐级诊疗 + state/control 文件 IPC + 滚动日志 |
| runtime.rs | 命令构建（系统 dsh 直启，**无 npx 回退**）+ 路径/运行器定位 + state.json/control.json 路径 |
| installer.rs | 环境探测（node/npm/dsh/winget，合并进程+注册表+npmpfx PATH）+ 异步安装（winget/npm）+ 安装状态 |
| doctor.rs | 证据驱动自愈诊疗（扫描取证→分级→按模式处置→兜底升级） |
| tray.rs | 系统托盘：状态行 + 菜单（打开界面置顶/重启/关闭/日志/退出）；3s 定时重建 |
| job.rs | Windows Job Object（KILL_ON_JOB_CLOSE 整树受控 + terminate_job 替代 taskkill /T） |
| notify.rs | 桌面通知（notify-rust；崩溃重启/达上限/接管/引导；失败静默） |
| status.rs | 管理页（std 零依赖 HTTP：状态展示 + 安装 Node/dsh + 启动/关闭 + 轮询） |
| wizard.rs | 首次引导：缺失时自动正常安装（node→winget / dsh→npm）→ 装完重试启动 |

## 管理页与安装（2026-08-19 新增）

`http://127.0.0.1:<status_port>`（config.status_port，默认 3081，0=关闭）：

- `GET /` → 内嵌 HTML 管理页：dsh 引擎状态卡片 + 环境探测卡片 + 安装状态 + 按钮
  （启动 dsh / 关闭 dsh / 安装 Node.js / 安装 dsh），JS 每 2s 轮询
- `GET /api/status` → `{ eng: 守护状态, env: node/npm/dsh/winget 探测, install: 安装状态 }`
- `POST /api/install/node` → winget 静默装 Node.js LTS（会弹一次 UAC）
- `POST /api/install/dsh` → `npm install -g @deepseek-ai/dsh`（用户级，无 UAC）
- `POST /api/start` / `POST /api/stop` → 启停 dsh
- `GET /api/install/status` → 安装任务状态（running/ok/msg）

**安装原则（2026-08-19 用户拍板）**：不走 npx 临时拉取——dsh 缺失就正常安装。启动引导
（wizard）检测到缺失时**自动安装**：node 缺失 → winget（480s 超时，UAC 取消/无 winget 则提示
管理页手动下载 nodejs.org）；dsh 缺失 → npm install -g（装完经 npmpfx 目录立即可找到）。
失败路径：日志 + 托盘提示 + 管理页可查原因重试，attempt 上限兜底防死循环。

## 菜单（当前实际形态）

```
打开界面                    ← 置顶（最常用）
DSH 伴侣｜0.1.0-rc.6｜运行中 ✓ http://127.0.0.1:3080
──────────
重启引擎
关闭引擎                   ← 不区分是否本壳启动，运行中即可点（真正关闭）
打开日志目录
──────────
退出
```

- 菜单**每 3 秒重建**（状态行/菜单可用性实时刷新；2026-08-19 由 15s 改）
- 「关闭引擎」= `supervisor::stop()`：`auto_restart=false` 不自动拉起，看门狗继续后台；要用时点「重启引擎」
- 崩溃复活归 `scripts/install-watchdog.ps1` 计划任务（登录启动 + 每分钟检查）；登录自启未提供
  （2026-08-19 用户移除 HKCU Run 开关方案）

## 状态与跨进程控制

- `root_dir/state.json`：监测线程每轮（1s）写入的状态快照；`dsh-come status` 读取输出 JSON
- `root_dir/control.json`：`dsh-come stop` 写入的停止请求，监测线程下一轮消费（消费后删除）
- **状态 HTTP 端点**：`http://127.0.0.1:<status_port>`（config.status_port，默认 3081，0=关闭）
  返回实时状态 JSON（`supervisor::status_json`，内存直读不落盘）
- 单实例：named mutex `Local\dsh-come-single-instance`；控制命令（status/stop/config edit）在
  守护运行时通过文件 IPC 执行，其余双开静默退出

## 关键行为语义（2026-08-19 定案）

- **认领（adopt）**：端口已有健康 dsh → 接管（owned=false），monitor 每 5s 探活、连续 3 次判死自动
  接管重启。**但 stop/重启/退出不再区分 owned**——`kill_child` 统一真正关闭（含外部认领实例，
  用户 2026-08-19 拍板，推翻了 08-18「不杀外部进程」的部分语义）
- **探活优先级**：`health_ok` 优先 `/api/health`（需 dsh 侧 cordis 插件暴露，见
  `resources/dsh-health-plugin.js` 参考，需手动启用），缺失降级 `/`
- **崩溃自愈**：指数退避（1,2,4…封顶）+ 健康期清零预算 + 每次重启前逐级升级诊疗（处置→主治→急救）
  + 上限耗尽急救兜底；monitor 锁中毒恢复 + heal/start 包 catch_unwind（守护本身有守护）
- **稳定性**：Job Object（KILL_ON_JOB_CLOSE 崩溃整树强杀）+ 看门狗计划任务
  （`scripts/install-watchdog.ps1`：登录启动 + 每分钟检查复活）

## 配置（root_dir/config.json）

| 字段 | 默认 | 说明 |
|------|------|------|
| port | 3080 | dsh web 端口 |
| host | 127.0.0.1 | 监听地址 |
| max_restarts | 5 | 崩溃重启上限（0=不自动重启） |
| backoff_max_secs | 30 | 退避封顶（秒） |
| startup_timeout_secs | 240 | 就绪等待上限（首次安装/下载 dsh 可能慢） |
| doctor_mode | null | 首次启动自检模式覆盖 |
| status_port | 3081 | 管理页端口（0=关闭） |

## CLI

```
dsh-come                双击启动（托盘模式；--no-tray 无头）
dsh-come status         查询运行状态（JSON）
dsh-come stop           停止 dsh 引擎（看门狗继续后台）
dsh-come config edit    打开配置文件
dsh-come doctor [--mode inspect|treat|attend|emergency]  独立自愈诊疗
dsh-come --port <端口>  --no-tray
```

## 契约（docs/cli-contract.md v2，仅依赖稳定表面）

C1 `dsh web --host <host> --port <port>`（PATH 直启；**无 npx 回退**——缺失走安装流程）｜
C2 HTTP 200 就绪探测（/api/health 优先）｜ C3 `dsh --patch <path>`（come.patch.yml overlay）｜
C4/C5 预留（冒烟验证 / 插件管理）
