$ErrorActionPreference='Continue'
$env:PYTHONUTF8='1'
$env:PYTHONIOENCODING='utf-8'
$env:NO_COLOR='1'
$env:HF_HUB_OFFLINE='1'
$env:TRANSFORMERS_OFFLINE='1'
$env:HF_HUB_DISABLE_TELEMETRY='1'
$root='D:\MeetingBox-Intel\asr-openvino'
& "$root\venv\Scripts\optimum-cli.exe" export openvino --model "$root\original-1.7b" --task automatic-speech-recognition-with-past --trust-remote-code --weight-format int8 "$root\model-1.7b" *> "$root\export17.log"
if($LASTEXITCODE -ne 0){Get-Content "$root\export17.log" -Tail 30;throw 'Model export failed'}
Get-Content "$root\export17.log" -Tail 15


