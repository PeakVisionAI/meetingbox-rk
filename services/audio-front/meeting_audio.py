"""Stateful WebRTC APM branch. Input/output PCM16 mono 16k; raw audio is unchanged."""
import ctypes,os,numpy as np
class MeetingAudio:
 def __init__(self):
  self.mode=os.getenv('MEETINGBOX_AUDIO_ENHANCE','off');self.processed_samples=0
  self.lib=None;self.state=None;self.failed=None
  if self.mode=='off':return
  try:
   self.lib=ctypes.CDLL('/audio-front/libmeeting_apm.so')
   self.lib.mb_apm_create.argtypes=[ctypes.c_int,ctypes.c_int];self.lib.mb_apm_create.restype=ctypes.c_void_p
   self.lib.mb_apm_process.argtypes=[ctypes.c_void_p,ctypes.POINTER(ctypes.c_short),ctypes.c_int]
   self.lib.mb_apm_destroy.argtypes=[ctypes.c_void_p]
   self.state=self.lib.mb_apm_create(int(self.mode in ('agc','agc_ns')),int(self.mode in ('ns','agc_ns')))
   if not self.state:raise RuntimeError('WebRTC APM init failed')
  except Exception as error:self.failed=str(error);self.state=None
 def process(self,pcm):
  if not self.state:return pcm
  values=np.frombuffer(pcm,dtype='<i2').copy();length=len(values);pad=(-length)%160
  if pad:values=np.pad(values,(0,pad))
  code=self.lib.mb_apm_process(self.state,values.ctypes.data_as(ctypes.POINTER(ctypes.c_short)),len(values))
  if code:raise RuntimeError('WebRTC APM error '+str(code))
  self.processed_samples+=length
  return values[:length].astype('<i2').tobytes()
 def close(self):
  if self.state:self.lib.mb_apm_destroy(self.state);self.state=None
