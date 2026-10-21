"""Evidence-linked 35B final minutes. Raw ASR remains unchanged."""
import asyncio,json,re,time,copy,hashlib
from pathlib import Path
import httpx
from contextlib import asynccontextmanager
import sys
from fastapi import FastAPI,Request,HTTPException
app=FastAPI();lock=asyncio.Lock();MODEL='Qwen3.5-35B-A3B-Q4_K_M';URL='http://127.0.0.1:8067'
PROMPT='''根据编号原文生成简短、正式的中文会议纪要。原文是语音识别结果，破碎不明的内容可以省略。每条内容必须给出依据编号 ids。
准确保留比较时段；疑问、记忆和推测放 questions；营销建议只放 topics；actions 仅限明确要求执行的事项。身份、数量、日期、实施状态未说明时保持缺省。不扩写、不自行解决冲突、不补充背景信息。每条 text 一至两句。
返回 JSON 对象，格式：
{"title":"具体主题", "overview":[{"text":"明确事实或会议范围","ids":[1]}], "topics":[{"title":"议题","points":[{"text":"讨论要点，保留建议语气","ids":[1,2]}]}], "decisions":[{"text":"明确决定","ids":[1]}], "actions":[{"text":"明确要求的任务","ids":[1],"owner":null,"due":null}], "questions":[{"text":"原文提出的未决问题或含混数据","ids":[1]}]}
overview 最多两条，topics 最多八个且每个最多三条。每条仅讲一个独立业务要点。描述既有分类时用客观陈述语气；建议保留建议语气。不自行给建议指派部门。不强求填写行动项。没有明确决定或行动时，数组为空。owner 和 due 缺失用 null。只返回 JSON。'''

def entries_from(messages):
 source='\n'.join(str(m.get('content','')) for m in messages if m.get('role')=='user')
 chunks=re.findall(r'<transcript_chunks>(.*?)</transcript_chunks>',source,re.S)
 if chunks:source='\n'.join(chunks)
 full=re.search(r'<full_transcript>(.*?)</full_transcript>',source,re.S)
 if full:source=full.group(1)
 entries=[]
 for line in source.splitlines():
  line=re.sub(r'^\s*\[\d{1,3}:\d{2}(?::\d{2})?\]\s*','',line).strip()
  if not line or line.startswith('<'):continue
  text=re.sub(r'^(?:发言人|说话人)[^：:\s]*[：:]\s*','',line)
  if text==line and not chunks:continue
  kind='question' if re.search(r'[？?]|我记得|好像|似乎|本月.*上月',text) else 'suggestion' if re.search(r'可以|希望|建议|能不能|是不是',text) else 'source'
  directive=bool(re.search(r'检查一下|看一下|挨个汇报|要检查|由.+负责|负责.+(?:完成|核对|提交)|(?:请|需要|必须|安排|要求).*(?:提交|完成|核对)|截止.*完成',text)) and kind=='source' and '这方面' not in text
  entries.append({'id':len(entries)+1,'text':text,'kind':kind,'directive':directive})
 return entries

def tidy(s):
 s=re.sub(r'嗯[，,、]?|呃[，,、]?','',s)
 s=re.sub(r'其实|呢[，,、]','',s)
 return s.strip('，, ')

def numeric_fact(e):
 return e['kind']=='source' and bool(re.search(r'(?:比|较).*(?:下降|增长|增加|减少|上升).*(?:百分之|\d+\s*%)',e['text']))

