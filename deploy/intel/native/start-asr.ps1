$ErrorActionPreference='Continue'
$root='D:\MeetingBox-Intel\asr-openvino'
$env:ASR_MODEL_DIR="$root\model-1.7b"
$env:ASR_DEVICE='CPU'
$env:ASR_MODEL_NAME='FireRedASR2-AED-INT8'
$env:HF_HUB_OFFLINE='1'
$env:TRANSFORMERS_OFFLINE='1'
$env:PYTHONUTF8='1'
Set-Location $root
& "$root\venv\Scripts\python.exe" -u -m uvicorn qwen_ov_server:app --host 192.168.44.1 --port 8060 1>> "$root\service.log" 2>> "$root\service-error.log"

exit $LASTEXITCODE
