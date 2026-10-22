$ErrorActionPreference='Stop'
$root='D:\MeetingBox-Intel\audio-front'
$env:HF_HUB_OFFLINE='1';$env:TRANSFORMERS_OFFLINE='1';$env:PYTHONUTF8='1'
$p=Start-Process -FilePath 'D:\MeetingBox-Intel\asr-openvino\venv\Scripts\python.exe' -ArgumentList '-u','-m','uvicorn','audio_front_server:app','--app-dir',$root,'--host','192.168.44.1','--port','8063' -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput "$root\service.log" -RedirectStandardError "$root\service-error.log" -Wait -PassThru
exit $p.ExitCode
