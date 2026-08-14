#!/usr/bin/env bash
# 打包脚本（最小便携版）：编译 release 并把单 exe 放到 dist/。
# 「下载即得」闭环：exe 首次运行会经 ensure_node() 自动下载 portable Node（约 30MB），
# 因此分发物只需这一个 exe，无需预捆绑 Node。
#
# 用法: bash scripts/build-dist.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 编译 release（零告警为佳）"
cargo build --release

mkdir -p dist
cp target/release/dsh-desktop.exe dist/
echo "==> 产物: dist/dsh-desktop.exe"
ls -lh dist/dsh-desktop.exe
echo "==> 完成。首次运行会自动下载 Node 并安装 DSH。"
