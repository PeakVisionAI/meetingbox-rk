import sys,os,time,pathlib,json,numpy as np
sys.path.insert(0,'/app');from meeting_audio import MeetingAudio
root=pathlib.Path('/test');raw=(root/'recent-stream.pcm').read_bytes();stats=[]
for mode in ['agc','ns','agc_ns']:
 os.environ['MEETINGBOX_AUDIO_ENHANCE']=mode;p=MeetingAudio();t=time.perf_counter();out=b''.join(p.process(raw[i:i+6400]) for i in range(0,len(raw),6400));p.close();seconds=time.perf_counter()-t
 (root/('recent-'+mode+'.pcm')).write_bytes(out)
 a=np.frombuffer(out,dtype='<i2').astype(float);r=np.frombuffer(raw,dtype='<i2').astype(float)
 stats.append(dict(mode=mode,seconds=seconds,audio_seconds=len(raw)/32000,same_length=len(out)==len(raw),clipped_fraction=float(np.mean(abs(a)>=32760)),peak=float(max(abs(a))),rms=float(np.sqrt(np.mean(a*a))),raw_rms=float(np.sqrt(np.mean(r*r)))))
(root/'enhancement-stats.json').write_text(json.dumps(stats,indent=2));print(json.dumps(stats))