def render_grounded(data,entries):
 byid={e['id']:e for e in entries};audit={'dropped':[],'kept':[]};seen=set()
 numeric=[e for e in entries if numeric_fact(e)][:5]
 def linked(item,section):
  if not isinstance(item,dict) or not isinstance(item.get('text'),str):return None
  ids=item.get('ids',[])
  if not isinstance(ids,list) or not ids or any(type(i)!=int or i not in byid for i in ids):return None
  refs=[byid[i] for i in ids];raw=''.join(e['text'] for e in refs);text=item['text'].strip()
  nearby=''.join(byid[k]['text'] for i in ids for k in range(max(1,i-3),min(len(entries),i+3)+1))
  sectors=re.findall(r'(?:建议|希望|针对|对于|由)?([\u4e00-\u9fff]{2,5})(?:部门|公司|业务)',text)
  reason=None
  if not text or len(text)>260:reason='length'
  elif section in ('overview','topics') and any(e['kind']=='question' for e in refs) and not (section=='topics' and re.search(r'建议|提议|询问|待确认|不确定',text)):reason='uncertain-source'
  elif section in ('overview','topics') and any(n['id'] in ids for n in numeric) and re.search(r'百分|\d+\s*%',text):reason='numeric-fact-rendered-verbatim'
  elif any(w in text and w not in raw for w in ('同比','去年同期','环比','专柜','租赁方','年轻女性顾客')):reason='unsupported-specificity'
  elif sectors and any(word not in nearby for word in sectors):reason='unsupported-business-assignment'
  elif section=='actions' and (not any(e['directive'] for e in refs) or any(w in text and w not in raw for w in ('制定','摘除','评估','策划','落实','提交','新增','建立'))):reason='unsupported-action'
  elif section=='decisions' and not re.search(r'决定|同意|达成一致|确定执行',raw):reason='no-explicit-decision'
  elif section=='questions' and not any(e['kind']=='question' or len(e['text'])<16 for e in refs):reason='not-an-open-question'
  elif section in ('overview','topics') and '月' in text and not any('月' in e['text'] and e['kind']=='source' for e in refs):reason='unconfirmed-month'
  if reason:audit['dropped'].append({'section':section,'text':text,'ids':ids,'reason':reason});return None
  key=re.sub(r'\W','',text)
  if key in seen:return None
  seen.add(key);audit['kept'].append({'section':section,'text':text,'ids':ids});return text
 title=re.sub(r'[\n#]','',str(data.get('title','会议纪要'))).strip()[:60]
 out=['# '+title,'','## 核心结论','']
 # Exact acoustic facts take precedence over a model's comparison rewrite.
 out.extend(tidy(e['text']) for e in numeric)
 overview=[x for i in data.get('overview',[])[:2] if (x:=linked(i,'overview'))]
 out.extend(overview or ([] if numeric else ['会议主要讨论内容如下。']))
 out+=['','## 议题讨论','']
 for topic in data.get('topics',[])[:8]:
  points=[x for i in topic.get('points',[])[:3] if (x:=linked(i,'topics'))]
  if points:out+=['### '+re.sub(r'[\n#]','',str(topic.get('title','讨论要点')))[:35],'']+points+['']
 out+=['## 决策事项','']
 decisions=[x for i in data.get('decisions',[]) if (x:=linked(i,'decisions'))]
 out+=['- '+x for x in decisions] or ['本次未形成明确决策。']
 out+=['','## 行动项',''];actions=[]
 for item in data.get('actions',[]):
  text=linked(item,'actions')
  if not text:continue
  raw=''.join(byid[i]['text'] for i in item['ids'])
  owner=item.get('owner');due=item.get('due')
  owner=owner if isinstance(owner,str) and owner in raw and re.search(r'负责|由.+执行|安排',raw) else '未明确'
  due=due if isinstance(due,str) and due in raw and re.search(r'截止|之前|前完成|限期',raw) else '未明确'
  actions.append('| '+text.replace('|','／')+' | '+owner+' | '+due+' |')
 out+=['| 事项 | 负责人 | 完成时间 |','| --- | --- | --- |']+actions if actions else ['本次未明确行动项。']
 out+=['','## 待确认事项','']
 questions=[x for i in data.get('questions',[])[:8] if (x:=linked(i,'questions'))]
 out+=['- '+x for x in questions] or ['本次未明确其他待确认事项。']
 result='\n'.join(out).strip()
 if re.search(r'\b\d{1,3}:\d{2}\b',result):raise ValueError('Unexpected timestamp')
 return result,audit

