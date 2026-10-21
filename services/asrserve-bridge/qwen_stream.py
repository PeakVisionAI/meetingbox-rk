"""Single Qwen3-ASR primary: live audio in, rolling hypotheses out; no draft ASR."""
import asyncio,base64,json,time,urllib.request
from collections import deque
import numpy as np
from fast_stream import FastStream

class QwenStream(FastStream):
 def __init__(self,*args,**kwargs):
  super().__init__(*args,**kwargs)
  self.active=False;self.last_voiced=0;self.last_request=0;self.partial_job=None;self.final_jobs=deque();self.decode_wake=asyncio.Event();self.decode_idle=asyncio.Event();self.decode_idle.set();self.decoder_task=asyncio.create_task(self.decode_loop())

 async def feed(self,pcm):
  self.speaker.feed(pcm);self.wake.set()
  now=self.samples//16;values=np.frombuffer(pcm,dtype='<i2').astype(np.float32)
  voiced=bool(values.size and np.sqrt(np.mean(values*values))>=80)
  if voiced:
   if not self.active:
    self.active=True;self.start_ms=max(0,now-len(values)//16-300);self.last_request=self.start_ms
   self.last_voiced=now
  if not self.active:return
  final=now-self.last_voiced>=800 or now-self.start_ms>=7000
  if final or now-self.last_request>=1000:
   job={'index':self.index,'start':self.start_ms,'end':now,'final':final,'context':self.context}
   if final:
    self.final_jobs.append(job);self.partial_job=None;self.index+=1;self.active=False
   else:self.partial_job=job
   self.last_request=now;self.decode_idle.clear();self.decode_wake.set()

 def recognize(self,job):
  with (self.folder/'stream.pcm').open('rb') as f:
   f.seek(job['start']*32);pcm=f.read((job['end']-job['start'])*32)
  req=urllib.request.Request(self.refine_url+'/transcribe',data=pcm,headers={'Content-Type':'application/octet-stream','X-ASR-Context':base64.b64encode(job['context'].encode()).decode()})
  with urllib.request.urlopen(req,timeout=20) as r:return json.load(r)

 async def decode_loop(self):
  while True:
   await self.decode_wake.wait();self.decode_wake.clear()
   while self.final_jobs or self.partial_job:
    job=self.final_jobs.popleft() if self.final_jobs else self.partial_job
    if not job['final']:self.partial_job=None
    try:
     result=await asyncio.to_thread(self.recognize,job);text=result.get('text','').strip();key=job['index']*100;old=self.records.get(key,{})
     if not text or (not job['final'] and old.get('is_partial') is False):continue
     previous=[r for k,r in self.records.items() if k<key and not r.get('is_partial') and not r.get('speaker_turn')]
     if previous and job['start']<=previous[-1]['end_ms']:
      prior=previous[-1]['text']
      for n in range(min(8,len(prior),len(text)),1,-1):
       if prior.endswith(text[:n]):text=text[n:].lstrip('，。 ');break
     if not text:continue
     r={'utterance':key,'start_ms':job['start'],'end_ms':job['end'],'text':text,'speaker':old.get('speaker'),'is_partial':not job['final'],'is_refined':job['final'],'refine_model':'Qwen3-ASR-1.7B','primary_model':'Qwen3-ASR-1.7B','recognition_seconds':result.get('seconds'),'text_ready_audio_ms':self.samples//16,'generation':self.generation,'token_times_ms':[],'emitted_wall_time':time.time()}
     self.generation+=1;self.records[key]=r;self.pending[key]=dict(r)
     with (self.folder/'stream.events.jsonl').open('a',encoding='utf8') as f:f.write(json.dumps(r,ensure_ascii=False)+'\n')
     if job['final']:
      self.speaker.utterance(job['index'],job['start'],job['end'])
      if self.refine_task:
       try:self.refine_queue.put_nowait(dict(r))
       except asyncio.QueueFull:self.refinement_error('Alignment queue full; primary transcript retained')
    except Exception as e:self.refinement_error('Qwen primary: '+str(e))
   self.decode_idle.set()

 async def run_refinement(self):
  # Word alignment is asynchronous; recognition text comes only from Qwen.
  while True:
   record=await self.refine_queue.get()
   try:
    aligned=await asyncio.to_thread(self.align,record);latest=self.records.get(record['utterance'],record)
    latest={**latest,'alignment':aligned,'alignment_seconds':aligned.get('seconds')}
    self.records[record['utterance']]=latest;self.pending[record['utterance']]=dict(latest)
   except Exception as e:self.refinement_error('Alignment: '+str(e))
   finally:self.refine_queue.task_done()

 async def finish(self):
  async with self.lock:
   if not self.done.is_set():
    if self.active:
     self.final_jobs.append({'index':self.index,'start':self.start_ms,'end':self.samples//16,'final':True,'context':self.context});self.partial_job=None;self.active=False;self.decode_idle.clear();self.decode_wake.set()
    try:await asyncio.wait_for(self.decode_idle.wait(),30)
    except asyncio.TimeoutError:self.refinement_error('Primary decode stop timeout; audio retained')
    self.decoder_task.cancel();self.speaker.ended=True;self.wake.set()
    try:await asyncio.wait_for(asyncio.shield(self.speaker_task),15)
    except asyncio.TimeoutError:self.speaker_task.cancel()
    if self.refine_task:
     try:await asyncio.wait_for(self.refine_queue.join(),15)
     except asyncio.TimeoutError:self.refinement_error('Alignment stop deadline; primary text retained')
     self.refine_task.cancel()
    self.finalize_word_speakers();self.speaker.audio=np.empty(0,dtype=np.float32);self.speaker.activity=np.empty((0,8),dtype=bool);self.speaker.cache=None;self.done.set()
    (self.folder/'live-final.json').write_text(json.dumps(list(self.records.values()),ensure_ascii=False),encoding='utf8')
   return self.take()
