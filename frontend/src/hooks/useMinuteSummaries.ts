import { useCallback, useEffect, useRef, useState } from 'react';
import type { Transcript } from '@/types';
import { minuteSummaryService, type MinuteSummaryRecord } from '@/services/minuteSummaryService';
import { SUMMARY_INTERVAL_SECONDS, intervalSegments, sourceText, nextSummaryBoundary, endOf } from '@/lib/incrementalSummary';
export { SUMMARY_INTERVAL_SECONDS } from '@/lib/incrementalSummary';
export const LIVE_SUMMARY_ENABLED=true;
const VERSION='adaptive-five-v1';
const CACHE='pending_minute_summaries';
const VERSION_KEY='pending_minute_summaries_version';
const SESSION_KEY='pending_summary_session_token';
let queue: Promise<void>=Promise.resolve();
const snapshots=new Map<string,MinuteSummaryRecord[]>();
const saveTargets=new Map<string,string>();
export function attachMinuteSummaryMeeting(meetingId:string) {
  const token=sessionStorage.getItem(SESSION_KEY);
  if(token){saveTargets.set(token,meetingId);snapshots.set(token,cachedMinuteSummaries());}
  return token;
}
export function cachedMinuteSummaries(): MinuteSummaryRecord[] {
  if(sessionStorage.getItem(VERSION_KEY)!==VERSION)return [];
  try { const data=JSON.parse(sessionStorage.getItem(CACHE)||'[]');return Array.isArray(data)?data:[]; } catch {return [];}
}
function cache(item: MinuteSummaryRecord) {
  const next=[...cachedMinuteSummaries().filter(t=>t.minute_index!==item.minute_index),item].sort((a,b)=>a.minute_index-b.minute_index);
  sessionStorage.setItem(CACHE,JSON.stringify(next));return next;
}
function serial<T>(task:()=>Promise<T>):Promise<T> {const p=queue.then(task,task);queue=p.then(()=>undefined,()=>undefined);return p;}
async function record(transcripts:Transcript[],templateId:string,index:number,start:number,end:number,previous:Record<string,unknown>={}):Promise<MinuteSummaryRecord> {
  const segments=intervalSegments(transcripts,start,end);const source=sourceText(transcripts,start,end);
  if(!source)return {minute_index:index,start_seconds:start,end_seconds:end,summary:'',status:'empty',source_text:source};
  try {
    const result=await minuteSummaryService.generate(templateId,index,segments,previous);
    return {minute_index:index,start_seconds:start,end_seconds:end,summary:result.summary,structure:result.structure,source_text:source,status:'completed'};
  } catch(error) {
    console.warn('分时摘要暂不可用',error);
    return {minute_index:index,start_seconds:start,end_seconds:end,summary:'本段摘要暂未生成，原文已保留。',source_text:source,status:'error'};
  }
}
export function generateMinuteSummaryInterval(transcripts:Transcript[],templateId:string,index:number,endSeconds?:number,startSeconds?:number) {
  const token=sessionStorage.getItem(SESSION_KEY);
  const ready=cachedMinuteSummaries();
  if(token && !snapshots.has(token))snapshots.set(token,ready);
  const previous=cachedMinuteSummaries().filter(t=>t.status==='completed').at(-1)?.structure||{};
  return serial(async()=>{
    const start=startSeconds ?? (index===0?0:cachedMinuteSummaries().find(t=>t.minute_index===index-1)?.end_seconds ?? index*SUMMARY_INTERVAL_SECONDS);
    const end=endSeconds ?? nextSummaryBoundary(transcripts,start,finalTranscriptEndSeconds(transcripts),true);
    if(end===null)throw new Error('等待段落结束');
    const result=await record(transcripts,templateId,index,start,end,previous);
    if(token){
      const next=[...(snapshots.get(token)||[]).filter(t=>t.minute_index!==result.minute_index),result].sort((a,b)=>a.minute_index-b.minute_index);
      snapshots.set(token,next);
      const meeting=saveTargets.get(token);
      if(meeting)await minuteSummaryService.save(meeting,next).catch(error=>console.warn('分时摘要稍后重试保存',error));
    }
    if(token===sessionStorage.getItem(SESSION_KEY))cache(result);return result;
  });
}
export function finalTranscriptEndSeconds(transcripts:Transcript[]) {
  return Math.ceil(transcripts.filter(t=>!t.is_partial && t.text.trim()).reduce((end,t)=>Math.max(end,endOf(t)),0));
}
export function finalizeMinuteSummaries(transcripts:Transcript[],templateId:string,snapshot=cachedMinuteSummaries(),token:string|null=null) {
  return serial(async()=>{
    // Completed intervals are retained. The final report consumes the raw tail.
    if(!LIVE_SUMMARY_ENABLED)return [];
    const result:MinuteSummaryRecord[]=[];
    for(const old of (token ? snapshots.get(token) || snapshot : snapshot)) {
      const same=old.source_text===sourceText(transcripts,old.start_seconds,old.end_seconds);
      result.push(old.status==='completed' && same ? old : await record(transcripts,templateId,old.minute_index,old.start_seconds,old.end_seconds,result.at(-1)?.structure||{}));
    }
    if(token){snapshots.delete(token);saveTargets.delete(token);}return result;
  });
}
export function formatMinuteRange(start:number,end:number) {
  const f=(s:number)=>`${String(Math.floor(s/60)).padStart(2,'0')}:${String(Math.floor(s%60)).padStart(2,'0')}`;
  return `${f(start)}–${f(end)}`;
}
export function useMinuteSummaries({activeDuration,isRecording,transcripts,templateId}:{activeDuration:number|null;isRecording:boolean;transcripts:Transcript[];templateId:string}) {
  const [summaries,setSummaries]=useState<MinuteSummaryRecord[]>([]);
  const [generatingMinute,setGeneratingMinute]=useState<number|null>(null);
  const processing=useRef(false);const cursor=useRef(0);const index=useRef(0);
  const append=useCallback((item:MinuteSummaryRecord)=>setSummaries(current=>[...current.filter(t=>t.minute_index!==item.minute_index),item].sort((a,b)=>a.minute_index-b.minute_index)),[]);
  useEffect(()=>{
    if(!LIVE_SUMMARY_ENABLED || !isRecording){setSummaries([]);return;}
    if(sessionStorage.getItem(VERSION_KEY)!==VERSION){sessionStorage.removeItem(CACHE);sessionStorage.setItem(VERSION_KEY,VERSION);}
    if(!sessionStorage.getItem(SESSION_KEY)){sessionStorage.removeItem(CACHE);sessionStorage.setItem(SESSION_KEY,crypto.randomUUID());}
    const restored=cachedMinuteSummaries();setSummaries(restored);cursor.current=restored.at(-1)?.end_seconds||0;index.current=restored.length;
  },[isRecording]);
  useEffect(()=>{
    if(!LIVE_SUMMARY_ENABLED || !isRecording || processing.current)return;
    const boundary=nextSummaryBoundary(transcripts,cursor.current,Math.max(0,(activeDuration||0)-5));
    if(boundary===null)return;
    processing.current=true;const i=index.current;const start=cursor.current;const token=sessionStorage.getItem(SESSION_KEY);setGeneratingMinute(i);
    void generateMinuteSummaryInterval(transcripts,templateId,i,boundary,start).then(item=>{
      if(token===sessionStorage.getItem(SESSION_KEY)){append(item);cursor.current=boundary;index.current=i+1;}
    }).finally(()=>{processing.current=false;setGeneratingMinute(null);});
  },[activeDuration,isRecording,transcripts,templateId,append]);
  return {summaries,generatingMinute};
}
