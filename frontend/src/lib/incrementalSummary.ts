import type { Transcript } from '@/types';
export const SUMMARY_INTERVAL_SECONDS = 300;
export function startOf(t: Transcript) { return t.audio_start_time ?? t.chunk_start_time ?? 0; }
export function endOf(t: Transcript) { return Math.max(startOf(t)+.01, t.audio_end_time ?? startOf(t) + (t.duration ?? 0)); }
export function speech(t: Transcript) {
  const m = t.text.match(/^((?:候选发言人|发言人)[^：:\s]{0,20})[：:]\s*([\s\S]*)$/);
  return { speaker: t.speaker || m?.[1], text: (m?.[2] || t.text).trim() };
}
export function intervalSegments(transcripts: Transcript[], start: number, end: number) {
  const parents = new Map<number, number>();
  for (const t of transcripts) if (t.id.startsWith('live-') && t.sequence_id) {
    const base = Math.floor((t.sequence_id - 1) / 100) * 100 + 1;
    parents.set(base, Math.min(parents.get(base) ?? Infinity, startOf(t)));
  }
  return [...transcripts].sort((a,b) => startOf(a)-startOf(b) || (a.sequence_id || 0)-(b.sequence_id || 0))
    .filter(t => {
      const base = t.sequence_id ? Math.floor((t.sequence_id-1)/100)*100+1 : 0;
      const time = t.id.startsWith('live-') ? parents.get(base) ?? startOf(t) : startOf(t);
      return !t.is_partial && time >= start && time < end;
    }).map(t => ({...speech(t), start_seconds:startOf(t)})).filter(t => t.text.length > 0);
}
export function sourceText(transcripts: Transcript[], start: number, end: number) {
  return intervalSegments(transcripts,start,end).map(t => t.text).join('').replace(/[\s\p{P}\p{S}]/gu,'');
}
export function nextSummaryBoundary(transcripts: Transcript[], cursor: number, now: number, finished=false): number | null {
  const groups: {speaker?:string;text:string;end:number;closed:boolean}[]=[];
  for (const t of [...transcripts].sort((a,b)=>startOf(a)-startOf(b))) {
    if (t.is_partial || !t.text.trim()) continue;
    const s=speech(t);const last=groups.at(-1);
    const complete=last ? /[。！？!?][”’」』）)]*$/.test(last.text.trim()) : false;
    const same=!!s.speaker && !/待识别|候选/.test(s.speaker) && last?.speaker===s.speaker;
    if (last && same && !(last.text.length>=200 && complete)) {
      last.text+=' '+s.text;last.end=Math.max(last.end,endOf(t));
    } else {
      if(last)last.closed=true;
      groups.push({...s,end:endOf(t),closed:false});
    }
  }
  const last=groups.at(-1);
  if(last && finished)last.closed=true;
  const ready=groups.find(g=>g.closed && g.end>=cursor+SUMMARY_INTERVAL_SECONDS && g.end<=now);
  return ready ? Math.ceil(ready.end) : null;
}
