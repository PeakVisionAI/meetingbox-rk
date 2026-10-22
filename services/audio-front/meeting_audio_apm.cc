#include <webrtc/modules/audio_processing/include/audio_processing.h>
#include <webrtc/modules/interface/module_common_types.h>
#include <cstring>
extern "C" {
void* mb_apm_create(int agc,int ns){
 auto *p=webrtc::AudioProcessing::Create();if(!p)return nullptr;
 p->high_pass_filter()->Enable(true);
 p->noise_suppression()->set_level(webrtc::NoiseSuppression::kLow);
 p->noise_suppression()->Enable(ns!=0);
 p->gain_control()->set_mode(webrtc::GainControl::kAdaptiveDigital);
 p->gain_control()->set_target_level_dbfs(6);
 p->gain_control()->set_compression_gain_db(9);
 p->gain_control()->enable_limiter(true);
 p->gain_control()->Enable(agc!=0);
 return p;
}
int mb_apm_process(void* state, short* pcm, int n){
 auto *p=(webrtc::AudioProcessing*)state;
 if(!p || n%160)return -1;
 for(int i=0;i<n;i+=160){webrtc::AudioFrame f;f.sample_rate_hz_=16000;f.samples_per_channel_=160;f.num_channels_=1;memcpy(f.data_,pcm+i,320);int code=p->ProcessStream(&f);if(code)return code;memcpy(pcm+i,f.data_,320);}
 return 0;
}
void mb_apm_destroy(void* state){delete (webrtc::AudioProcessing*)state;}
}
