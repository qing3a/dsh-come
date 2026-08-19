<#
.SYNOPSIS
    注册「DSH 伴侣守护」计划任务，给守护进程自己一个守护。

.DESCRIPTION
    dsh-come 是「进程外 supervisor」——它守护 dsh，但 dsh-come 自身此前没有任何守护：
    一旦被任务管理器强杀 / winit 事件循环崩 / 图标生成抛异常，dsh 会变成孤儿继续占 3080，
    下次启动只能「认领」、不再有崩溃自愈。

    本脚本把 dsh-come 注册成计划任务：
      - 登录时启动（AtLogOn）
      - 每分钟重复检查（RepetitionInterval 1 分钟，长期有效）
      - 已在运行则不新建实例（MultipleInstances IgnoreNew，配合单实例 mutex）
    → dsh-come 崩溃/被关后约 1 分钟内自动复活，守护进程也有了守护。

    注意：因「每分钟重复 + 已在运行忽略」，若你从托盘主动「退出」，任务仍会在下一分钟把它拉起来
    （常驻伴侣的预期行为）。若不想自动复活，运行 Uninstall-Watchdog 或手动删除任务即可。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/install-watchdog.ps1
#>

$ErrorActionPreference = "Stop"

$taskName = "DSH伴侣守护"

# 解析 dsh-come.exe 路径：优先仓库内构建产物，回退到已发布的 dist
$repoRoot = Resolve-Path "$PSScriptRoot\.."
$exe = ""
foreach ($cand in @(
        "$repoRoot\target\release\dsh-come.exe",
        "$repoRoot\dist\dsh-come.exe",
        "$repoRoot\target\debug\dsh-come.exe"
    )) {
    if (Test-Path $cand) { $exe = (Resolve-Path $cand).Path; break }
}
if (-not $exe) {
    Write-Error "找不到 dsh-come.exe（请先 cargo build --release 或确认 dist\dsh-come.exe 存在）"
    exit 1
}

# 以「当前用户、最高权限」运行；不在电池/电源切换时停止，保证常驻。
$action = New-ScheduledTaskAction -Execute $exe
$trigger = New-ScheduledTaskTrigger -AtLogOn
$trigger.RepetitionInterval = [TimeSpan]::FromMinutes(1)
$trigger.RepetitionDuration = [TimeSpan]::FromDays(3650)   # 长期重复，等价于看门狗
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -MultipleInstances IgnoreNew `
    -RestartCount 3 `
    -RestartInterval ([TimeSpan]::FromMinutes(1)) `
    -ExecutionTimeLimit ([TimeSpan]::FromDays(3650))

# 当前用户上下文运行（无需管理员）；若需系统级常驻可加 -RunLevel Highest 并管理员运行。
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Force

Write-Host "✅ 已注册计划任务 '$taskName'"
Write-Host "   路径: $exe"
Write-Host "   行为: 登录启动 + 每分钟检查（已在运行则忽略），崩溃后约 1 分钟自动复活。"
Write-Host "   卸载: 任务计划程序 → 删除 '$taskName'，或 Register-ScheduledTask 同名覆盖 / 运行下方命令："
Write-Host "         Unregister-ScheduledTask -TaskName '$taskName' -Confirm:`$false"
