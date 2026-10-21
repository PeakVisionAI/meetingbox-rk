$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
function Ready($url) {
 try { $r=Invoke-RestMethod -Uri $url -TimeoutSec 2; return $r.ready -eq $true } catch { return $false }
}
try {
 $state=Invoke-RestMethod -Uri 'http://192.168.44.50:8020/runtime/model-resources' -TimeoutSec 3
 if($state.final_summary_active) { throw 'Final summary owns model resources; retry after completion' }
} catch {
 if($_.Exception.Message -like '*owns model resources*') { throw }
}
# Summary warm-up is independent of the speech readiness gate.
if((Get-ScheduledTask -TaskName 'MeetingBox-Intel-Summary').State -ne 'Running') { Start-ScheduledTask -TaskName 'MeetingBox-Intel-Summary' }
Start-Process -FilePath powershell.exe -ArgumentList '-NoProfile','-WindowStyle','Hidden','-ExecutionPolicy','Bypass','-File','D:\MeetingBox-Intel\app\Warm-Summary.ps1' -WindowStyle Hidden
foreach($item in @(@('MeetingBox-Intel-ASR','http://192.168.44.1:8060/health'),@('MeetingBox-Intel-AudioFront','http://192.168.44.1:8063/health'),@('MeetingBox-Overlap-GPU','http://192.168.44.1:8065/health'))) {
 if(!(Ready $item[1])) {
  $task=Get-ScheduledTask -TaskName $item[0]
  if($task.State -ne 'Running') { Start-ScheduledTask -TaskName $item[0] }
 }
}
$bridgeReachable=$false
try { $null=Invoke-RestMethod 'http://192.168.44.50:8020/runtime/model-resources' -TimeoutSec 2; $bridgeReachable=$true } catch {}
if(!$bridgeReachable) {
 $vm=Get-ScheduledTask -TaskName MeetingBox-Intel-VM
 if($vm.State -ne 'Running') { Start-ScheduledTask -TaskName MeetingBox-Intel-VM }
 $key='D:\MeetingBox-Intel\app\service-start-key'
 for($i=0;$i -lt 45;$i++) {
  if(Ready 'http://192.168.44.50:8020/healthz') { break }
  if(Test-Path -LiteralPath $key) {
   & "$env:WINDIR\System32\OpenSSH\ssh.exe" -i $key -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=yes -o UserKnownHostsFile=D:\MeetingBox-Intel\app\service-start-known-hosts meetingbox@192.168.44.50 start-meetingbox 2>>"D:\MeetingBox-Intel\app\service-start-$PID.log"
   if($LASTEXITCODE -eq 0) { break }
  }
  Start-Sleep -Seconds 2
 }
}


