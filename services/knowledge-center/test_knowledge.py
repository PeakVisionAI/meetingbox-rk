import asyncio, importlib.util, json, os, sqlite3, tempfile, unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch

class KnowledgeTests(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();root=Path(self.temp.name)
  os.environ['MEETINGBOX_KNOWLEDGE_ROOT']=str(root)
  os.environ['MEETINGBOX_SOURCE_DB']=str(root/'source.sqlite')
  spec=importlib.util.spec_from_file_location('kb_test_module',Path(__file__).with_name('knowledge_server.py'))
  self.k=importlib.util.module_from_spec(spec);spec.loader.exec_module(self.k)
  with sqlite3.connect(root/'source.sqlite') as c:
   c.executescript('CREATE TABLE meetings(id,title,created_at,folder_path);CREATE TABLE transcripts(id,meeting_id,transcript,speaker,audio_start_time,audio_end_time,duration,timestamp);CREATE TABLE summary_processes(meeting_id,result);')
   c.execute('INSERT INTO meetings VALUES(?,?,?,?)',('m1','测试会议','2026-10-21',str(root)))
   c.executemany('INSERT INTO transcripts VALUES(?,?,?,?,?,?,?,?)',[('t1','m1','讨论会议纪要。','发言人1',0,3,3,'a'),('t2','m1','决定保留标点！','发言人1',4,6,2,'b')])
 def tearDown(self):
  # sqlite connections are finalised before removing the temporary directory.
  import gc;gc.collect();self.temp.cleanup()
 def test_source_remains_read_only(self):
  with self.k.source() as c:
   with self.assertRaises(sqlite3.OperationalError):c.execute("DELETE FROM meetings")
 def test_all_source_chunks_preserved(self):
  rows=[{'id':str(i),'transcript':'字'*1500} for i in range(20)]
  parts=list(self.k.chunks(rows));self.assertEqual(sum(len(x['transcript']) for p in parts for x in p),30000)
 def test_archive_rejects_invalid_source_and_stale_hash_before_embedding(self):
  mid='m1';rows=self.k.detail(mid)['transcripts'];event={'event':'会议决议','source_ids':['unknown']}
  with self.assertRaises(self.k.HTTPException) as ctx:asyncio.run(self.k.save_archive({'meeting_id':mid,'source_hash':self.k.source_hash(rows),'events':[event]}))
  self.assertEqual(ctx.exception.status_code,400)
  with self.assertRaises(self.k.HTTPException) as ctx:asyncio.run(self.k.save_archive({'meeting_id':mid,'source_hash':'stale','events':[event]}))
  self.assertEqual(ctx.exception.status_code,409)
 def test_archive_versions_and_search_keep_only_latest(self):
  rows=self.k.detail('m1')['transcripts'];base={'meeting_id':'m1','source_hash':self.k.source_hash(rows)}
  with patch.object(self.k,'embed',AsyncMock(return_value=[[1.,0.]])):
   for text in ['旧会议纪要结论','新会议纪要结论']:
    asyncio.run(self.k.save_archive({**base,'events':[{'event':text,'source_ids':['t1'],'aliases':['纪要'],'conclusion':'保留标点','speakers':[],'valid_period':'待定'}]}))
   result=asyncio.run(self.k.search({'query':'会议纪要'}))
  self.assertEqual(len(result['results']),1);self.assertEqual(result['results'][0]['revision'],2)
 def test_master_event_groups_meetings_and_preserves_claims(self):
  with sqlite3.connect(self.k.SOURCE) as c:
   c.execute("INSERT INTO meetings VALUES('m2','第二场会议','2026-10-22','')")
   c.execute("INSERT INTO transcripts VALUES('t3','m2','结论变更。','发言人2',0,2,2,'c')")
  with patch.object(self.k,'embed',AsyncMock(return_value=[[1.,0.]])):
   for mid,tid in [('m1','t1'),('m2','t3')]:
    event={'event':'同一项目的新讨论','source_ids':[tid],'aliases':['项目A'],'conclusion':mid,'speakers':[{'speaker':'发言人1','view':mid}]}
    if mid=='m2':event['topic_id']=topic
    asyncio.run(self.k.save_archive({'meeting_id':mid,'events':[event],'source_hash':self.k.source_hash(self.k.detail(mid)['transcripts'])}))
    topic=self.k.event_catalog()[0]['id']
  catalog=self.k.event_catalog();self.assertEqual(len(catalog),1);self.assertEqual(len(catalog[0]['records']),2)
  self.assertEqual({r['conclusion'] for r in catalog[0]['records']},{'m1','m2'})
 def test_legacy_migration_is_idempotent_and_keeps_sources(self):
  with self.k.db() as c:c.execute('INSERT INTO events VALUES(?,?,?,?,?,?,?,?,?,?,?)',('old','m1','旧事件','[]','[]','结论','待定','["t1"]','[]',1,'now'))
  for _ in range(2):
   with self.k.db() as c:self.assertEqual(c.execute('SELECT COUNT(*) FROM event_members WHERE event_id="old"').fetchone()[0],1)
  self.assertEqual(self.k.event_catalog()[0]['records'][0]['source_ids'],['t1'])
 def test_unknown_topic_is_rejected_without_partial_write(self):
  with patch.object(self.k,'embed',AsyncMock(return_value=[[1.,0.]])):
   with self.assertRaises(self.k.HTTPException):asyncio.run(self.k.save_archive({'meeting_id':'m1','source_hash':self.k.source_hash(self.k.detail('m1')['transcripts']),'events':[{'event':'测试','source_ids':['t1'],'topic_id':'missing'}]}))
  with self.k.db() as c:self.assertEqual(c.execute('SELECT COUNT(*) FROM events').fetchone()[0],0)
 def test_relations_carry_evidence_and_invalid_self_relation_rolls_back(self):
  base={'meeting_id':'m1','source_hash':self.k.source_hash(self.k.detail('m1')['transcripts'])}
  with patch.object(self.k,'embed',AsyncMock(return_value=[[1.,0.]])):
   asyncio.run(self.k.save_archive({**base,'events':[{'event':'前置事项','source_ids':['t1']}]}))
   target=self.k.event_catalog()[0]['id']
   asyncio.run(self.k.save_archive({**base,'events':[{'event':'后续事项','source_ids':['t2'],'relations':[{'target_id':target,'kind':'前置依赖'}]}]}))
   current=self.k.event_catalog()[0];self.assertEqual(current['relations'][0]['evidence_id'],current['records'][0]['id'])
   with self.assertRaises(self.k.HTTPException):asyncio.run(self.k.save_archive({**base,'events':[{'event':'无效','topic_id':target,'source_ids':['t1'],'relations':[{'target_id':target,'kind':'关联'}]}]}))
   with self.k.db() as c:self.assertEqual(c.execute('SELECT COUNT(*) FROM events').fetchone()[0],2)
 def test_matching_recommends_without_automatic_attachment(self):
  with patch.object(self.k,'embed',AsyncMock(return_value=[[1.,0.]])):
   asyncio.run(self.k.save_archive({'meeting_id':'m1','source_hash':self.k.source_hash(self.k.detail('m1')['transcripts']),'events':[{'event':'项目规划','source_ids':['t1']}]}))
   result=asyncio.run(self.k.match_events([{'event':'项目规划','aliases':[]}]))
  self.assertEqual(len(result[0]['candidates']),1);self.assertNotIn('topic_id',result[0])
 def test_model_short_references_map_back_to_exact_transcript_ids(self):
  answer=json.dumps({'events':[{'event':'确认纪要保留标点','source_ids':['S2']},{'event':'无依据事项','source_ids':['S99']}]})
  with patch.object(self.k,'infer',AsyncMock(return_value=answer)):
   draft=asyncio.run(self.k.extract_events('m1'))
  self.assertEqual(len(draft['events']),1)
  self.assertEqual(draft['events'][0]['source_ids'],['t2'])
 def test_docx_transcript_preserves_punctuation_and_grouping(self):
  from docx import Document
  result=asyncio.run(self.k.make_document({'meeting_id':'m1','kind':'transcript'}))
  texts=[p.text for p in Document(result['path']).paragraphs]
  self.assertIn('讨论会议纪要。决定保留标点！',texts)
  self.assertEqual(sum(x=='发言人1' for x in texts),1)
  self.assertEqual(self.k.detail('m1')['documents'],[])
  self.assertFalse(self.k.list_meetings({})['meetings'][0]['has_document'])
 def test_native_embedded_speaker_is_normalized_for_voice_and_export(self):
  with sqlite3.connect(self.k.SOURCE) as c:c.execute("UPDATE transcripts SET speaker=NULL,transcript='发言人2：决定保留标点！' WHERE id='t2'")
  row=self.k.detail('m1')['transcripts'][1]
  self.assertEqual(row['speaker'],'发言人2')
  self.assertEqual(row['transcript'],'决定保留标点！')
 def test_transport_requires_secret_and_session(self):
  class Request:
   class Client:host='127.0.0.1'
   client=Client();headers={}
  with self.assertRaises(self.k.HTTPException):self.k.authorize(Request())
  request=Request();request.headers={'X-Knowledge-Token':self.k.TOKEN}
  with self.assertRaises(self.k.HTTPException):self.k.authorize(request)
  self.k.SESSION='test-session';request.headers['X-Knowledge-Session']='test-session';self.k.authorize(request)
 def test_resource_renewal_recovers_after_transient_failures(self):
  import time
  self.k.SESSION='test-session';self.k.LAST_SEEN=time.monotonic()
  sleeps=0
  async def sleep(_):
   nonlocal sleeps
   sleeps+=1
   if sleeps>3:raise asyncio.CancelledError()
  async def prepared(_):self.k.READY=True;self.k.ERROR=None
  async def run():
   with patch.object(self.k.asyncio,'sleep',sleep),patch.object(self.k,'bridge_action',AsyncMock(side_effect=[RuntimeError(),RuntimeError(),{'exclusive':True}])),patch.object(self.k,'prepare',AsyncMock(side_effect=prepared)) as prepare:
    try:await self.k.renew_loop()
    except asyncio.CancelledError:pass
    self.assertEqual(prepare.await_count,1)
  asyncio.run(run());self.assertTrue(self.k.READY);self.assertIsNone(self.k.ERROR)

if __name__=='__main__':unittest.main()
