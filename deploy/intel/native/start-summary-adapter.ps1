$env:SUMMARY_SOURCE_ONLY='0'
$env:SUMMARY_DISABLED='0'
$env:PYTHONUTF8='1'
Start-ScheduledTask -TaskName 'MeetingBox-Summary-35B'
Set-Location D:\MeetingBox-Intel\asr-openvino
& 'D:\MeetingBox-Intel\asr-openvino\venv\Scripts\python.exe' -u -m uvicorn summary_ov_server:app --host 192.168.44.1 --port 8061 1>> D:\MeetingBox-Intel\asr-openvino\summary-service.log 2>> D:\MeetingBox-Intel\asr-openvino\summary-service-error.log
exit $LASTEXITCODE
