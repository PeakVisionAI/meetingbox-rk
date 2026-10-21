import urllib.request,json,pathlib,concurrent.futures,hashlib,time
root=pathlib.Path(r'D:\MeetingBox-Intel\asr-openvino\summary-8b');root.mkdir(parents=True,exist_ok=True)
repo='OpenVINO/Qwen3-8B-int4-ov';rev='5c47abf4b8e12ebe8e99745bb0c1ec17e0c0abcc'
info=[{'type': 'file', 'path': 'added_tokens.json', 'size': 707}, {'type': 'file', 'path': 'chat_template.jinja', 'size': 4168}, {'type': 'file', 'path': 'config.json', 'size': 1537}, {'type': 'file', 'path': 'generation_config.json', 'size': 214}, {'type': 'file', 'path': 'merges.txt', 'size': 1671853}, {'type': 'file', 'path': 'openvino_config.json', 'size': 527}, {'type': 'file', 'path': 'openvino_detokenizer.bin', 'size': 2189717, 'lfs': {'oid': 'd73093f72c80ebcaffb76a192727170ecd3c6a06c4df52df0e76b3f70ad9d7f7'}}, {'type': 'file', 'path': 'openvino_detokenizer.xml', 'size': 11650}, {'type': 'file', 'path': 'openvino_model.bin', 'size': 4855348053, 'lfs': {'oid': 'f2e6960fde61d24a83c477c46f9ee7158748762089f2018fc2bb6a6d6c7f2a7e'}}, {'type': 'file', 'path': 'openvino_model.xml', 'size': 3807005}, {'type': 'file', 'path': 'openvino_tokenizer.bin', 'size': 5588835, 'lfs': {'oid': '63c613e041e2d7f84f7376d6b628b72c8af29259b4d1b27540035f0d9ef034b3'}}, {'type': 'file', 'path': 'openvino_tokenizer.xml', 'size': 28644}, {'type': 'file', 'path': 'special_tokens_map.json', 'size': 613}, {'type': 'file', 'path': 'tokenizer.json', 'size': 11422654, 'lfs': {'oid': 'aeb13307a71acd8fe81861d94ad54ab689df773318809eed3cbe794b4492dae4'}}, {'type': 'file', 'path': 'tokenizer_config.json', 'size': 5404}, {'type': 'file', 'path': 'vocab.json', 'size': 2776833}]
for entry in info:
 name=entry['path']
 if name in ['.gitattributes','README.md'] or entry['type']!='file':continue
 url=f'https://hf-mirror.com/{repo}/resolve/{rev}/{name}'
 dst=root/name;size=entry['size']
 if dst.exists() and dst.stat().st_size==size:continue
 if size<20*1024*1024:
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

