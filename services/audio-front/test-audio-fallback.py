import os,asyncio,time,json
from unittest.mock import patch
from fire_red_stream import FireRedStream
from meeting_audio import MeetingAudio
os.environ['MEETINGBOX_PUNC_URL']='http://127.0.0.1:1'
s=object.__new__(FireRedStream);s.records={0:{'text':'测试原文123','is_partial':False}};s.pending={};s.punc_retry_after=0;errors=[];s.refinement_error=errors.append
t=time.perf_counter();asyncio.run(s.punctuate_final_records());assert s.records[0]['text']=='测试原文123';assert errors;assert time.perf_counter()-t<2
os.environ['MEETINGBOX_AUDIO_ENHANCE']='agc';raw=b'\x01\x00'*320
with patch('meeting_audio.ctypes.CDLL',side_effect=OSError('injected unavailable library')):
 a=MeetingAudio();assert a.failed;assert a.process(raw)==raw;a.close()
os.environ['MEETINGBOX_AUDIO_ENHANCE']='off';a=MeetingAudio();assert a.process(raw)==raw;a.close()
print(json.dumps({'punctuation_unavailable_preserves_text':True,'enhancement_unavailable_bypasses_raw':True,'off_preserves_raw':True}))
