"""Local VM-only OpenVINO separator. Fixed 4.5-second blocks, one GPU request at a time."""
import base64, json, pathlib, sys, threading, time, os, ctypes
import numpy as np
import openvino as ov
from fastapi import FastAPI, Request, HTTPException

ROOT = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, 'D:/MeetingBox-Intel/firered-int8-test/deps')
dll_dir=os.add_dll_directory('D:/MeetingBox-Intel/firered-int8-test/deps/onnxruntime/capi')
ort_dll=ctypes.WinDLL('D:/MeetingBox-Intel/firered-int8-test/deps/onnxruntime/capi/onnxruntime.dll')
import sherpa_onnx
app = FastAPI(title='MeetingBox overlap GPU worker')
lock = threading.Lock()
compiled = extractor = None

@app.on_event('startup')
def startup():
    global compiled, extractor
    compiled = ov.Core().compile_model(str(ROOT/'mossformer2-4p5s-fp32.xml'), 'GPU',
        {'PERFORMANCE_HINT':'LATENCY','INFERENCE_PRECISION_HINT':'f16'})
    compiled([np.zeros((1,72000),np.float32)])
    extractor = sherpa_onnx.SpeakerEmbeddingExtractor(sherpa_onnx.SpeakerEmbeddingExtractorConfig(
        model=str(ROOT/'campplus.onnx'), num_threads=2, provider='cpu'))

def embedding(x):
    s = extractor.create_stream(); s.accept_waveform(16000, x.astype(np.float32)); s.input_finished()
    if not extractor.is_ready(s): raise ValueError('insufficient audio for embedding')
    v = np.asarray(extractor.compute(s), np.float32)
    return (v/(np.linalg.norm(v)+1e-12)).tolist()

def samples(blob):
    if len(blob)%2 or not 16000 <= len(blob) <= 144000: raise HTTPException(400,'expected 0.5–4.5 seconds mono PCM16/16k')
    return np.frombuffer(blob,dtype='<i2').astype(np.float32)/32768

@app.get('/health')
def health():
    return {'ready':compiled is not None and extractor is not None,'device':'GPU','precision':'f16','block_seconds':4.5}

@app.post('/embed')
async def embed(request:Request):
    import asyncio
    x=samples(await request.body())
    def run():
        with lock:return {'embedding':embedding(x)}
    return await asyncio.to_thread(run)

@app.post('/separate')
async def separate(request:Request):
    import asyncio
    x=samples(await request.body())
    def run():
        with lock:
            started=time.perf_counter(); padded=np.zeros((1,72000),np.float32);padded[0,:len(x)]=x
            result=compiled([padded]); seconds=time.perf_counter()-started; tracks=[]
            input_rms=float(np.sqrt(np.mean(x*x)))
            ys=[np.array(result[p][0,:len(x)],copy=True) for p in compiled.outputs]
            strongest=max(float(np.sqrt(np.mean(y*y))) for y in ys)
            for y in ys:
                rms=float(np.sqrt(np.mean(y*y)))
                relative_db=20*np.log10((rms+1e-12)/(input_rms+1e-12))
                # Reject weak residuals BEFORE any independent loudness normalization.
                if 20*np.log10((rms+1e-12)/(strongest+1e-12)) < -24: continue
                vector=embedding(y)
                y*=min(8.,input_rms/max(rms,1e-12)); y/=max(1.,float(np.max(np.abs(y)))/.98)
                pcm=(y*32767).astype('<i2').tobytes()
                tracks.append({'pcm':base64.b64encode(pcm).decode(),'embedding':vector,'raw_relative_db':float(relative_db)})
            return {'seconds':seconds,'tracks':tracks,'audio_seconds':len(x)/16000,'model':'MossFormer2-OpenVINO-FP16'}
    return await asyncio.to_thread(run)
