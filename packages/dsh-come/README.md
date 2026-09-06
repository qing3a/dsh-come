# dsh-come — npm 安装器

通过 npm 安装 DSH 桌面壳（`dsh-come`）：**二进制经 npm registry 平台分包分发，安装只做本地复制，无 GitHub 直连依赖**（国内镜像友好）。

## 安装

```bash
npm install -g dsh-come        # postinstall 自动就位二进制
dsh-come status                # 查看安装状态
dsh-come                       # 启动桌面壳（托盘常驻，等价于直接运行二进制）
```

- **免安装直用**：`npx dsh-come install`（npx 临时环境，适合不想全局装）
- **升级**：`npm install -g dsh-come@latest`（npm 升级 = 壳升级，版本标记随二进制写入）
- **卸载**：`npm uninstall -g dsh-come`（preuninstall 自动清理二进制，配置与数据保留）

## 命令

| 命令 | 说明 |
| --- | --- |
| `dsh-come [参数...]` | 启动壳并透传全部参数（如 `dsh-come web`） |
| `dsh-come install [--force] [--source registry\|github]` | 安装/强制重装；`--source github` 强制从 GitHub Releases 拉取 |
| `dsh-come update` | 更新到已发布的最新版 |
| `dsh-come status` | 平台/安装路径/本地版本/最新版本 |
| `dsh-come uninstall` | 删除二进制（保留配置与数据） |

## 二进制来源（两级）

1. **npm registry 平台包**（默认）：`optionalDependencies` 按平台自动安装
   - `dsh-come-win32-x64`（Windows x64）
   - `dsh-come-darwin`（macOS universal，x64+arm64）
   - `dsh-come-linux-x64`（Linux x64）
   - postinstall 从已安装的平台包复制二进制到安装目录，**无网络请求**。
2. **GitHub Releases**（fallback）：平台包未随 npm 装上时自动回退（`releases/latest/download/update-{win|macos|linux}.json` + sha256 校验）。

## 安装位置

| 平台 | 路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\dsh-come\dsh-come.exe` |
| macOS / Linux | `~/.local/bin/dsh-come`（与 `scripts/install.sh` 一致） |

## 发布（维护者）

打 `v*` 标签后 CI（`.github/workflows/release.yml` 的 `npm-publish` job）自动：
1. 从 release 资产下载三平台二进制 → 放入对应平台包的 `bin/`
2. 四个包 version 对齐 tag（`npm version` + 同步 optionalDependencies 版本）
3. 依次 publish：三个平台包 → 主包 `dsh-come`

需要仓库 Secret：`NPM_TOKEN`（npmjs 账号，且为 `dsh-come*` 包名的 owner）。

手动发布（本地）：

```bash
# 1) 平台包二进制就位（从 Releases 下载或本地构建复制）
cp target/release/dsh-come.exe packages/dsh-come-win32-x64/bin/
# macOS/Linux 同理（universal 切片见 release.yml）

# 2) 版本对齐（tag v0.2.0 → 0.2.0）
VER=0.2.0
for d in packages/dsh-come packages/dsh-come-win32-x64 packages/dsh-come-darwin packages/dsh-come-linux-x64; do
  (cd "$d" && npm version "$VER" --no-git-tag-version --allow-same-version)
done

# 3) 发布（平台包先、主包最后；本机 registry 若为镜像需显式指定官方源）
(cd packages/dsh-come-win32-x64 && npm publish --access public --registry https://registry.npmjs.org/)
(cd packages/dsh-come-darwin && npm publish --access public --registry https://registry.npmjs.org/)
(cd packages/dsh-come-linux-x64 && npm publish --access public --registry https://registry.npmjs.org/)
(cd packages/dsh-come && npm publish --access public --registry https://registry.npmjs.org/)
```

## 开发与测试

```bash
cd packages/dsh-come
npm test                 # node --test（平台映射 / 版本比较 / 平台包映射）
```

端到端本地验证：把平台包 tgz 解压到主包旁 `node_modules/<平台包名>/`，再执行
`node lib/cli.js install` 与 `node lib/cli.js status`。
