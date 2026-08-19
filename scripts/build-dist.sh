#!/usr/bin/env bash
# 打包脚本（最小便携版）：编译 release 并把单 exe 放到 dist/。
#
# 用法: bash scripts/build-dist.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 编译 release（零告警为佳）"
cargo build --release

mkdir -p dist
cp target/release/dsh-come.exe dist/
echo "==> 产物: dist/dsh-come.exe"
ls -lh dist/dsh-come.exe
echo "==> 完成。产物为托盘常驻壳：双击运行即启动系统 dsh 引擎（需系统已装 dsh 或 Node/npx）。"
