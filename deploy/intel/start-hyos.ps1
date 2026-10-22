$env:MEETINGBOX_RUNTIME_CLONE_TEST='0'
$env:RUST_BACKTRACE='full'
$env:RUST_LIB_BACKTRACE='1'
$ErrorActionPreference = 'Stop'
$env:MEETINGBOX_ASR_URL = 'http://192.168.44.50:8020'
$env:MEETINGBOX_ASR_MODEL = 'confucius4-r2t2'
$env:MEETINGBOX_ASR_STREAM = '1'
$env:MEETINGBOX_SUMMARY_URL = 'http://192.168.44.50:8050'
$env:MEETINGBOX_SUMMARY_LLM_URL = 'http://192.168.44.50:8040/v1'
$env:MEETINGBOX_SUMMARY_MODEL = 'Qwen3.5-35B-A3B-Q4_K_M'
$env:MEETINGBOX_FINALIZE_TIMEOUT_SECONDS = '3600'
# Code-supported debug mode. Release continues to require a device license.
$env:MEETINGBOX_ALLOW_UNLICENSED_DEV = '1'
$appPath = 'D:\MeetingBox-Intel\app\HYOS.exe'
if (!(Test-Path -LiteralPath $appPath)) { throw 'HYOS.exe has not been built yet' }
$appDirectory = Split-Path -Parent $appPath
$encoderPath = Join-Path $appDirectory 'ffmpeg.exe'
$bundledEncoderPath = Join-Path $appDirectory 'ffmpeg-x86_64-pc-windows-msvc.exe'
if (!(Test-Path -LiteralPath $encoderPath) -and (Test-Path -LiteralPath $bundledEncoderPath)) {
    Copy-Item -LiteralPath $bundledEncoderPath -Destination $encoderPath
}
Start-Process -FilePath $appPath -WindowStyle Normal -RedirectStandardError 'D:\MeetingBox-Intel\app\runtime-error.log' -RedirectStandardOutput 'D:\MeetingBox-Intel\app\runtime-output.log'
