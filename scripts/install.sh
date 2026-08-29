#!/bin/sh
# dsh-come 一键安装（macOS / Linux）：从 GitHub Releases 下载当前平台最新版 →
# sha256 校验 → 安装到 ~/.local/bin → 注册看门狗（launchd / systemd）。
#
# 用法（三选一）:
#   curl -fsSL https://github.com/qing3a/dsh-come/releases/latest/download/install.sh | sh
#   wget -qO- https://github.com/qing3a/dsh-come/releases/latest/download/install.sh | sh
#   sh scripts/install.sh            # 仓库内直接跑（同效果）
#
# 卸载: scripts/install-watchdog.sh remove && rm -f ~/.local/bin/dsh-come
#
# 发布资产与更新清单按平台命名（见 .github/workflows/release.yml 矩阵构建）:
#   dsh-come-macos（universal）/ dsh-come-linux（x64）/ dsh-come.exe（Windows）
#   update-macos.json / update-linux.json / update-win.json
set -u

# ---------- 平台判定 ----------
os=$(uname -s)
case "$os" in
    Darwin)
        suffix=macos
        asset=dsh-come-macos
        hash_cmd="shasum -a 256"
        ;;
    Linux)
        suffix=linux
        asset=dsh-come-linux
        hash_cmd="sha256sum"
        ;;
    *)
        echo "不支持的系统: $os（仅支持 macOS / Linux；Windows 直接下载 dsh-come.exe）" >&2
        exit 1
        ;;
esac

base="https://github.com/qing3a/dsh-come/releases/latest/download"

echo "==> 获取 $suffix 更新清单"
manifest=$(curl -fsSL "$base/update-$suffix.json") || {
    echo "获取更新清单失败（网络或该平台暂无发布资产）: $base/update-$suffix.json" >&2
    exit 1
}
url=$(printf '%s' "$manifest" | sed -n 's/.*"url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
sha=$(printf '%s' "$manifest" | sed -n 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -n "$url" ] && [ -n "$sha" ] || {
    echo "更新清单解析失败: $manifest" >&2
    exit 1
}

tmp="$HOME/.dsh-come-install.$$"
mkdir -p "$tmp"
trap 'rm -rf "$tmp"' EXIT

echo "==> 下载 $asset"
curl -fsSL -o "$tmp/$asset" "$url" || {
    echo "下载失败: $url" >&2
    exit 1
}

echo "==> 校验 sha256"
got=$($hash_cmd "$tmp/$asset" | awk '{print $1}')
if [ "$got" != "$sha" ]; then
    echo "校验失败: 期望 $sha" >&2
    echo "          实际 $got" >&2
    exit 1
fi

echo "==> 安装到 ~/.local/bin"
mkdir -p "$HOME/.local/bin"
install -m 0755 "$tmp/$asset" "$HOME/.local/bin/dsh-come" || {
    echo "安装失败（~/.local/bin 不可写？）" >&2
    exit 1
}

echo "==> 注册看门狗（launchd / systemd）"
watchdog="$tmp/install-watchdog.sh"
curl -fsSL -o "$watchdog" "$base/install-watchdog.sh" 2>/dev/null || true
if [ -s "$watchdog" ]; then
    sh "$watchdog" install
else
    echo "  看门狗脚本下载失败——二进制已装好，可稍后手动执行 install-watchdog.sh install" >&2
fi

echo "==> 完成。验证状态:"
"$HOME/.local/bin/dsh-come" status || true
