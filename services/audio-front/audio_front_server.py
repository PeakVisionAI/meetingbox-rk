"""CPU-only punctuation service; does not rewrite lexical content."""
import os,sys,time,threading,unicodedata,re
from pathlib import Path
import torch
from fastapi import FastAPI,HTTPException,Request
root=Path(__file__).parent
sys.path.insert(0,str(root/'punc-code'))
from fireredasr2s.fireredpunc.punc import FireRedPunc,FireRedPuncConfig
app=FastAPI();model=None;lock=threading.Lock()
def lexical(s):return ''.join(c for c in s if not unicodedata.category(c).startswith(('P','Z'))).casefold()
def project_marks(source, predicted):
 # Preserve original casing, spacing and technical expressions exactly.
 positions=[i for i,c in enumerate(source) if not unicodedata.category(c).startswith(('P','Z'))]
 boundaries={};n=0
 for c in predicted:
  if not unicodedata.category(c).startswith(('P','Z')):n+=1
  elif c in '，。？！；：、,.?!;:':boundaries.setdefault(n,'');boundaries[n]+=c
 protected=[m.span() for m in re.finditer(r'[A-Za-z0-9]+(?:[ .:/_+\-][A-Za-z0-9]+)*',source)]
 result=[];cursor=0
 for n,i in enumerate(positions,1):
  result.append(source[cursor:i+1]);cursor=i+1
  end=positions[n] if n<len(positions) else len(source)
  gap=source[cursor:end]
  if n in boundaries and not any(unicodedata.category(c).startswith('P') for c in gap) and not any(a<i+1<b for a,b in protected):result.append(boundaries[n])
 result.append(source[cursor:]);return ''.join(result)
@app.on_event('startup')
def start():
 global model
 torch.set_num_threads(2)
 model=FireRedPunc.from_pretrained(str(root/'punc'),FireRedPuncConfig(use_gpu=False))
 model.process(['你好今天测试会议标点谢谢'])
@app.get('/health')
def health():return {'ready':model is not None,'model':'FireRedPunc','device':'CPU','threads':2}
@app.post('/punctuate')
async def punctuate(req:Request):
 if req.client.host not in ('127.0.0.1','192.168.44.1','192.168.44.50'):raise HTTPException(403)
 data=await req.json();text=str(data.get('text',''))
 if len(text)>2000:raise HTTPException(413)
 def run():
  t=time.perf_counter()
  with lock:out=model.process([text])[0]['punc_text'] if text.strip() else text
  # Do not allow casing/tokenizer normalisation to change words, numbers or names.
  accepted=lexical(out)==lexical(text)
  return {'text':project_marks(text,out) if accepted else text,'accepted':accepted,'seconds':round(time.perf_counter()-t,4),'model':'FireRedPunc'}
 import asyncio
 return await asyncio.to_thread(run)
