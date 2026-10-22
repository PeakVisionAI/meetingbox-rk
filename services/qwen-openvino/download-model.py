import urllib.request,json,pathlib,concurrent.futures,hashlib,time
root=pathlib.Path(r'D:\MeetingBox-Intel\asr-openvino\original-1.7b');root.mkdir(parents=True,exist_ok=True)
repo='Qwen/Qwen3-ASR-1.7B';rev='7278e1e70fe206f11671096ffdd38061171dd6e5'
info=[{'type': 'file', 'oid': 'a6344aac8c09253b3b630fb776ae94478aa0275b', 'size': 1519, 'path': '.gitattributes'}, {'type': 'file', 'oid': '3a67d43f21febda213ff36efdb60ef10019af7f3', 'size': 57456, 'path': 'README.md'}, {'type': 'file', 'oid': 'c44736493efd71ec96218cc626904698cdb13235', 'size': 1161, 'path': 'chat_template.json'}, {'type': 'file', 'oid': '2bc16c9d4ca08963715cfb94d879799b9adbd0e9', 'size': 6194, 'path': 'config.json'}, {'type': 'file', 'oid': '7382a4d347c0a865b76bb1b8277f66a5ac312854', 'size': 142, 'path': 'generation_config.json'}, {'type': 'file', 'oid': '31349551d90c7606f325fe0f11bbb8bd5fa0d7c7', 'size': 1671853, 'path': 'merges.txt'}, {'type': 'file', 'oid': '1522f6592ff361cdc8b95692bbd8c513d68fc40a', 'size': 4220320824, 'lfs': {'oid': 'a4cd1f1a04d90b757dc7f7dd26254e69a013b19e80efe590a83c6a3bde8608d6', 'size': 4220320824, 'pointerSize': 135}, 'xetHash': 'b54414b27c0a1c30846c8791ffad74e14a0567c5f949185f2dedbdb03ecf3636', 'path': 'model-00001-of-00002.safetensors'}, {'type': 'file', 'oid': '5a13bd2988d1368745384b4acb6e8bab73f98a75', 'size': 478200688, 'lfs': {'oid': '6e0b9d9e09e2e0238e7ef3cc8a484ab387e91b90f1900bedf88bc92d7929ccfc', 'size': 478200688, 'pointerSize': 134}, 'xetHash': '11577930f4829ff7d0945e6697ac8a86a6c2b59a4b19b5f16dc5f239008e9edc', 'path': 'model-00002-of-00002.safetensors'}, {'type': 'file', 'oid': '1048a4eb4f21fef9aea06d8568a784b2b5595689', 'size': 64821, 'path': 'model.safetensors.index.json'}, {'type': 'file', 'oid': '8f7f07346466d5d494ec0d4969d1c3d0190eed72', 'size': 330, 'path': 'preprocessor_config.json'}, {'type': 'file', 'oid': 'b93109843922a40c6654c5449d3bf95372267c66', 'size': 12487, 'path': 'tokenizer_config.json'}, {'type': 'file', 'oid': '4783fe10ac3adce15ac8f358ef5462739852c569', 'size': 2776833, 'path': 'vocab.json'}]
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

