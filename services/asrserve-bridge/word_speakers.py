"""Conservative final attribution using timed words and exclusive voice activity."""
import re
import numpy as np


def register_stable_labels(activity, labels):
    """Recover voices hidden by whole-utterance voting, using sustained evidence."""
    exclusive = activity.sum(axis=1) == 1
    for column in range(activity.shape[1]):
        if column in labels:
            continue
        mask = exclusive & activity[:, column]
        edges = np.flatnonzero(np.diff(np.r_[False, mask, False].astype(np.int8)))
        longest = max(edges[1::2] - edges[::2], default=0)
        if mask.sum() >= 120 and longest >= 60:
            labels[column] = f'说话人{len(labels)+1}'


def assign_words(record, activity, labels, min_turn_ms=600):
    aligned = record.get('alignment')
    if not aligned or not aligned.get('words'):
        return []
    groups = []
    matched = []
    cursor = 0
    text = record['text']
    for word in aligned['words']:
        token = word['text'].strip()
        if not token:
            continue
        match = re.search(re.escape(token), text[cursor:], re.IGNORECASE)
        if match is None:
            return []
        pos = cursor + match.start()
        cursor += match.end()
        matched.append((pos, word))
        start = aligned['audio_start_ms'] + round(word['start_time'] * 1000)
        end = aligned['audio_start_ms'] + round(word['end_time'] * 1000)
        a = activity[max(0, start // 10):min(len(activity), (end + 9) // 10)]
        exclusive = a[a.sum(axis=1) == 1]
        votes = exclusive.sum(axis=0)
        column = int(votes.argmax()) if votes.size and votes.sum() and votes.max() / votes.sum() >= .65 else None
        speaker = labels.get(column) if column is not None else None
        speaker = speaker.replace('说话人', '发言人', 1) if speaker else record.get('speaker')
        if groups and groups[-1]['speaker'] == speaker:
            groups[-1]['end'] = end
        else:
            groups.append(dict(pos=pos, speaker=speaker, start=start, end=end))
    if not groups:
        return []
    # Short islands cannot introduce a different identity. Never create labels.
    for i, group in enumerate(groups):
        if group['end'] - group['start'] < min_turn_ms:
            if 0 < i < len(groups) - 1 and groups[i-1]['speaker'] == groups[i+1]['speaker']:
                group['speaker'] = groups[i-1]['speaker']
            else:
                group['speaker'] = record.get('speaker')
    merged = []
    for group in groups:
        if merged and merged[-1]['speaker'] == group['speaker']:
            merged[-1]['end'] = group['end']
        else:
            merged.append(dict(group))
    merged[0]['pos'] = 0
    parts = []
    for i, group in enumerate(merged):
        stop = merged[i+1]['pos'] if i+1 < len(merged) else len(text)
        parts.append(dict(text=text[group['pos']:stop], speaker=group['speaker'],
                          start_ms=group['start'], end_ms=group['end'],
                          alignment={**aligned, 'words': [word for pos, word in matched if group['pos'] <= pos < stop]}))
    if len(parts) >= 100 or ''.join(part['text'] for part in parts) != text:
        return []
    return parts
