# dsh-come-darwin

dsh-come macOS universal（x64 + arm64）桌面壳二进制（平台分发包）。

**请勿直接安装**——由主包 `dsh-come` 通过 `optionalDependencies` 自动安装。
`bin/dsh-come-macos` 由 CI（release.yml 的 npm-publish job）在发版时从 GitHub Release 资产填充。
