"""Download the pinned Chinese embedding weights and verify every file against saved official hashes."""
from pathlib import Path
import os,json,hashlib,requests
root=Path(os.getenv('MEETINGBOX_KNOWLEDGE_ROOT',r'D:\MeetingBox-Intel\knowledge-center'))/'embedding-model'
root.mkdir(parents=True,exist_ok=True)
provenance=json.loads(Path(__file__).with_name('embedding-provenance.json').read_text())
for name,expected in provenance['files'].items():
 path=root/name
 if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest()==expected:continue
 urls=['https://modelscope.cn/models/BAAI/bge-small-zh-v1.5/resolve/master/'+name+'?download=1','https://huggingface.co/BAAI/bge-small-zh-v1.5/resolve/'+provenance['revision']+'/'+name]
 for url in urls:
  try:
   response=requests.get(url,timeout=(15,90),stream=True);response.raise_for_status()
   part=path.with_suffix(path.suffix+'.part')
   with part.open('wb') as file:
    for chunk in response.iter_content(1024*1024):file.write(chunk)
   if hashlib.sha256(part.read_bytes()).hexdigest()!=expected:raise ValueError('Model file hash mismatch')
   part.replace(path);print(name+' verified',flush=True);break
  except (requests.RequestException,ValueError):continue
 else:raise RuntimeError('Cannot obtain a verified model file: '+name)
(root/'PROVENANCE.json').write_text(json.dumps(provenance,indent=2))
print('Chinese embedding model verified')
