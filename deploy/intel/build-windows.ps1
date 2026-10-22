$ErrorActionPreference = 'Stop'
$env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
$taskVcCommand = '"D:\MeetingBox-Intel\BuildTools\Common7\Tools\VsDevCmd.bat" -arch=x64 -host_arch=x64 >nul && set'
& cmd.exe /c $taskVcCommand | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
}
$env:CARGO_HOME = 'D:\MeetingBox-Intel\toolchain\cargo'
$env:RUSTUP_HOME = 'D:\MeetingBox-Intel\toolchain\rustup'
$env:CARGO_TARGET_DIR = 'D:\MeetingBox-Intel\build-target'
$env:LIBCLANG_PATH = 'D:\MeetingBox-Intel\toolchain\libclang\clang\native'
$env:CL = ($env:CL + ' /utf-8').Trim()
$env:Path = $env:CARGO_HOME + '\bin;' + $env:Path
Set-Location -LiteralPath 'D:\MeetingBox-Intel\source\meetingbox-rk\frontend'
& 'C:\Program Files\nodejs\node.exe' 'D:\MeetingBox-Intel\toolchain\tauri\node_modules\@tauri-apps\cli\tauri.js' build --debug --no-bundle --config '../deploy/intel/tauri.windows.json'
if ($LASTEXITCODE -ne 0) { throw "Windows build failed: $LASTEXITCODE" }
