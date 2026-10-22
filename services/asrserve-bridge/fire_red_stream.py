"""FireRed primary ASR with asynchronous speaker attribution and overlap separation."""
import asyncio,base64,json,time,urllib.request,os,re
from collections import deque
import numpy as np
from fast_stream import FastStream
from meeting_audio import MeetingAudio
from transcript_cleanup import display_record


def aligned_text_span(text,words,origin,low,high):
 """Trim by aligned word boundaries while retaining the punctuated source span."""
 positions=[];cursor=0;selected=[]
 for index,word in enumerate(words):
  token=word['text'].strip()
  if not token:return None
  matched=re.search(re.escape(token),text[cursor:],re.IGNORECASE)
  if matched is None:return None
  start=cursor+matched.start();end=cursor+matched.end();positions.append((start,end));cursor=end
  if low<=origin+(word['start_time']+word['end_time'])*500<high:selected.append(index)
 if not selected:return None
 first,last=selected[0],selected[-1]
 a=0 if first==0 else positions[first][0]
 b=len(text) if last==len(words)-1 else positions[last+1][0]
 return text[a:b].strip()

class FireRedStream(FastStream):
 def __init__(self,*args,**kwargs):
  super().__init__(*args,**kwargs)
  import sherpa_onnx
  config=sherpa_onnx.VadModelConfig();config.silero_vad.model=os.getenv('MEETINGBOX_VAD_MODEL','/fast-model/silero_vad.onnx');config.silero_vad.min_silence_duration=0.5;config.silero_vad.min_speech_duration=0.15;config.silero_vad.max_speech_duration=8;config.sample_rate=16000;config.num_threads=1
  self.vad=sherpa_onnx.VoiceActivityDetector(config,buffer_size_in_seconds=16)
  self.audio_front=MeetingAudio();self.enhanced_path=self.folder/'asr-enhanced.pcm'
  self.audio_front_failed=bool(self.audio_front.failed);self.punc_retry_after=0
  if self.audio_front.failed:self.refinement_error('Audio enhancement init bypass: '+self.audio_front.failed)
  from overlap_live import OverlapLive
  self.overlap_worker=OverlapLive(self) if os.getenv('MEETINGBOX_OVERLAP_ENABLED','0')=='1' else None
  self.active=False;self.last_voiced=0;self.last_request=0;self.partial_job=None;self.final_jobs=deque();self.decode_wake=asyncio.Event();self.decode_idle=asyncio.Event();self.decode_idle.set();self.decoder_task=asyncio.create_task(self.decode_loop())

 def take(self):
  result=super().take()
  result['segments']=[r for r in (display_record(x) for x in result['segments']) if r['text']]
  return result

 async def feed(self,pcm):
  self.speaker.feed(pcm);self.wake.set()
  try: enhanced=self.audio_front.process(pcm)
  except Exception as e:
   if not self.audio_front_failed:self.refinement_error('Audio enhancement bypass: '+str(e))
   self.audio_front_failed=True;enhanced=pcm
  with self.enhanced_path.open('ab') as f:f.write(enhanced)
  now=self.samples//16;values=np.frombuffer(pcm,dtype='<i2').astype(np.float32)
  self.vad.accept_waveform(values/32768)
  voiced=self.vad.is_speech_detected()
  while not self.vad.empty():self.vad.pop()
  if voiced:
   if not self.active:
    self.active=True;self.start_ms=max(0,now-len(values)//16-300);self.last_request=self.start_ms
   self.last_voiced=now
  if not self.active:return
  final=not voiced or now-self.start_ms>=7000
  if final or now-self.last_request>=1000:
   job={'index':self.index,'start':self.start_ms,'end':now,'final':final,'context':self.context}
   if final:
    self.final_jobs.append(job);self.partial_job=None;self.index+=1;self.active=False
   else:self.partial_job=job
   self.last_request=now;self.decode_idle.clear();self.decode_wake.set()

 def recognize(self,job):
  with self.enhanced_path.open('rb') as f:
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
     if not text and job['final']:text=old.get('raw_asr_text',old.get('text',''))
     if not text or (not job['final'] and old.get('is_partial') is False):continue
     r={'utterance':key,'start_ms':job['start'],'end_ms':job['end'],'text':text,'raw_asr_text':text,'speaker':old.get('speaker'),'is_partial':not job['final'],'is_refined':job['final'],'refine_model':'FireRedASR2-AED-INT8','primary_model':'FireRedASR2-AED-INT8','recognition_seconds':result.get('seconds'),'text_ready_audio_ms':self.samples//16,'generation':self.generation,'token_times_ms':[],'emitted_wall_time':time.time()}
     self.generation+=1;self.records[key]=r;self.pending[key]=dict(r)
     with (self.folder/'stream.events.jsonl').open('a',encoding='utf8') as f:f.write(json.dumps(r,ensure_ascii=False)+'\n')
     if job['final']:
      self.speaker.utterance(job['index'],job['start'],job['end'])
      if self.refine_task:
       try:self.refine_queue.put_nowait(dict(r))
       except asyncio.QueueFull:self.refinement_error('Alignment queue full; primary transcript retained')
    except Exception as e:self.refinement_error('FireRed primary: '+str(e))
   self.decode_idle.set()

 async def run_refinement(self):
  # Word alignment is asynchronous; recognition text comes from FireRed.
  while True:
   record=await self.refine_queue.get()
   try:
    aligned=await asyncio.to_thread(self.align,record);latest=self.records.get(record['utterance'],record)
    if latest.get('overlap_processed'):continue
    latest={**latest,'alignment':aligned,'alignment_seconds':aligned.get('seconds')}
    if aligned.get('words'):
     origin=aligned['audio_start_ms'];latest.update(speech_start_ms=origin+round(aligned['words'][0]['start_time']*1000),speech_end_ms=min(latest['end_ms'],origin+round(aligned['words'][-1]['end_time']*1000)))
    self.records[record['utterance']]=latest;self.pending[record['utterance']]=dict(latest)
    self.merge_overlap()
   except Exception as e:self.refinement_error('Alignment: '+str(e))
   finally:
    await self.punctuate_final_records()
    self.refine_queue.task_done()

 def punctuate(self,text):
  url=os.getenv('MEETINGBOX_PUNC_URL','')
  if not url:return {'text':text}
  req=urllib.request.Request(url+'/punctuate',data=json.dumps({'text':text}).encode(),headers={'Content-Type':'application/json'})
  with urllib.request.urlopen(req,timeout=3) as f:return json.load(f)

 async def punctuate_final_records(self):
  if not os.getenv('MEETINGBOX_PUNC_URL') or time.monotonic()<self.punc_retry_after:return
  for key,r in list(self.records.items()):
   if r.get('is_partial') or r.get('punctuation_input')==r.get('text'):continue
   try:
    source=r['text'];result=await asyncio.to_thread(self.punctuate,source)
    # Same record can be updated by asynchronous speaker assignment.
    current=self.records.get(key)
    if current is None or current.get('text')!=source:continue
    current.update(text=result.get('text',source),punctuation_model='FireRedPunc',punctuation_seconds=result.get('seconds'),punctuation_input=result.get('text',source))
    self.pending[key]=dict(current)
   except Exception as e:
    self.punc_retry_after=time.monotonic()+15;self.refinement_error('Punctuation bypass: '+str(e));break

 async def finish_punctuation(self,deadline):
  try:await asyncio.wait_for(self.punctuate_final_records(),max(.1,deadline-asyncio.get_running_loop().time()))
  except asyncio.TimeoutError:self.refinement_error('Punctuation stop deadline; transcript retained')

 def align(self,record):
  start_ms=record['start_ms']
  with (self.folder/'stream.pcm').open('rb') as audio:
   audio.seek(start_ms*32);pcm=audio.read((record['end_ms']-start_ms)*32)
  req=urllib.request.Request(self.refine_url+'/align',data=pcm,headers={'Content-Type':'application/octet-stream','X-ASR-Text':base64.b64encode(record.get('raw_asr_text',record['text']).encode()).decode()})
  with urllib.request.urlopen(req,timeout=15) as response:result=json.load(response)
  result['audio_start_ms']=start_ms
  return result

 def merge_overlap(self):
  records=sorted((r for r in self.records.values() if not r.get('is_partial') and not r.get('speaker_turn')),key=lambda r:r['utterance'])
  for i,r in enumerate(records):
   alignment=r.get('alignment')
   if not alignment or not alignment.get('words'):continue
   low=(r['start_ms']+records[i-1]['end_ms'])/2 if i else r['start_ms']
   high=(r['end_ms']+records[i+1]['start_ms'])/2 if i+1<len(records) else r['end_ms']
   origin=alignment['audio_start_ms']
   text=aligned_text_span(r['text'],alignment['words'],origin,low,high)
   if text and text!=r['text']:
    r.update(text=text,overlap_merged=True);self.pending[r['utterance']]=dict(r)

 async def finish(self):
  async with self.lock:
   if not self.done.is_set():
    deadline=asyncio.get_running_loop().time()+28
    if self.active:
     self.final_jobs.append({'index':self.index,'start':self.start_ms,'end':self.samples//16,'final':True,'context':self.context});self.partial_job=None;self.active=False;self.decode_idle.clear();self.decode_wake.set()
    try:await asyncio.wait_for(self.decode_idle.wait(),min(15,max(.1,deadline-asyncio.get_running_loop().time())))
    except asyncio.TimeoutError:self.refinement_error('Primary decode stop timeout; audio retained')
    self.decoder_task.cancel();self.speaker.ended=True;self.wake.set()
    try:await asyncio.wait_for(asyncio.shield(self.speaker_task),min(8,max(.1,deadline-asyncio.get_running_loop().time())))
    except asyncio.TimeoutError:self.speaker_task.cancel()
    if self.refine_task:
     try:await asyncio.wait_for(self.refine_queue.join(),max(.1,deadline-asyncio.get_running_loop().time()))
     except asyncio.TimeoutError:self.refinement_error('Alignment stop deadline; primary text retained')
     self.refine_task.cancel()
    if self.overlap_worker:await self.overlap_worker.finish(max(.1,deadline-asyncio.get_running_loop().time()))
    self.merge_overlap();self.finalize_word_speakers();await self.finish_punctuation(deadline);self.audio_front.close();self.speaker.audio=np.empty(0,dtype=np.float32);self.speaker.activity=np.empty((0,8),dtype=bool);self.speaker.cache=None;self.done.set()
    (self.folder/'live-final.json').write_text(json.dumps([r for r in (display_record(x) for x in self.records.values()) if r['text']],ensure_ascii=False),encoding='utf8')
   return self.take()