def source_number_text(text,source):
 # Keep ASR number surfaces, including spoken Chinese dates, in incremental notes.
 digits={'零':'0','〇':'0','一':'1','二':'2','两':'2','三':'3','四':'4','五':'5','六':'6','七':'7','八':'8','九':'9','幺':'1'}
 def number(token):
  from decimal import Decimal
  if '点' in token:
   left,right=token.split('点',1);suffix=''
   while right and right[-1] in '十百千万亿':suffix=right[-1]+suffix;right=right[:-1]
   if not right or any(c not in digits for c in right):raise ValueError('invalid decimal')
   return (number(left)+Decimal('0.'+''.join(digits[c] for c in right)))*(number(suffix) if suffix else 1)
  if all(c in digits for c in token):return Decimal(''.join(digits[c] for c in token))
  total=section=current=0
  for char in token:
   if char in digits:current=int(digits[char])
   elif char in '十百千':section+=(current or 1)*{'十':10,'百':100,'千':1000}[char];current=0
   elif char=='万':total+=(section+current or 1)*10000;section=current=0
   elif char=='亿':total=(total+section+current or 1)*100000000;section=current=0
  return Decimal(total+section+current)
 from decimal import Decimal,InvalidOperation
 surfaces={}
 for token in re.findall(r'[零〇一二两三四五六七八九幺十百千万亿点]+',source):
  for end in range(1,len(token)+1):
   try:surfaces.setdefault(number(token[:end]),token[:end])
   except (ValueError,InvalidOperation):pass
 literal=set(re.findall(r'\d+(?:\.\d+)?',source));unsupported=False
 def replace(match):
  nonlocal unsupported
  token=match.group()
  if token in literal:return token
  surface=surfaces.get(Decimal(token))
  if surface:return surface
  unsupported=True;return token
 result=re.sub(r'\d+(?:\.\d+)?',replace,text)
 return None if unsupported else result

def normalized(text):
 return ''.join(c for c in text if c.isalnum()).lower()

def session_path(session):
 if not re.fullmatch(r'[0-9]{1,20}',str(session)):raise ValueError('Invalid session')
 root=Path(__file__).with_name('incremental-35b');root.mkdir(exist_ok=True)
 return root/('session-'+str(session)+'.json')

def reusable_partials(messages,entries):
 source='\n'.join(str(m.get('content','')) for m in messages if m.get('role')=='user')
 match=re.search(r'<incremental_summary>\s*(.*?)\s*</incremental_summary>',source,re.S)
 if not match:return [],0
 try:
  state=json.loads(match.group(1));path=session_path(state.get('grounded_session',''))
  saved=json.loads(path.read_text(encoding='utf8'));cursor=0;partials=[]
  spans=[];position=0
  for entry in entries:
   text=normalized(entry['text']);spans.append((position,position+len(text),entry['id']));position+=len(text)
  full=''.join(normalized(e['text']) for e in entries)
  for expected,item in enumerate(saved.get('intervals',[])):
   if item['index']!=expected:break
   old=item['entries'];chunk=''.join(normalized(e['text']) for e in old)
   if not chunk or full[cursor:cursor+len(chunk)]!=chunk:break
   mapping={};offset=cursor
   for entry in old:
    end=offset+len(normalized(entry['text']))
    mapping[entry['id']]=[eid for start,stop,eid in spans if start<end and stop>offset]
    offset=end
   def remap(value):
    if isinstance(value,list):return [remap(x) for x in value]
    if isinstance(value,dict):return {k:(sorted({eid for i in v for eid in mapping[i]}) if k=='ids' else remap(v)) for k,v in value.items()}
    return value
   partials.append(remap(item['candidate']));cursor+=len(chunk)
  covered=sum(stop<=cursor for start,stop,eid in spans)
  return partials,covered
 except (ValueError,KeyError,TypeError,OSError):return [],0

