"""Intel GPU final-answer-only local summarizer, Ollama-compatible endpoint."""
import os,time,json,threading,asyncio,traceback,re
from pathlib import Path
import openvino_genai as g
from transformers import AutoTokenizer
from fastapi import FastAPI,Request,HTTPException
from summary_memory import standard_request,memory_messages,validate_state,render_state
from grounded_report import report_request,render_report,admissible,SECTIONS
app=FastAPI();pipe=tokenizer=None;lock=threading.Lock();model='Qwen3-8B-int4'
DISABLED=os.environ.get('SUMMARY_DISABLED','0')=='1'
SOURCE_ONLY=os.environ.get('SUMMARY_SOURCE_ONLY','0')=='1'
GROUNDING_RULES='''
补充事实约束：
- “建议、考虑、可以、看看”一律作为讨论建议；仅明确同意、决定或要求执行的事项才进入关键决策和行动项。
- 不根据发言内容给发言人补充“招生负责人”等身份；姓名或职责未明确指定时，负责人写“未指定”。
- 状态、截止时间、影响范围未明确说出时写“未说明”；禁止默认“未开始”或编造实施安排。
- 风险与阻塞、遗留问题仅收录原文明确提出的内容；禁止补充课程、人员、信任度等常见议题。
- 关键决策与行动项保留已有时间依据；只能依据原文明确事项。分析、观点与建议可归纳，必须保持为分析或建议。
- 保留建议与已达成决定的区别。不要把讨论者自动指定为执行负责人。
- 短会议各节简短填写，避免逐句复述、背景扩写和重复。
- 自然概括主题、观点变化、分歧与结论，不照抄逐字稿。每节一至两句或少量条目，总结尽量简短。
- 只输出最终内容，不输出思考过程、阅读步骤或模板填写说明。已有增量摘要是工作记忆，结合待办与剩余原文统一归纳。
'''

def ground_messages(messages):
 if any(m.get('role')=='system' and '<template>' in str(m.get('content','')) for m in messages) and any('<transcript_chunks>' in str(m.get('content','')) for m in messages):
  return [{**m,'content':m['content']+GROUNDING_RULES} if m.get('role')=='system' else dict(m) for m in messages]
 return messages
@app.on_event('startup')
def startup():
 global pipe,tokenizer
 if DISABLED or SOURCE_ONLY:
  print(json.dumps({'ready':True,'summary_enabled':SOURCE_ONLY,'device':'none'}),flush=True)
  return
 root=os.environ['SUMMARY_MODEL_DIR'];tokenizer=AutoTokenizer.from_pretrained(root,local_files_only=True,fix_mistral_regex=True)
 # This Arc driver fails to restore some oneDNN kernels from disk cache.
 # Uncached compilation passed both full-prompt and repeated-request tests.
 pipe=g.LLMPipeline(root,'GPU')
 print(json.dumps({'ready':True,'model':model,'device':'GPU'}),flush=True)
