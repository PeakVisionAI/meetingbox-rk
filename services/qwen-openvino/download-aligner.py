import urllib.request,json,pathlib,concurrent.futures,hashlib,time
root=pathlib.Path(r'D:\MeetingBox-Intel\asr-openvino\aligner');root.mkdir(parents=True,exist_ok=True)
repo='Qwen/Qwen3-ForcedAligner-0.6B';rev='c7cbfc2048c462b0d63a45797104fc9db3ad62b7'
info=[{'type': 'file', 'path': 'chat_template.json', 'size': 1161}, {'type': 'file', 'path': 'config.json', 'size': 5982}, {'type': 'file', 'path': 'generation_config.json', 'size': 115}, {'type': 'file', 'path': 'merges.txt', 'size': 1671853}, {'type': 'file', 'path': 'model.safetensors', 'size': 1835544544, 'lfs': {'oid': '47831d0e82f96b20e9034dba01a075ee06436654719f6a68289e49f1b65ce0e7'}}, {'type': 'file', 'path': 'preprocessor_config.json', 'size': 330}, {'type': 'file', 'path': 'tokenizer_config.json', 'size': 12666}, {'type': 'file', 'path': 'vocab.json', 'size': 2776833}]
for entry in info:
 name=entry['path']
 if name in ['.gitattributes','README.md'] or entry['type']!='file':continue
 url=f'https://modelscope.cn/models/{repo}/resolve/master/{name}'
 dst=root/name;size=entry['size']
 if dst.exists() and dst.stat().st_size==size:continue
 if size<10000000:
  dst.write_bytes(urllib.request.urlopen(url,timeout=60).read());print('Downloaded',name,flush=True);continue
 step=32*1024*1024;count=(size+step-1)//step
 def part(i):
  p=root/f'{name}.part{i}';a,b=i*step,min(size-1,(i+1)*step-1)
  for attempt in range(12):
   have=p.stat().st_size if p.exists() else 0
   if have==b-a+1:return p
   try:
    req=urllib.request.Request(url+f'?download=true&part={i}&attempt={attempt}',headers={'Range':f'bytes={a+have}-{b}'})
    with urllib.request.urlopen(req,timeout=45) as r:
     if r.status!=206 or not r.headers.get('Content-Range','').startswith(f'bytes {a+have}-'):raise RuntimeError('Invalid range')
     with p.open('ab') as f:
      while chunk:=r.read(256*1024):f.write(chunk)
   except Exception as e:print('Retry',i,attempt,str(e)[:100],flush=True);time.sleep(2)
  raise RuntimeError(f'Incomplete part {i}')
 with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
  parts=list(pool.map(part,range(count)))
 with dst.open('wb') as f:
  for p in parts:
   with p.open('rb') as s:
    while chunk:=s.read(1024*1024):f.write(chunk)
 expected=entry['lfs']['oid'];actual=hashlib.file_digest(dst.open('rb'),'sha256').hexdigest();assert expected==actual
 print('Verified',name,actual,flush=True)
 for p in parts:p.unlink()
(root/'revision.txt').write_text(rev)

