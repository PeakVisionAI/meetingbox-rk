"""Stable short speaker turns from Nemotron's 10ms activity timeline."""
import numpy as np

def speaker_turns(activity, start_ms, end_ms, frame_ms=10, min_turn_ms=500):
    first=max(0,start_ms//frame_ms);last=min(len(activity),(end_ms+frame_ms-1)//frame_ms)
    a=np.asarray(activity[first:last],dtype=bool)
    if not len(a):return []
    count=a.sum(axis=1);labels=np.where(count==1,a.argmax(axis=1),-1)
    # Bridge brief silence within the same voice; overlap remains ambiguous.
    for i in range(1,len(labels)):
        if labels[i]>=0:continue
        j=i
        while j<len(labels) and labels[j]<0:j+=1
        if j-i<=30 and i>0 and j<len(labels) and labels[i-1]==labels[j] and not np.any(count[i:j]>1):labels[i:j]=labels[j]
    runs=[];i=0
    while i<len(labels):
        j=i+1
        while j<len(labels) and labels[j]==labels[i]:j+=1
        runs.append([i,j,int(labels[i])]);i=j
    # Discard brief model flips rather than inventing a new speaker identity.
    for k in range(1,len(runs)-1):
        p,r,n=runs[k-1:k+2]
        if r[2]>=0 and (r[1]-r[0])*frame_ms<min_turn_ms and p[2]==n[2] and p[2]>=0:r[2]=p[2]
    merged=[]
    for r in runs:
        if merged and merged[-1][2]==r[2]:merged[-1][1]=r[1]
        else:merged.append(r)
    stable=[(a,b,c) for a,b,c in merged if (b-a)*frame_ms>=min_turn_ms
            and (c>=0 or np.mean(count[a:b]>1)>.5)]
    if not stable:return []
    combined=[]
    for a,b,c in stable:
        if combined and combined[-1][2]==c:
            combined[-1]=(combined[-1][0],b,c)
        else:combined.append((a,b,c))
    stable=combined
    # Partition the complete original audio, including low-confidence boundary
    # margins. Never throw away unassigned words between the identified turns.
    cuts=[start_ms]+[round((stable[i-1][1]+stable[i][0])*frame_ms/2)+first*frame_ms
                     for i in range(1,len(stable))]+[end_ms]
    return [{'start_ms':cuts[i],'end_ms':cuts[i+1],'column':c if c>=0 else None}
            for i,(_,_,c) in enumerate(stable)]
