$ErrorActionPreference='Continue'
$env:PYTHONUTF8='1'
$env:MEETINGBOX_KNOWLEDGE_ROOT='D:\MeetingBox-Intel\knowledge-center'
Set-Location $env:MEETINGBOX_KNOWLEDGE_ROOT
& 'D:\MeetingBox-Intel\asr-openvino\venv\Scripts\python.exe' -u -m uvicorn knowledge_server:app --host 127.0.0.1 --port 8069 1>> knowledge.log 2>> knowledge-error.log
exit $LASTEXITCODE