async def bounded_infer(client,value,prompt,max_tokens=1600):
 encoded=json.dumps(value,ensure_ascii=False)
 tokenized=await client.post(URL+'/tokenize',json={'content':prompt+'\n'+encoded});tokenized.raise_for_status()
 if len(tokenized.json()['tokens'])>4900:raise ValueError('Incremental input exceeds bounded token budget')
 response=await client.post(URL+'/v1/chat/completions',json={'model':MODEL,'messages':[{'role':'system','content':prompt},{'role':'user','content':encoded}],'response_format':{'type':'json_object'},'max_tokens':max_tokens,'temperature':0.1,'top_p':0.8,'seed':42,'chat_template_kwargs':{'enable_thinking':False}})
 response.raise_for_status();payload=response.json()
 if payload['choices'][0].get('finish_reason')=='length':raise ValueError('Truncated incremental output')
 return json.loads(payload['choices'][0]['message']['content']),payload

@app.post('/api/incremental')
async def incremental(request:Request):
 if request.client.host not in ('127.0.0.1','192.168.44.50'):raise HTTPException(403,'Local backend only')
 body=await request.json();started=time.perf_counter()
 try:
  session=str(body['meeting_id']);index=int(body['interval_index'])
  if session=='0':
   session=str((body.get('previous_context') or {}).get('grounded_session') or time.time_ns())
   if session=='0':session=str(time.time_ns())
  path=session_path(session)
  if index<0:raise ValueError('Invalid interval')
  entries=[{'id':i+1,'text':str(row.get('text','')).strip()} for i,row in enumerate([r for r in body['segments'] if str(r.get('text','')).strip()])]
  if not entries or len(json.dumps(entries,ensure_ascii=False))>24000:raise ValueError('Invalid source')
  previous=body.get('previous_context') or {}
  async with lock:
   async with httpx.AsyncClient(timeout=210) as client:
    prompt=PROMPT+' 这是会议的一个新时段：最多三个议题，每题最多两条，概述最多一条。全部文字简短。已有概括仅供理解上下文，每项 ids 必须来自新增原文，不将旧内容当作新证据。'
    candidate,payload=await bounded_infer(client,{'previous_context':previous.get('context_summary','')[:240],'new_source':entries},prompt)
   allowed={e['id'] for e in entries}
   raw=''.join(e['text'] for e in entries)
   def prune(value):
    if isinstance(value,list):return [y for x in value if (y:=prune(x)) is not None]
    if isinstance(value,dict):
     if 'title' in value:
      value={**value,'title':source_number_text(str(value['title']),raw) or '讨论要点'}
     if 'text' in value:
      text=source_number_text(str(value['text']),raw)
      if text is None:return None
      value={**value,'text':text}
     if 'ids' in value and (not isinstance(value['ids'],list) or not value['ids'] or any(type(i)!=int or i not in allowed for i in value['ids'])):return None
     return {k:prune(v) for k,v in value.items()}
    return value
   candidate=prune(candidate)
   points=[i['text'] for t in candidate.get('topics',[]) for i in t.get('points',[]) if isinstance(i,dict) and isinstance(i.get('text'),str)]
   overview=[i['text'] for i in candidate.get('overview',[]) if isinstance(i,dict) and isinstance(i.get('text'),str)]
   brief='；'.join(overview[:1]+points[:2])[:110]
   if not brief:raise ValueError('Empty incremental summary')
   def texts(key):return [i['text'] for i in candidate.get(key,[]) if isinstance(i,dict) and isinstance(i.get('text'),str)]
   raw=''.join(e['text'] for e in entries)
   decisions=[i['text'] for i in candidate.get('decisions',[]) if re.search(r'决定|同意|达成一致|确定执行',''.join(entries[j-1]['text'] for j in i.get('ids',[])))]
   todos=[i['text'] for i in candidate.get('actions',[]) if re.search(r'检查一下|看一下|挨个汇报|要检查|由.+负责|负责.+(?:完成|核对|提交)|(?:请|需要|必须|安排|要求).*(?:提交|完成|核对)|截止.*完成',''.join(entries[j-1]['text'] for j in i.get('ids',[])))]
   state={'title':str(candidate.get('title','会议'))[:24],'summary':brief,'context_summary':((previous.get('context_summary','')+'；') if previous.get('context_summary') else '')+brief,'agenda':[str(t.get('title','')) for t in candidate.get('topics',[])],'discussion':points[:3],'decisions':decisions,'todos':todos,'risks':[],'open_questions':texts('questions')[:3],'evidence':[],'grounded_session':session}
   state['context_summary']=state['context_summary'][-220:]
   saved=json.loads(path.read_text(encoding='utf8')) if path.exists() else {'session':session,'intervals':[]}
   saved['intervals']=[item for item in saved['intervals'] if item['index']!=index]+[{'index':index,'entries':entries,'candidate':candidate,'seconds':time.perf_counter()-started,'usage':payload.get('usage',{})}]
   saved['intervals'].sort(key=lambda item:item['index'])
   temporary=path.with_suffix('.tmp');temporary.write_text(json.dumps(saved,ensure_ascii=False,indent=2),encoding='utf8');temporary.replace(path)
  seconds=time.perf_counter()-started
  print(json.dumps({'kind':'incremental','session':session,'index':index,'seconds':seconds,'model':MODEL}),flush=True)
  return {'structure':state,'seconds':seconds,'model':MODEL}
 except (httpx.HTTPError,ValueError,KeyError,TypeError,OSError) as error:
  print(repr(error),flush=True);raise HTTPException(502,'Incremental summary unavailable; transcript retained') from error

