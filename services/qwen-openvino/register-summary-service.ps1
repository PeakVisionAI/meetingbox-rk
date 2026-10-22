$ErrorActionPreference='Stop'
$root='D:\MeetingBox-Intel\asr-openvino'
$principal=(Get-ScheduledTask -TaskName 'MeetingBox-Intel-App').Principal
$action=New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File $root\start-summary-service.ps1"
$trigger=New-ScheduledTaskTrigger -AtLogOn -User $principal.UserId
$settings=New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -StartWhenAvailable
Register-ScheduledTask -TaskName 'MeetingBox-Intel-Summary' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
if(-not (Get-NetFirewallRule -DisplayName 'MeetingBox Summary VM only' -ErrorAction SilentlyContinue)){
 New-NetFirewallRule -DisplayName 'MeetingBox Summary VM only' -Direction Inbound -Action Allow -Protocol TCP -LocalAddress 192.168.44.1 -LocalPort 8061 -RemoteAddress 192.168.44.50 -Profile Any | Out-Null
}
Start-ScheduledTask -TaskName 'MeetingBox-Intel-Summary'

