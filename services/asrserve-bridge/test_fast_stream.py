import asyncio,importlib.util,json,time
from pathlib import Path
import numpy as np
import pytest
spec=importlib.util.spec_from_file_location('fast_stream',Path(__file__).with_name('fast_stream.py'));fast=importlib.util.module_from_spec(spec);spec.loader.exec_module(fast)

def test_final_attribution_updates_parent_and_adds_children_without_losing_text(tmp_path):
    import sys
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).parent))
    activity=np.zeros((200,2),dtype=bool);activity[:100,0]=True;activity[100:,1]=True
    stream=object.__new__(fast.FastStream);stream.folder=tmp_path
    stream.speaker=SimpleNamespace(activity=activity,frames=200,labels={0:'说话人1',1:'说话人2'})
    original={'utterance':100,'text':'甲话。乙话！','speaker':'发言人1','is_partial':False,
              'alignment':{'audio_start_ms':0,'words':[{'text':'甲话','start_time':0,'end_time':1},{'text':'乙话','start_time':1,'end_time':2}]}}
    stream.records={100:original,200:{'utterance':200,'text':'下一句'}};stream.pending={}
    stream.finalize_word_speakers()
    assert stream.records[100]['speaker']=='发言人1' and stream.records[101]['speaker']=='发言人2'
    assert stream.records[100]['text']+stream.records[101]['text']==original['text']
    assert len(stream.records[101]['alignment']['words'])==1
    assert stream.records[200]['text']=='下一句' and set(stream.pending)=={100,101}
    stream.finalize_word_speakers();assert len(stream.records)==3

@pytest.mark.asyncio
async def test_text_does_not_wait_for_speaker_and_labels_replace_same_utterance(tmp_path,monkeypatch):
    gate=asyncio.Event()
    class Speaker:
        ended=False
        audio=np.empty(0);activity=np.empty((0,8));cache=None
        def __init__(self):self.pending=[]
        def feed(self,pcm):pass
        def utterance(self,i,a,b):self.pending.append(i)
        async def advance(self):
            await gate.wait()
            items=[{'utterance':i,'speaker':'说话人1'} for i in self.pending];self.pending=[];return items
    class Stream:
        def accept_waveform(self,*args):pass
        def input_finished(self):pass
    class Recognizer:
        text='测试';endpoint=False
        def create_stream(self):return Stream()
        def is_ready(self,s):return False
        def decode_stream(self,s):pass
        def get_result_as_json_string(self,s):return json.dumps({'text':self.text,'timestamps':[.2],'start_time':0})
        def is_endpoint(self,s):return self.endpoint
        def reset(self,s):self.text='';self.endpoint=False
    recognizer=Recognizer();monkeypatch.setattr(fast,'recognizer',recognizer)
    stream=fast.FastStream(1,tmp_path,Speaker());stream.samples=3200
    await asyncio.wait_for(stream.feed(b'\0'*6400),.5)
    first=stream.take()['segments'];assert len(first)==1 and first[0]['is_partial'] and first[0]['speaker'] is None
    recognizer.text='测试成功';recognizer.endpoint=True;stream.samples+=3200
    await stream.feed(b'\0'*6400)
    final=stream.take()['segments'];assert len(final)==1 and final[0]['utterance']==first[0]['utterance'] and not final[0]['is_partial']
    gate.set();await asyncio.sleep(.02)
    labelled=stream.take()['segments'];assert labelled[0]['speaker']=='发言人1' and labelled[0]['utterance']==first[0]['utterance']
    await stream.finish();assert stream.done.is_set() and len(stream.records)==1

@pytest.mark.asyncio
async def test_refinement_keeps_latest_speaker_and_same_id(tmp_path):
    import threading
    entered=threading.Event(); release=threading.Event()
    stream=object.__new__(fast.FastStream)
    stream.folder=tmp_path;stream.records={3:{'utterance':3,'text':'错误草稿','speaker':None,'is_partial':False}}
    stream.pending={};stream.refine_queue=asyncio.Queue()
    def refine(record):
        entered.set();assert release.wait(2)
        return {'text':'识别正确。','model':'Qwen3-ASR-0.6B','seconds':.2}
    stream.refine=refine
    task=asyncio.create_task(stream.run_refinement())
    stream.refine_queue.put_nowait(dict(stream.records[3]))
    assert await asyncio.to_thread(entered.wait,1)
    stream.records[3]['speaker']='发言人2'
    release.set();await asyncio.wait_for(stream.refine_queue.join(),2)
    assert stream.pending[3]['speaker']=='发言人2'
    assert stream.pending[3]['utterance']==3 and stream.pending[3]['text']=='识别正确。'
    assert stream.pending[3]['draft_text']=='错误草稿'
    task.cancel()
    with pytest.raises(asyncio.CancelledError):await task

@pytest.mark.asyncio
async def test_refinement_failure_retains_draft(tmp_path):
    stream=object.__new__(fast.FastStream)
    stream.folder=tmp_path;stream.records={1:{'utterance':1,'text':'原稿'}}
    stream.pending={};stream.refine_queue=asyncio.Queue()
    def fail(record):raise TimeoutError('offline')
    stream.refine=fail
    task=asyncio.create_task(stream.run_refinement())
    stream.refine_queue.put_nowait(dict(stream.records[1]))
    await asyncio.wait_for(stream.refine_queue.join(),2)
    assert stream.records[1]['text']=='原稿' and not stream.pending
    assert 'TimeoutError' in (tmp_path/'refinement-errors.log').read_text()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):await task

@pytest.mark.asyncio
async def test_aligned_turns_preserve_complete_text_and_replace_parent(tmp_path):
    import sys
    sys.path.insert(0,str(Path(__file__).parent))
    stream=object.__new__(fast.FastStream)
    stream.folder=tmp_path
    text='甲说话，乙接话。'
    stream.records={300:{'utterance':300,'text':text,'speaker':None,'mixed_speakers':True,'is_refined':True},400:{'utterance':400,'text':'下一句'}}
    stream.pending={};stream.refine_queue=asyncio.Queue()
    def align(record):
        return {'audio_start_ms':0,'words':[{'text':'甲说话','start_time':0,'end_time':1},{'text':'乙接话','start_time':1,'end_time':2.1}]}
    stream.align=align
    task=asyncio.create_task(stream.run_refinement())
    stream.refine_queue.put_nowait({'utterance':300,'text':text,'turns':[
        {'start_ms':0,'end_ms':1000,'speaker':'发言人1'},
        {'start_ms':1000,'end_ms':2100,'speaker':'发言人2'}]})
    await asyncio.wait_for(stream.refine_queue.join(),2)
    assert stream.records[300]['speaker']=='发言人1' and stream.records[301]['speaker']=='发言人2'
    assert stream.records[300]['text']+stream.records[301]['text']==text
    assert stream.records[400]['text']=='下一句' and len(stream.records)==3
    task.cancel()
    with pytest.raises(asyncio.CancelledError):await task
