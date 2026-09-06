# Contributing to dsh-come

欢迎贡献！dsh-come 是开源的 DSH 桌面应用（Rust 桌面壳 + 自带 [Harness-LocalApp](https://github.com/qing3a/harness-localapp) 的 HLP 协议层与 LocalApp 生态）。

## 开发环境

- **Rust stable**（edition 2021）
- **Windows**：`cargo build --release`（tray-icon/winit）；Linux 需 `libgtk-3-dev libglib2.0-dev libxdo-dev`；macOS 用 launchd
- **项目结构**：
  - `src/`：main（单实例/启动顺序）、supervisor（spawn dsh web + 崩溃自愈）、runtime（目录/`come.patch.yml`/HLP 插件部署）、installer（node/dsh 安装）、wizard（首次引导）、doctor（自愈诊疗）、status（管理页 API）、hlp_plugin（HLP 插件检测/安装/修复）、config/updater/tray/notify/patchyml/job/uninstall
  - `resources/admin.html`：管理页（`include_str!` 内嵌）
  - `scripts/sync-hlp-plugin.mjs`：发版前同步 HLP 插件
- **本地运行**：`cargo run --release`（开发可用 `--no-tray`）

## 代码规范

- `cargo fmt` + `cargo clippy`（CI 检查；存量警告收敛中，暂不 `-D warnings`）
- 测试：`cargo test`（当前 59+ 断言）
- 管理页 HTML 是 release 内嵌（绝不读 exe 外文件——防本地提权）；改 admin.html 需重编译

## 关键设计约束（改前必读）

- **壳只碰「门把手」**：只依赖 dsh 启动命令/端口探测/进程管理，不解析 CLI 输出、不读内部文件
- **come.patch.yml 是内容感知维护**：新增插件条目必须用 `- insert:` 结构（裸条目=对已有插件配置覆盖）；HLP 插件部署进共享层 `@hlp/`（file:// 直指目录不可行——Node ESM + 依赖解析，见 `src/runtime.rs` 注释）
- **doctor 分级处置**：所有修改先备份 `.bak`；进程处置不碰当前运行的引擎
- **数据隔离**：不设 DSH_HOME（默认跟系统 dsh 走）；测试用 `DSH_COME_HOME` + `DSH_HOME` 指向临时目录隔离

## 提交流程

- **Conventional Commits**：`feat:` / `fix:` / `docs:` / `test:` / `ci:`
- 小步提交，单维护者可直推 master；PR 描述改动 + 测试

## 测试与 CI

```bash
cargo fmt --check
cargo clippy --release
cargo test --release
```

CI（`.github/workflows/ci.yml`）push/PR 自动跑 fmt/clippy/test/build；`hlp-plugin-check` job 校验随附插件源与 HLP 仓库一致（需 Secret `DEPLOY_SSH_KEY`，未配置自动跳过）。

## 发布流程

```bash
# 1. 同步最新 HLP 插件到随附插件源（铁律：发版前必须跑）
node scripts/sync-hlp-plugin.mjs
# 2. cargo build --release
# 3. 打 tag → GitHub Actions 自动发 Release（上传 exe + 资产）
```

Release 资产含 `dsh-come.exe` + `hlp-plugin.zip`（用户解压到 exe 同目录 `hlp-plugin\` 即得 LocalApp 生态）。

## 敏感信息纪律

真实邮箱 / SMTP 授权码 / Token / 私钥绝不进仓库（含 git 历史）；文档用示例地址。随附插件源由 sync 脚本排除 `data/`（真实业务数据不进安装包）。

## 许可证

MIT — 详见 [LICENSE](LICENSE)。
