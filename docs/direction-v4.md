# 方向 v4：商业化与升级方案（讨论定稿）

> 2026-08-27 定稿。本文记录商业化与功能升级方向的决策（四个拍板 + 统一架构 + 路线图），
> 是对 v3（`docs/slimming-plan.md`，越做越薄）的延续与收口，核心原则：
> **壳零 UI（管理页例外）**——dsh-come 不拥有业务插件和业务 UI；管理页因「预启动鸡生蛋 +
> 功能少」保留在壳内（2026-08-27 拍板，见 P2）。
>
> 本文生效后：
> - `docs/integration-plan.md` 的 Phase 2/3（md-agent 守护、三层集成）**作废**
> - MEMORY.md 待办中「supervisor 增加 md-agent 守护」**删除**
> - `docs/market.md` 的「零清单」方向**延续**（上架统一走 dsh-market）

> ⚠️ **2026-08-27 实施勘误**：本文基于 08-19 文档快照撰写，实施时发现工作区已演进
> （壳管理页已含插件清单/卸载/dsh 版本管理、uninstall.rs、Unix flock 跨平台苗头、托盘 7 项菜单）。
> 两个张力点：① **管理页保留 or 迁出 come-manager —— 已拍板：保留在壳内**（功能少 +
> 预启动鸡生蛋，迁出会变两个页面；未来管理 UI 膨胀或企业包需要时再评估插件形态）；
> ② **跨平台是否继续**（v4 原定「不盲目跨平台」，待拍板）。P0（自动更新 + GitHub Actions + i18n）
> 已于 2026-08-27 实施完成，见 MEMORY.md「P0 实施记录」。

## 1. 定位一句话

dsh-come 是 DSH 生态的 Windows 守护壳：让 dsh 双击即用、挂了自愈、多 profile 常驻。
商业化结论：**Open Core**——壳的三件事（守护 / 引导安装 / 环境清单）+ 自更新全部免费 MIT；
未来企业增强包（多机控制面、策略分发、webhook 告警、签名安装包）收费。

## 2. 四个拍板（2026-08-27 讨论确认）

| 议题 | 结论 | 直接影响 |
|---|---|---|
| md-agent 插件化边界 | **整体插件化**（数据层并入 dsh 进程） | 三层收敛为两层；dsh-come 不需要 md-agent 守护位 |
| 多实例范围 | **单机多 profile** | supervisor 从守护单实例重构为守护 profile 组 |
| 工作台定位 | **参考模板**（md-studio 为样板 + 配方文档） | 不做通用工作台产品化 |
| i18n 范围 | **代码面 + 文档**（壳内字符串外部化；插件 UI 各自负责） | P0 工作量可控 |

## 3. 最终架构（两层）

```
dsh-come（Rust 壳，三件事 + 自更新）
├── 守护      守护 profile 组：每个 profile = 一个 dsh 实例（端口 + patch 集），
│             托盘分组切换；崩溃自愈/认领/看门狗逻辑对每组复用
├── 引导安装   预启动向导（node/dsh 缺失时 winget/npm）——鸡生蛋阶段唯一留在壳里的 UI
├── 环境清单   come.patch.yml 从「写一条 dsh-market」扩展为编排 CRUD（装/卸插件 = 改条目）
└── 自更新    GitHub Releases + GitHub Actions 自动发布（见 P0）

dsh web（UI 层，插件为主；管理页例外，保留在壳内 3081）
├── 管理页       保留在壳内（2026-08-27 拍板不迁出，见 P2）
├── md-studio     工作台参考模板（工具 + 页面双通道同源，不扩展领域功能）
├── md-agent       整体插件化后并入（agent/kb/graph/memory 全在 dsh 进程内）
└── …业务插件（recruit-workbench 等）
```

三个收益：

1. **守护叙事完整**：md-agent 插件化后「守护 dsh 即守护一切」成立，壳的守护面更纯粹；
2. **壳保持薄**：市场清单已移出；管理页保留在壳内（小 + 预启动依赖，见 P2 决策），
   不因「壳零 UI」原则为一个小页面付两个页面的维护成本；
3. **商业化边界 = 架构边界**：免费层 = 壳的三件事；未来企业包恰好长在壳保留的能力上，不用为收费重新设计。

## 4. 路线图

### P0 —— 产品化地基

**1. 自动更新（轻量 updater，约 150 行）**

- 每个 release 附 `update.json`（`{version, url, sha256}`）；启动时静默检查、托盘提示、**询问制**（沿用历史习惯，不静默安装）；
- 下载后 SHA256 校验 → 旧 exe 存 `.bak`（校验失败即回滚）→ 替换；
- 实现要点：
  - Windows 不能覆盖运行中的 exe：下载为 `dsh-come.new.exe`，退出后用 cmd 等进程结束 → `move /y` → 重启；
  - 替换窗口期**临时禁用看门狗任务再恢复**（否则旧 exe 被拉起导致文件锁死）；
- **不买代码签名证书**：GitHub Releases + HTTPS + SHA256 对个人项目够用；代价仅是 SmartScreen 首次运行警告（文档写明「更多信息 → 仍要运行」）；未来企业包需要时用 Azure Trusted Signing（约 $10/月），届时是企业成本。

