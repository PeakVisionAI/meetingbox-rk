$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$modules=@('qwen_ov_server:app','audio_front_server:app','overlap_gpu_server:app')
$stopped=@()
# End only the three speech tasks, then catch detached venv child workers.
foreach($name in @('MeetingBox-Intel-ASR','MeetingBox-Intel-AudioFront','MeetingBox-Overlap-GPU')) {
 Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
}
foreach($p in Get-CimInstance Win32_Process -Filter "Name='python.exe' OR Name='pythonw.exe'") {
 if($p.CommandLine -match 'D:\\MeetingBox-Intel\\' -and @($modules | Where-Object {$p.CommandLine.Contains($_)}).Count -gt 0) {
  $stopped += $p.ProcessId
  Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
 }
}
Start-Sleep -Seconds 2
$remaining=@(Get-CimInstance Win32_Process -Filter "Name='python.exe' OR Name='pythonw.exe'" | Where-Object { $cmd=$_.CommandLine; $cmd -match 'D:\\MeetingBox-Intel\\' -and @($modules | Where-Object {$cmd.Contains($_)}).Count -gt 0 })
if($remaining.Count){throw 'Speech workers remain resident'}
$llm=@(Get-Process llama-server -ErrorAction SilentlyContinue)
if($llm.Count -eq 0) {
 Start-ScheduledTask -TaskName 'MeetingBox-Summary-35B'
}
$modelReady=$false
for($attempt=0;$attempt -lt 90;$attempt++) {
 try {if((Invoke-WebRequest 'http://127.0.0.1:8067/health' -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200){$modelReady=$true;break}}catch{}
 Start-Sleep -Seconds 2
}
if(!$modelReady){throw 'Summary LLM did not become ready; transcript retained'}
$llm=@(Get-Process llama-server -ErrorAction SilentlyContinue)
if($llm.Count -ne 1){throw 'Expected exactly one summary LLM worker'}
$gpu=@(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction Stop | ForEach-Object {
 if($_.Name -match '^pid_(\d+)_') {
  $processId=[int]$Matches[1]
  [pscustomobject]@{pid=$processId;bytes=[double]$_.DedicatedUsage+[double]$_.SharedUsage}
 }
} | Group-Object pid | ForEach-Object {
 [pscustomobject]@{pid=[int]$_.Name;bytes=($_.Group|Measure-Object bytes -Sum).Sum}
})
$otherLarge=@($gpu|Where-Object {$_.pid -notin $llm.Id -and $_.bytes -ge 1GB})
if($otherLarge.Count){throw 'Another process occupies at least 1 GiB GPU memory; final summary blocked'}
$os=Get-CimInstance Win32_OperatingSystem
$commit=Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory
if($os.FreePhysicalMemory -lt 6MB -or ($commit.CommitLimit-$commit.CommittedBytes) -lt 4GB){throw 'Insufficient memory headroom for final summary'}
@{time=(Get-Date).ToString('o');speech_workers_remaining=$remaining.Count;stopped_pids=$stopped;free_GiB=[math]::Round($os.FreePhysicalMemory/1MB,2);commit_headroom_GiB=[math]::Round(($commit.CommitLimit-$commit.CommittedBytes)/1GB,2);llm_workers=$llm.Count;gpu_processes=$gpu;other_large_gpu_processes=$otherLarge.Count;checked=$true} | ConvertTo-Json -Depth 4 -Compress
