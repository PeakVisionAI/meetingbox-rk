$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$mutex=New-Object System.Threading.Mutex($false,'Local\MeetingBox-Summary-Warm')
$owns=$false
$status='D:\MeetingBox-Intel\app\summary-warm-status.json'
function Status($stage,$ready) {
 @{stage=$stage;ready=$ready;time=(Get-Date).ToString('o')} | ConvertTo-Json -Compress | Set-Content -LiteralPath $status -Encoding utf8
}
try {
 try {$owns=$mutex.WaitOne(0)} catch [System.Threading.AbandonedMutexException] {$owns=$true}
 if(!$owns){exit 0}
 try {if((Invoke-RestMethod 'http://192.168.44.1:8061/health' -TimeoutSec 2).ready){Status 'ready' $true;exit 0}}catch{}
 Status 'deferred' $false
 Start-Sleep -Seconds 30
 $runtime=Invoke-RestMethod 'http://192.168.44.50:8020/runtime/model-resources' -TimeoutSec 3
 if($runtime.final_summary_active){Status 'final-summary-active' $false;exit 0}
 foreach($name in @('MeetingBox-Intel-Summary','MeetingBox-Summary-35B')) {
  if((Get-ScheduledTask -TaskName $name).State -ne 'Running'){Start-ScheduledTask -TaskName $name}
 }
 Status 'loading' $false
 for($i=0;$i -lt 150;$i++) {
  try {if((Invoke-RestMethod 'http://192.168.44.1:8061/health' -TimeoutSec 2).ready){Status 'ready' $true;exit 0}}catch{}
  Start-Sleep -Seconds 2
 }
 Status 'timeout-transcript-retained' $false
} catch {Status 'failed-transcript-retained' $false;throw} finally {if($owns){$mutex.ReleaseMutex()};$mutex.Dispose()}
