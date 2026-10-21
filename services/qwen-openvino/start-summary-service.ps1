$env:SUMMARY_MODEL_DIR='D:\MeetingBox-Intel\asr-openvino\summary-8b'
$env:HF_HUB_OFFLINE='1'
$env:TRANSFORMERS_OFFLINE='1'
$env:PYTHONUTF8='1'
Set-Location D:\MeetingBox-Intel\asr-openvino
& 'D:\MeetingBox-Intel\asr-openvino\venv\Scripts\python.exe' -u -m uvicorn summary_ov_server:app --host 192.168.44.1 --port 8061 1>> D:\MeetingBox-Intel\asr-openvino\summary-service.log 2>> D:\MeetingBox-Intel\asr-openvino\summary-service-error.log
exit $LASTEXITCODE
