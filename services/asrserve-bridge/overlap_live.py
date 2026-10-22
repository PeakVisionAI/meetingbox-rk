"""Conservative asynchronous overlap replacement, using meeting-local clean enrollment."""
import asyncio, base64, json, os, time, urllib.request, wave
from pathlib import Path
import numpy as np

URL=os.getenv('MEETINGBOX_OVERLAP_URL','http://192.168.44.1:8065')

def request(path, pcm, timeout=25):
    req=urllib.request.Request(URL+path,data=pcm,headers={'Content-Type':'application/octet-stream'})
    with urllib.request.urlopen(req,timeout=timeout) as response:return json.load(response)

def read_pcm(owner,start,end):
    with (owner.folder/'stream.pcm').open('rb') as f:
        f.seek(round(start)*32);return f.read(round(end-start)*32)

def save_wav(path,pcm):
    with wave.open(str(path),'wb') as w:
        w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes(pcm)

class OverlapLive:
    def __init__(self,owner):
        self.o=owner;self.refs={};self.tried=set();self.stopping=False
        self.task=asyncio.create_task(self.run())

    async def enroll(self,activity):
        # Use exclusively single-active-speaker runs. No reference RTTM is used.
        count=activity.sum(1)
        for col in range(activity.shape[1]):
            if col in self.refs:continue
            mask=(count==1)&activity[:,col]
            edges=np.diff(np.r_[False,mask,False].astype(int))
            runs=[(a,b) for a,b in zip(np.where(edges==1)[0],np.where(edges==-1)[0]) if b-a>=150]
            if not runs:continue
            a,b=max(runs,key=lambda p:p[1]-p[0]);a+=10;b=min(b-10,a+400)
            try:
                result=await asyncio.to_thread(request,'/embed',read_pcm(self.o,a*10,b*10))
                self.refs[col]=np.array(result['embedding'])
            except Exception as e:self.o.refinement_error('Overlap enrollment bypass: '+str(e));return

    async def process(self,r,activity):
        start,end=r['start_ms'],r['end_ms'];rows=activity[start//10:end//10]
        if len(rows)==0 or int((rows.sum(1)>=2).sum())<20:return
        if len(self.refs)<2:return
        self.tried.add(r['utterance'])
        audit={'utterance':r['utterance'],'start_ms':start,'end_ms':end,'status':'checking','chunks':[]}
        mask=rows.sum(1)>=2;edges=np.diff(np.r_[False,mask,False].astype(int))
        runs=list(zip(np.where(edges==1)[0],np.where(edges==-1)[0]))
        a,b=max(runs,key=lambda v:v[1]-v[0]);center=start+(a+b)*5
        offset=int(max(start,min(end-4500,center-2250)));stop=int(min(end,offset+4500))
        result=await asyncio.to_thread(request,'/separate',read_pcm(self.o,offset,stop))
        tracks=result['tracks'];chunk={'start_ms':offset,'end_ms':stop,'separation_seconds':result['seconds'],'tracks':[]};audit['chunks'].append(chunk)
        parts=[];matches=[]
        if len(tracks)==2:
            for track in tracks:
                scores=sorted(((float(np.array(track['embedding'])@v),c) for c,v in self.refs.items()),reverse=True)
                score,col=scores[0];margin=score-scores[1][0]
                chunk['tracks'].append({'column':col,'score':score,'margin':margin,'raw_relative_db':track['raw_relative_db']})
                matches.append(col if score>=.45 and margin>=.06 else None)
        if len(matches)!=2 or None in matches or len(set(matches))!=2:
            audit['status']='retained-uncertain-identity'
        else:
            alignment=r.get('alignment',{});words=alignment.get('words',[]);origin=alignment.get('audio_start_ms',start)
            def outside(low,high):
                selected=[w for w in words if low<=origin+(w['start_time']+w['end_time'])*500<high]
                return ''.join(w['text'] for w in selected).strip()
            prefix=outside(start,offset);suffix=outside(stop,end)
            if prefix:parts.append({**r,'start_ms':start,'end_ms':offset,'text':prefix})
            for j,(track,col) in enumerate(zip(tracks,matches)):
                pcm=base64.b64decode(track['pcm']);name=f"overlap-{r['utterance']}-{offset}-{j}.wav";save_wav(self.o.folder/name,pcm)
                req=urllib.request.Request(self.o.refine_url+'/transcribe',data=pcm,headers={'Content-Type':'application/octet-stream'})
                def decode():
                    with urllib.request.urlopen(req,timeout=20) as f:return json.load(f)
                decoded=await asyncio.to_thread(decode)
                from transcript_cleanup import clean_fillers
                text=clean_fillers(decoded.get('text','').strip())
                if not text:continue
                label=self.o.speaker.labels.setdefault(col,f'说话人{len(self.o.speaker.labels)+1}').replace('说话人','发言人',1)
                parts.append({**r,'start_ms':offset,'end_ms':stop,'text':text,'raw_asr_text':text,'speaker':label,
                    'overlap_separated':True,'overlap_audio':name,'separator':'MossFormer2-OpenVINO-FP16',
                    'speaker_match_cosine':chunk['tracks'][j]['score']})
            if suffix:parts.append({**r,'start_ms':stop,'end_ms':end,'text':suffix})
            if sum(bool(p.get('overlap_separated')) for p in parts)==2:
                for i,part in enumerate(parts):
                    part.update(utterance=r['utterance']+i,speech_start_ms=part['start_ms'],speech_end_ms=part['end_ms'],
                        speaker_turn=True,overlap_processed=True,is_partial=False,token_times_ms=[],emitted_wall_time=time.time())
                    part.pop('alignment',None);part.pop('punctuation_input',None)
                    self.o.records[part['utterance']]=part;self.o.pending[part['utterance']]=dict(part)
                    with (self.o.folder/'stream.events.jsonl').open('a',encoding='utf8') as f:f.write(json.dumps(part,ensure_ascii=False)+'\n')
                audit['status']='replaced';audit['parts']=len(parts)
            else:audit['status']='retained-insufficient-text'
        with (self.o.folder/'overlap-audit.jsonl').open('a',encoding='utf8') as f:f.write(json.dumps(audit,ensure_ascii=False)+'\n')

    async def scan(self):
        activity=self.o.speaker.activity[:self.o.speaker.frames].copy()
        if len(activity)==0:return
        await self.enroll(activity)
        for r in list(self.o.records.values()):
            if not r.get('alignment',{}).get('words') or r.get('is_partial') or r.get('speaker_turn') or r['utterance'] in self.tried:continue
            if r['end_ms']>len(activity)*10:continue
            await self.process(dict(r),activity)

    async def run(self):
        while not self.stopping:
            try:await self.scan()
            except Exception as e:self.o.refinement_error('Overlap retained original: '+str(e))
            await asyncio.sleep(.7)

    async def finish(self,seconds):
        self.stopping=True;deadline=asyncio.get_running_loop().time()+seconds
        try:
            await asyncio.wait_for(self.task,max(.1,seconds))
            await asyncio.wait_for(self.scan(),max(.1,deadline-asyncio.get_running_loop().time()))
        except asyncio.TimeoutError:
            self.task.cancel();self.o.refinement_error('Overlap stop deadline; available original transcript retained')
