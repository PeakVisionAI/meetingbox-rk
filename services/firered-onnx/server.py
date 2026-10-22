"""Local Intel GPU Qwen3 ASR endpoint; raw PCM never leaves the host."""
import os,time,threading,json,base64,re
from pathlib import Path
import numpy as np
from fastapi import FastAPI,Request,HTTPException
import sys,ctypes
app=FastAPI(); lock=threading.Lock(); pipe=None; aligner=None; align_lock=threading.Lock(); device='CPU'; model_name='FireRedASR2-AED-INT8'
@app.on_event('startup')
def startup():
 global pipe,aligner
 root=str(Path(__file__).parent/'model-1.7b')
 test_root=Path('D:/MeetingBox-Intel/firered-int8-test')
 sys.path.insert(0,str(test_root/'deps'))
 global dll_dir,ort_dll
 dll_dir=os.add_dll_directory(str(test_root/'deps/onnxruntime/capi'))
 ort_dll=ctypes.WinDLL(str(test_root/'deps/onnxruntime/capi/onnxruntime.dll'))
 import sherpa_onnx
 pipe=sherpa_onnx.OfflineRecognizer.from_fire_red_asr(encoder=str(test_root/'encoder.int8.onnx'),decoder=str(test_root/'decoder.int8.onnx'),tokens=str(test_root/'tokens.txt'),num_threads=4,provider='cpu')
 warm=pipe.create_stream();warm.accept_waveform(16000,np.zeros(16000,dtype=np.float32));pipe.decode_stream(warm)

 import torch
 from qwen_asr import Qwen3ForcedAligner
 torch.set_num_threads(4)
 aligner=Qwen3ForcedAligner.from_pretrained(str(Path(root).parent/'aligner'),dtype=torch.float32,device_map='cpu')
 print(json.dumps({'ready':True,'device':device,'model':model_name}),flush=True)
@app.get('/health')
def health():return {'ready':pipe is not None and aligner is not None,'model':model_name,'device':device,'word_alignment':aligner is not None,'primary_asr':'FireRedASR2-AED-INT8','lightweight_asr':False,'streaming':'rolling-audio'}
@app.post('/transcribe')
async def transcribe(request:Request):
 if request.client.host not in ('127.0.0.1','192.168.44.50'):raise HTTPException(403,'Restricted to local meeting backend')
 if int(request.headers.get('content-length','0'))>640000:raise HTTPException(413,'20 second maximum')
 pcm=await request.body()
 if not pcm or len(pcm)%2 or len(pcm)>640000:raise HTTPException(400,'PCM16 mono 16kHz required')
 try:
  encoded=request.headers.get('X-ASR-Context','')
  if len(encoded)>12000:raise ValueError('Too long')
  context=base64.b64decode(encoded,validate=True).decode('utf8')[:2048]
 except (ValueError,UnicodeError):raise HTTPException(400,'Invalid vocabulary context')
 def infer():
  audio=np.frombuffer(pcm,dtype='<i2').astype(np.float32)/32768
  t=time.perf_counter()
  seconds=len(audio)/16000
  with lock:
   stream=pipe.create_stream();stream.accept_waveform(16000,audio);pipe.decode_stream(stream)
   text=stream.result.text.strip()
  if len(text)>seconds*14+30 or (len(text)>24 and re.search(r'(.{1,8})\1{5,}',text)):
   raise RuntimeError('Abnormal ASR repetition; retain draft')
  return {'text':text,'seconds':round(time.perf_counter()-t,4),'model':model_name,'device':device}
 import asyncio
 return await asyncio.to_thread(infer)

@app.post('/align')
async def align(request:Request):
 if request.client.host not in ('127.0.0.1','192.168.44.50'):raise HTTPException(403,'Local backend only')
 if int(request.headers.get('content-length','0'))>640000:raise HTTPException(413,'20 second maximum')
 pcm=await request.body()
 if not pcm or len(pcm)%2 or len(pcm)>640000:raise HTTPException(400,'PCM16 mono 16kHz required')
 try:
  encoded=request.headers.get('X-ASR-Text','')
  if len(encoded)>20000:raise ValueError('Too long')
  text=base64.b64decode(encoded,validate=True).decode('utf8')
  if not text.strip():raise ValueError('No transcript')
 except (ValueError,UnicodeError):raise HTTPException(400,'Invalid alignment text')
 def infer():
  audio=np.frombuffer(pcm,dtype='<i2').astype(np.float32)/32768
  t=time.perf_counter()
  with align_lock:result=aligner.align(audio=(audio,16000),text=text,language='Chinese')[0]
  return {'words':[{'text':w.text,'start_time':w.start_time,'end_time':w.end_time} for w in result.items],'seconds':round(time.perf_counter()-t,4)}
 import asyncio
 return await asyncio.to_thread(infer)
