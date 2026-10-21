"""Remove explicit hesitation syllables for display, retaining verbatim ASR for timing."""
import re
_QUOTED=re.compile(r'("[^"\n]*"|“[^”\n]*”|「[^」\n]*」|『[^』\n]*』)')
def clean_fillers(text):
 # Decoder silence labels are metadata only when they form the whole result.
 # Keep actual speech mentioning SIL/ASR/silence, including user questions.
 if re.fullmatch(r'\s*(?:[<\[({]?\s*(?:sil|silence|no_speech)\s*[>\])}]?[\s.,，。!?！？;；]*)+',text,re.I):
  return ''
 def clean(part):
  part=re.sub(r'呃+', '',part)
  # Preserve a questioning interjection and lexicalised exclamations.
  part=re.sub(r'啊+(?![啊？?哈呀哟嚏])','',part)
  return part
 parts=_QUOTED.split(text);out=''.join(p if i%2 else clean(p) for i,p in enumerate(parts))
 out=re.sub(r'[，,、]\s*([，,。！？!?；;])',r'\1',out)
 out=out.lstrip(' ，,、；;').strip()
 if not any(c.isalnum() for c in out):out=''
 return out

def display_record(record):
 out=dict(record);source=out.get('text','');cleaned=clean_fillers(source)
 out['text']=cleaned
 if cleaned!=source:out.update(verbatim_text=source,filler_cleanup=True)
 return out