@app.get('/health')
def health():return {'ready':DISABLED or SOURCE_ONLY or pipe is not None,'summary_enabled':SOURCE_ONLY or not DISABLED,'model':model,'device':'none' if DISABLED or SOURCE_ONLY else 'GPU','thinking':False,'summarization':'source-paragraphs' if SOURCE_ONLY else 'disabled-for-diagnostics' if DISABLED else 'incremental-abstractive-v1'}
@app.post('/api/chat')
async def chat(request:Request):
 if DISABLED and not SOURCE_ONLY:raise HTTPException(503,'会议纪要已临时关闭，当前仅运行录音与转写。')
 if request.client.host not in ('127.0.0.1','192.168.44.50'):raise HTTPException(403,'Local meeting backend only')
 body=await request.json();messages=body.get('messages',[])
 if not messages or len(json.dumps(messages))>200000:raise HTTPException(400,'Invalid messages')
 if SOURCE_ONLY:
  parsed=report_request(messages)
  if not parsed:raise HTTPException(400,'原文整理模式需要带时间戳的逐字稿及标准会议模板。')
  entries,_=parsed
  selection={}
  for section in SECTIONS:
   valid=[i+1 for i,e in enumerate(entries) if admissible(section,e['text']) and len(re.findall(r'[\u4e00-\u9fff]',re.sub(r'^.*?[：:]','',e['text'])))>=16]
   ranked=sorted(valid,key=lambda i:(len(set(re.findall(r'招生|安全|饮食|健康|老师|教育|孩子|宣传|幼儿园',entries[i-1]['text'])))+min(len(entries[i-1]['text'])/100,2)),reverse=True)
   chosen=[];seen=set()
   for i in ranked:
    key=re.sub(r'[\W_]+','',re.sub(r'^.*?[：:]','',entries[i-1]['text']))
    if key in seen:continue
    seen.add(key);chosen.append(i)
    if len(chosen)==3:break
   selection[section]=sorted(chosen)
  text=render_report(entries,selection)
  text+='\n\n## 原文分段\n\n'+'\n\n'.join(f"**[{e['timestamp']}]** {e['text']}" for e in entries)
  print(json.dumps({'time':time.time(),'mode':'source-paragraphs','source_entries':len(entries),'output_chars':len(text)}),flush=True)
  return {'message':{'role':'assistant','content':text},'done':True,'done_reason':'stop','model':'source-paragraphs','eval_count':0,'prompt_eval_count':0,'total_duration':0}
 messages=ground_messages(messages)
 report=standard_request(messages)
 budget=max(256,min(int(body.get('options',{}).get('num_predict',1200)),1800))
 def infer():
  previous,source=report if report else ({},'')
  if report and not source.strip() and previous.get('context_summary'):
   text=render_state(previous)
   return {'message':{'role':'assistant','content':text},'done':True,'done_reason':'stop','model':model,'total_duration':0,'eval_count':0,'prompt_eval_count':0,'grounding':'reused-incremental-memory'}
  prompt=tokenizer.apply_chat_template(memory_messages(previous,source) if report else messages,tokenize=False,add_generation_prompt=True,enable_thinking=False)
  t=time.perf_counter()
  with lock:result=pipe.generate(prompt,apply_chat_template=False,max_new_tokens=budget,do_sample=False,repetition_penalty=1.05)
  text=(result if isinstance(result,str) else str(result.texts[0])).strip()
  if '<think>' in text: text=text.split('</think>',1)[-1].strip() if '</think>' in text else ''
  generated_count=len(tokenizer.encode(text,add_special_tokens=False))
  if report:
   state=json.loads(text.removeprefix('```json').removesuffix('```').strip())
   text=render_state(validate_state(state,previous,source))
  source='\n'.join(str(m.get('content','')) for m in messages)
  dates=re.findall(r'\d{1,4}年|\d{1,2}月(?:\d{1,2}日)?',text)
  if any(date not in source for date in dates):raise RuntimeError('Unsupported calendar date; original retained')
  if body.get('format')=='json':
   # Do not publish invalid JSON corrections.
   text=text.removeprefix('```json').removesuffix('```').strip();json.loads(text)
  seconds=time.perf_counter()-t
  print(json.dumps({'seconds':round(seconds,3),'output_chars':len(text),'model':model}),flush=True)
  count=len(tokenizer.encode(text,add_special_tokens=False))
  return {'message':{'role':'assistant','content':text},'done':True,'done_reason':'length' if generated_count>=budget else 'stop','model':model,'total_duration':int(seconds*1e9),'eval_count':generated_count,'prompt_eval_count':len(tokenizer.encode(prompt,add_special_tokens=False)), 'grounding':'source-constrained-abstractive'}
 try:return await asyncio.to_thread(infer)
 except Exception as e:
  traceback.print_exc()
  raise HTTPException(502,'Local summary unavailable') from e
