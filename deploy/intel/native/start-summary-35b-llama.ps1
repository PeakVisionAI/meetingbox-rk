$ProgressPreference='SilentlyContinue'
$root='D:\MeetingBox-Intel\summary-35b'
$env:LLAMA_ARG_CHAT_TEMPLATE_KWARGS='{"enable_thinking":false}'
& "$root\runtime\llama-server.exe" -m "$root\Qwen3.5-35B-A3B-Q4_K_M.gguf" --host 127.0.0.1 --port 8067 -ngl 99 -c 8192 -b 512 -ub 128 -t 6 --flash-attn on --jinja --parallel 1 1>> "$root\server.log" 2>> "$root\server.err"
exit $LASTEXITCODE
