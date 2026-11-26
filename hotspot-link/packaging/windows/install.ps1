$binaryPath = "$env:LOCALAPPDATA\HotspotLink\hotspotlinkd.exe"
$binaryDir = Split-Path $binaryPath

if (-not (Test-Path $binaryDir)) {
    New-Item -ItemType Directory -Path $binaryDir -Force | Out-Null
}

Copy-Item -Path ".\hotspotlinkd.exe" -Destination $binaryPath -Force

$taskName = "HotspotLink"
$action = New-ScheduledTaskAction -Execute $binaryPath
$trigger = New-ScheduledTaskTrigger -AtLogOn
$settings = New-ScheduledTaskSettingsSet -Hidden

Unregister-ScheduledTask -TaskName $taskName -Force -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings

Write-Host "HotspotLink daemon installed as hidden Task Scheduler task"
Write-Host "Binary copied to: $binaryPath"