@app.get('/health')
async def health():
 ready=False
 try:
  async with httpx.AsyncClient(timeout=3) as c:ready=(await c.get(URL+'/health')).status_code==200
 except httpx.HTTPError:pass
 return {'ready':ready,'summary_enabled':True,'model':MODEL,'device':'Vulkan GPU','thinking':False,'summarization':'formal-meeting-minutes-v5-incremental-reuse','grounding':'source-linked-items'}


@asynccontextmanager
async def final_speech_lease():
 async with httpx.AsyncClient(timeout=10) as client:
  acquired=await client.post('http://192.168.44.50:8020/runtime/final-summary/acquire')
  if acquired.status_code==409:raise HTTPException(409,'语音处理尚未结束，请稍后再生成纪要。')
  acquired.raise_for_status()
  try:
   if sys.platform=='win32':
    proc=await asyncio.create_subprocess_exec('powershell.exe','-NoProfile','-WindowStyle','Hidden','-ExecutionPolicy','Bypass','-File',str(Path(__file__).with_name('Prepare-FinalSummary.ps1')),stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE,creationflags=0x08000000)
    out,err=await asyncio.wait_for(proc.communicate(),240)
    if proc.returncode:
     print('Resource precheck: '+err.decode(errors='replace')[-600:],flush=True)
     raise HTTPException(503,'生成纪要前的资源预检未通过：请等待语音处理结束，并检查其他大显存程序和剩余内存。')
    Path(__file__).with_name('final-resource-proof.json').write_bytes(out)
   yield
  finally:
   await client.post('http://192.168.44.50:8020/runtime/final-summary/release')

