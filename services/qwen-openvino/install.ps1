$ErrorActionPreference='Stop'
$process=Get-CimInstance Win32_Process -Filter 'ProcessId=11840'
if($process.CommandLine -match 'asr-openvino.*pip install'){Stop-Process -Id 11840 -Force}
& 'D:\MeetingBox-Intel\asr-openvino\venv\Scripts\python.exe' -m pip install --timeout 20 --retries 2 --index-url https://pypi.tuna.tsinghua.edu.cn/simple --log D:\MeetingBox-Intel\asr-openvino\install.log openvino-genai==2026.4.1.0 'optimum-intel[openvino]==2.2.0' qwen-asr==0.0.6 fastapi uvicorn soundfile
if($LASTEXITCODE -ne 0){throw 'Install failed'}
& 'D:\MeetingBox-Intel\asr-openvino\venv\Scripts\python.exe' -c "import openvino as ov,openvino_genai as g; c=ov.Core(); print(ov.__version__); print(c.available_devices); print('ASRPipeline',hasattr(g,'ASRPipeline')); print(c.get_property('GPU','FULL_DEVICE_NAME'))"
