import type { Transcript } from '@/types';

/** Replace an utterance in place while preserving its UI identity and time order. */
export function upsertLiveTranscript(
  previous: Transcript[], incoming: Transcript,
  compare: (a: Transcript, b: Transcript) => number,
): Transcript[] {
  const existing = previous.findIndex(item => item.sequence_id === incoming.sequence_id);
  const result = [...previous];
  if (existing >= 0) result[existing] = { ...incoming, id: previous[existing].id };
  else result.push(incoming);
  return result.sort(compare);
}
