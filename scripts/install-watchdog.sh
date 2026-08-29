#!/bin/sh
# 注册/卸载 dsh-come 看门狗（Unix 对应物；Windows 见 scripts/install-watchdog.ps1）。
#
# dsh-come 是「进程外 supervisor」——它守护 dsh，但 dsh-come 自身此前没有系统级守护：
# 一旦崩溃/被强杀，dsh 会变孤儿继续占端口，下次启动只能「认领」、不再有崩溃自愈。
# 本脚本给 dsh-come 自己一个守护（与 Windows 计划任务「DSH伴侣守护」同语义）：
#   - macOS: launchd LaunchAgent（KeepAlive —— 进程退出即自动重启）
#   - Linux: systemd user unit（Restart=always + 60s 退避；--no-tray 一等形态，审计 P2-5）
#
# 用法:
#   scripts/install-watchdog.sh install   # 注册并启动（默认）
#   scripts/install-watchdog.sh remove    # 停止并移除
#
# 注意: 与 Windows 版同语义——主动退出也会被拉起（常驻伴侣预期行为）；
#       remove 后不再自动复活。
set -u

action="${1:-install}"

# ---------- 定位可执行文件 ----------
find_exe() {
    # 仓库内开发产物优先，其次已安装位置（install.sh 安装到 ~/.local/bin）
    repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
    for cand in \
        "$repo_root/target/release/dsh-come" \
        "$repo_root/dist/dsh-come" \
        "$repo_root/target/debug/dsh-come" \
        "$HOME/.local/bin/dsh-come" \
        "/usr/local/bin/dsh-come"; do
        if [ -x "$cand" ]; then
            exe=$(CDPATH= cd -- "$(dirname -- "$cand")" && pwd)/$(basename -- "$cand")
            return 0
        fi
    done
    echo "找不到 dsh-come 可执行文件（先 cargo build --release，或 scripts/install.sh 安装）" >&2
    exit 1
}

# ---------- macOS: launchd LaunchAgent ----------
install_macos() {
    find_exe
    plist_dir="$HOME/Library/LaunchAgents"
    plist="$plist_dir/com.qing3a.dsh-come.plist"
    mkdir -p "$plist_dir"
    cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.qing3a.dsh-come</string>
    <key>ProgramArguments</key>
    <array>
        <string>$exe</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
EOF
    # 先卸旧实例（未注册时失败无碍），再加载；Catalina 前无 bootstrap 回落 load -w
    launchctl bootout "gui/$(id -u)/com.qing3a.dsh-come" 2>/dev/null || true
    if ! launchctl bootstrap "gui/$(id -u)" "$plist" 2>/dev/null; then
        launchctl load -w "$plist" 2>/dev/null || {
            echo "launchctl 加载失败（请确认 macOS 版本）" >&2
            exit 1
        }
    fi
    echo "✅ 已注册 launchd 看门狗: $plist"
    echo "   行为: 登录自动启动 + KeepAlive（进程退出即重启）"
    echo "   卸载: scripts/install-watchdog.sh remove"
}

remove_macos() {
    plist="$HOME/Library/LaunchAgents/com.qing3a.dsh-come.plist"
    launchctl bootout "gui/$(id -u)/com.qing3a.dsh-come" 2>/dev/null \
        || launchctl unload -w "$plist" 2>/dev/null || true
    rm -f "$plist"
    echo "✅ 已移除 launchd 看门狗"
}

# ---------- Linux: systemd user unit ----------
install_linux() {
    find_exe
    if ! command -v systemctl >/dev/null 2>&1; then
        echo "当前环境无 systemd（容器/非 systemd 发行版），看门狗未注册。" >&2
        echo "可改用托盘 / --no-tray 常驻；无看门狗时崩溃不会自动复活。" >&2
        exit 1
    fi
    unit_dir="$HOME/.config/systemd/user"
    unit="$unit_dir/dsh-come.service"
    mkdir -p "$unit_dir"
    cat > "$unit" <<EOF
[Unit]
Description=DSH Companion (dsh-come)
Documentation=https://github.com/qing3a/dsh-come
After=network-online.target

[Service]
Type=simple
ExecStart=$exe --no-tray
Restart=always
RestartSec=60

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    if ! systemctl --user enable --now dsh-come.service 2>/dev/null; then
        echo "systemctl --user 启用失败。" >&2
        echo "常见原因: 当前会话无用户 systemd 总线（XDG_RUNTIME_DIR 未设置）。" >&2
        echo "解决: 先 loginctl enable-linger \$(id -u)，再重跑本脚本。" >&2
        exit 1
    fi
    # 登出后仍常驻（无头/服务器场景）；失败仅提示不阻断
    loginctl enable-linger "$(id -u)" 2>/dev/null || \
        echo "  提示: 如需登出后仍常驻，请执行 loginctl enable-linger \$(id -u)"
    echo "✅ 已注册 systemd 用户服务: $unit"
    echo "   行为: 开机自启 + 崩溃 60s 后自动重启（--no-tray 无头形态）"
    echo "   卸载: scripts/install-watchdog.sh remove"
}

remove_linux() {
    unit="$HOME/.config/systemd/user/dsh-come.service"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now dsh-come.service 2>/dev/null || true
        systemctl --user daemon-reload 2>/dev/null || true
    fi
    rm -f "$unit"
    echo "✅ 已移除 systemd 用户服务"
}

# ---------- 分发 ----------
case "$(uname -s)" in
    Darwin)
        if [ "$action" = "remove" ]; then remove_macos; else install_macos; fi
        ;;
    Linux)
        if [ "$action" = "remove" ]; then remove_linux; else install_linux; fi
        ;;
    *)
        echo "不支持的系统: $(uname -s)（看门狗仅支持 macOS / Linux）" >&2
        exit 1
        ;;
esac
