import re

def distribute_aligned_text(text, words, turns, audio_start_ms):
    """Assign timed words without re-recognizing or modifying the complete text."""
    matched=[];cursor=0
    for word in words:
        token=word['text'].strip()
        if not token:continue
        match=re.search(re.escape(token),text[cursor:],re.IGNORECASE)
        if match is None:raise ValueError('Alignment text mismatch; original retained')
        pos=cursor+match.start();cursor=cursor+match.end()
        mid=audio_start_ms+round((word['start_time']+word['end_time'])*500)
        mid=max(turns[0]['start_ms'],min(mid,turns[-1]['end_ms']-1))
        turn=next((t for t in turns if t['start_ms']<=mid<t['end_ms']),None)
        matched.append((pos,turn.get('speaker') if turn else None,max(turns[0]['start_ms'],min(audio_start_ms+round(word['start_time']*1000),turns[-1]['end_ms']))))
    if not matched:raise ValueError('No aligned words; original retained')
    starts=[]
    for pos,speaker,mid in matched:
        if not starts or starts[-1]['speaker']!=speaker:
            starts.append({'pos':pos,'speaker':speaker,'word_ms':mid})
    starts[0]['pos']=0
    parts=[]
    for i,item in enumerate(starts):
        end=starts[i+1]['pos'] if i+1<len(starts) else len(text)
        part=text[item['pos']:end]
        if part:parts.append({'text':part,'speaker':item['speaker'],
            'start_ms':item['word_ms'],
            'end_ms':starts[i+1]['word_ms'] if i+1<len(starts) else min(turns[-1]['end_ms'],audio_start_ms+round(words[-1]['end_time']*1000))})
    if ''.join(p['text'] for p in parts)!=text:raise ValueError('Text coverage mismatch')
    return parts