@app.post('/api/chat')
async def chat(request:Request):
 if request.client.host not in ('127.0.0.1','192.168.44.50'):raise HTTPException(403,'Local backend only')
 body=await request.json();messages=body.get('messages',[])
 if not any('<template>' in str(m.get('content','')) for m in messages):raise HTTPException(400,'35B is reserved for final minutes')
 entries=entries_from(messages)
 if not entries or len(json.dumps(entries,ensure_ascii=False))>600000:raise HTTPException(400,'Invalid transcript')
 started=time.perf_counter()
 try:
  async with lock, final_speech_lease():
   async with httpx.AsyncClient(timeout=600) as c:
    async def infer(value,prompt=PROMPT):
     encoded=json.dumps(value,ensure_ascii=False)
     tokens=await c.post(URL+'/tokenize',json={'content':prompt+'\n'+encoded})
     tokens.raise_for_status();count=len(tokens.json()['tokens'])
     if count>4900:raise ValueError('Summary input exceeds bounded token budget')
     response=await c.post(URL+'/v1/chat/completions',json={'model':MODEL,'messages':[{'role':'system','content':prompt},{'role':'user','content':json.dumps(value,ensure_ascii=False)}],'response_format':{'type':'json_object'},'max_tokens':3000,'temperature':0.1,'top_p':0.8,'seed':42,'chat_template_kwargs':{'enable_thinking':False}})
     response.raise_for_status();result=response.json()
     if result['choices'][0].get('finish_reason')=='length':raise ValueError('Truncated output')
     return json.loads(result['choices'][0]['message']['content']),result
    reused,covered=reusable_partials(messages,entries)
    groups=[];group=[];size=0
    for entry in entries[covered:]:
     n=len(json.dumps(entry,ensure_ascii=False))
     if group and size+n>3500:groups.append(group);group=[];size=0
     group.append(entry);size+=n
    if group:groups.append(group)
    partials=copy.deepcopy(reused);total_usage={'completion_tokens':0,'prompt_tokens':0}
    checkpoint=Path(__file__).with_name('summary-checkpoint.json')
    saved=json.loads(checkpoint.read_text(encoding='utf8')) if checkpoint.exists() else {}
    cached=saved.get('partials',[]) if saved.get('entries')==entries and saved.get('covered',0)==covered else []
    for group_index,group in enumerate(groups):
     index=group_index+len(reused)
     if index<len(cached):
      partials.append(cached[index]);continue
     data,payload=await infer(group,PROMPT+' 这是整场的一小段：最多三个议题，每个最多两条要点；概述最多一条。优先保留本段独有信息，文本简洁。')
     # Every candidate keeps original global evidence IDs through all reduction levels.
     def prune(obj):
      if isinstance(obj,list):return [y for x in obj if (y:=prune(x)) is not None]
      if isinstance(obj,dict):
       if 'ids' in obj and (not obj['ids'] or any(type(i)!=int or i not in {e['id'] for e in group} for i in obj['ids'])):return None
       return {k:prune(v) for k,v in obj.items()}
      return obj
     partials.append(prune(data))
     for k in total_usage:total_usage[k]+=payload.get('usage',{}).get(k,0)
     Path(__file__).with_name('summary-progress.json').write_text(json.dumps({'stage':'分段归纳','completed':index+1,'total':len(groups)+len(reused)},ensure_ascii=False),encoding='utf8')
     Path(__file__).with_name('summary-checkpoint.json').write_text(json.dumps({'entries':entries,'partials':partials,'covered':covered},ensure_ascii=False),encoding='utf8')
    reduce_prompt=PROMPT+' 输入为同一会议按顺序生成的分段候选，请合并相近议题、消除重复，覆盖前中后段关键内容。保留原始依据 ids，禁止生成新编号。提议、问题与决定分开。后来明确更正的信息优先；冲突无法消解则列待确认。标题概括整场会议。'
    while len(partials)>1:
     batches=[];batch=[];size=0
     for part in partials:
      n=len(json.dumps(part,ensure_ascii=False))
      if batch and size+n>5000:batches.append(batch);batch=[];size=0
      batch.append(part);size+=n
     if batch:batches.append(batch)
     if all(len(b)==1 for b in batches):raise ValueError('Reduction candidates exceed context budget')
     merged=[]
     for batch in batches:
      Path(__file__).with_name('summary-progress.json').write_text(json.dumps({'stage':'合并议题与核对依据','completed':len(merged),'total':len(batches)},ensure_ascii=False),encoding='utf8')
      if len(batch)==1:
       merged.append(batch[0]);continue
      instruction=reduce_prompt if len(batches)==1 else reduce_prompt+' 这是中间层合并稿：最多四个议题，每题最多两条，一条一句；概述最多一条，全部文字控制在800个汉字以内，为下一轮整合保留空间。'
      data,payload=await infer(batch,instruction);merged.append(data)
      for k in total_usage:total_usage[k]+=payload.get('usage',{}).get(k,0)
     partials=merged
     Path(__file__).with_name('summary-reduction-checkpoint.json').write_text(json.dumps({'entries':entries,'partials':partials,'covered':covered},ensure_ascii=False),encoding='utf8')
    data=partials[0]
    evidence_audit=[];byid={e['id']:e for e in entries}
    all_items=[x for x in data.get('overview',[])]+[x for topic in data.get('topics',[]) for x in topic.get('points',[])]
    audit_prompt='你是会议纪要事实核查员。逐项核对候选与所附原文，只修正语义范围、主语、时间、单位、提议和既定事实混淆，不新增外部知识。不同技术的领先程度分开表述。原文中的口语化技术单位含混时，保留可确定的定性内容，省去无法确定的参数。每个 item 按索引返回 text；没有足够依据则 text=null。禁止把计划或设想写成已实施。输出 JSON {"items":[{"index":0,"text":"核查后文字"}]}。'
    for offset in range(0,len(all_items),4):
     batch=all_items[offset:offset+4];evidence=[]
     for k,item in enumerate(batch):
      ids=sorted({n for i in item.get('ids',[]) for n in range(max(1,i-1),min(len(entries),i+1)+1)})
      evidence.append({'index':k,'candidate':item['text'],'source':[{'id':n,'text':byid[n]['text']} for n in ids]})
     checked,payload=await infer(evidence,audit_prompt)
     for result in checked.get('items',[]):
      k=result.get('index')
      if type(k)==int and 0<=k<len(batch):
       evidence_audit.append({'before':batch[k]['text'],'after':result.get('text')})
       batch[k]['text']=result.get('text') or ''
     Path(__file__).with_name('summary-progress.json').write_text(json.dumps({'stage':'核查概括范围与原文依据','completed':min(offset+4,len(all_items)),'total':len(all_items)},ensure_ascii=False),encoding='utf8')
    open_questions=[]
    for index,item in enumerate(data.get('questions',[])):
     first=min(item.get('ids') or [1]);last=min(len(entries),first+85)
     excerpt=[{'id':n,'text':byid[n]['text']} for n in range(max(1,first-2),last+1)]
     check,payload=await infer({'question':item['text'],'following_discussion':excerpt},'检查会议问题是否在随后的讨论中得到回答。已回答、已形成解释或只是主持人引导话题的问题，从待确认事项中删除。仅在确实仍有待解决内容时保留，拿不准也删除。只返回 JSON {"keep":false,"reason":"简短依据"}，keep 为布尔值。不得引入外部知识。')
     if check.get('keep') is True:open_questions.append(item)
     evidence_audit.append({'question':item['text'],**check})
     Path(__file__).with_name('summary-progress.json').write_text(json.dumps({'stage':'检查问题是否已回答','completed':index+1,'total':len(data.get('questions',[]))},ensure_ascii=False),encoding='utf8')
    data['questions']=open_questions
    text,audit=render_grounded(data,entries);audit['semantic_checks']=evidence_audit
    audit['reused_intervals']=len(reused);audit['covered_entries']=covered;audit['tail_entries']=len(entries)-covered
    payload['usage']=total_usage
    Path(__file__).with_name('summary-progress.json').write_text(json.dumps({'stage':'完成','completed':1,'total':1},ensure_ascii=False),encoding='utf8')
  Path(__file__).with_name('last-summary-attempt.json').write_text(json.dumps({'raw':data,'audit':audit,'content':text,'entries':entries},ensure_ascii=False,indent=2),encoding='utf8')
  usage=payload.get('usage',{});seconds=time.perf_counter()-started
  print(json.dumps({'model':MODEL,'seconds':seconds,'usage':usage,'grounding':'source-linked-items','dropped':len(audit['dropped'])}),flush=True)
  return {'message':{'role':'assistant','content':text},'done':True,'done_reason':'stop','model':MODEL,'total_duration':int(seconds*1e9),'eval_count':usage.get('completion_tokens',0),'prompt_eval_count':usage.get('prompt_tokens',0)}
 except (httpx.HTTPError,ValueError,KeyError,TypeError) as e:
  print(repr(e),flush=True);raise HTTPException(502,'35B minutes unavailable; existing report retained') from e