**2. GitHub Actions 自动发布流水线**

- 仓库新增 `.github/workflows/release.yml`：打 `v*` 标签自动执行：
  - `cargo test`（质量门）
  - `cargo build --release`（GitHub 的 windows-latest 机器）
  - PowerShell `Get-FileHash` 计算 sha256 → 生成 `update.json`
  - 创建 GitHub Release 并附上 `dsh-come.exe` + `update.json`
- 价值：发布不漏步骤、每版自动过测试、构建可复现（对无签名分发也是一种信任背书）；
- 附带清理：CI 接管产出后，`dist/` 不再需要提交进仓库（可选）；
- 工作流骨架（实现时细化）：

```yaml
name: release
on:
  push:
    tags: ['v*']
jobs:
  build:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --release
      - run: cargo build --release
      - run: |
          $h = (Get-FileHash target/release/dsh-come.exe -Algorithm SHA256).Hash.ToLower()
          '{"version":"${{ github.ref_name }}","url":"https://github.com/qing3a/dsh-come/releases/download/${{ github.ref_name }}/dsh-come.exe","sha256":"' + $h + '"}' | Out-File update.json -Encoding utf8
      - uses: softprops/action-gh-release@v2
        with:
          files: target/release/dsh-come.exe, update.json
```

**3. i18n**

- 托盘菜单 / 桌面通知 / 管理页 / CLI 输出（status/doctor 等）字符串集中到一张字符串表；
- `config.json` 增加 `lang: zh|en`（默认 zh）；
- README 双语；插件 UI 由插件项目自行处理，壳不背。

### P1 —— 多实例（单机多 profile）

1. **supervisor 重构**：单 dsh 实例 → profile 组。状态模型从「一个引擎」变为「一组引擎」；退避重启 / 认领 / 三段式探活 / doctor 协调对每个实例独立生效；
2. **托盘分组**：每个 profile 一组菜单（打开/重启/日志），状态行显示组摘要；
3. **管理页适配**：支持 profile 组状态（管理页保留在壳内，直接增强；不迁插件）；
4. **IPC 版本化**：state.json / control.json 加 `schema_version`；
5. **删除 md-agent 守护项**：MEMORY.md 待办清理；integration-plan Phase 2/3 标注作废。

### P2 —— 统一为「写插件」

1. **md-studio 定型为参考模板**：不扩展领域功能；补「写一个业务工作台」配方文档（基于 `docs/dsh-plugin-guide.md`）；
2. **管理页保留在壳内**（2026-08-27 拍板）：后端 JSON API（/api/status、启停、安装）本就归壳，
   迁出只搬前端渲染；且 dsh 未装时插件跑不起来（鸡生蛋）→ 迁出会变成「预启动小页 + 插件大页」
   两个页面，代码更多而非更少。**触发再评估的条件**：管理 UI 膨胀（如 P1 多实例仪表盘）或
   企业包需要独立 UI 时；
3. **上架零清单**：md-studio（及未来业务插件）走 dsh-market / awesome-dsh-plugin 注册表 PR；壳 README 列「官方插件」小节；
4. **环境清单落地**：come.patch.yml 从单条扩展为条目 CRUD（`dsh-come plugin add/remove` CLI 编辑 patch 文件）。

## 5. 明确不做（防回潮）

- 代码签名证书（个人不付费；企业包再议）
- md-agent 守护（整体插件化后不必要）
- 通用工作台产品化（md-studio 只是样板）
- 多机集中管理 / 企业控制面（后置，属未来企业包）
- 遥测（未拍板；保持 opt-in 候选，作为企业包论证素材）

## 6. 商业化边界与触发条件

- 免费层 = 壳的三件事 + 自更新（MIT 开源）；
- 企业增强包候选清单（未来）：多机集中管理、策略分发、webhook 告警、签名安装包、健康报告；
- 触发条件：**用户规模与留存数据**（埋点 opt-in 先行），不要在没数据时投入重运营；
- 与 DSH 生态的分工：DSH 免费获客（模型 API 收费）、壳免费获客（守护服务收费）——不与上游抢免费心智。

## 7. DSH 生态对齐（要点备忘）

- DSH = DeepSeek 的 Everything-is-a-Plugin agent harness（MIT，v0.1.1-rc）；官方已注册「DeepSeek Harness（工具框架）」公众号（北京深度求索主体）→ 官方在认真运营工具生态；
- 对齐方式：
  - patch overlay（come.patch.yml）是编排枢纽 → 升级为「环境编排文件」；
  - client plugin 是业务 UI 形态（md-studio 示范）；壳管理页例外保留（见 P2）；
  - `/api/health` 双向可观测（已做）→ 扩展为壳暴露自身状态（守护状态、重启历史、doctor 报告）；
  - 兼容性矩阵：跟随 dsh release 节奏做契约冒烟验证（cli-contract C1–C5 自动化）；
- 风险预案：若 DSH 官方推出桌面壳，护城河 = 守护深度（doctor 自愈 / Job Object / 看门狗）+ 多实例编排 + 企业包——官方大概率不会为免费用户做这些。
