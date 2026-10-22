/* Small stable ABI around the pinned upstream sherpa-onnx C API. */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "sherpa-onnx/c-api/c-api.h"

void *mb_create(const char *model, const char *tokens) {
  SherpaOnnxOfflineRecognizerConfig config;
  memset(&config, 0, sizeof(config));
  config.feat_config.sample_rate = 16000;
  config.feat_config.feature_dim = 80;
  config.model_config.sense_voice.model = model;
  config.model_config.sense_voice.language = "auto";
  config.model_config.sense_voice.use_itn = 1;
  config.model_config.tokens = tokens;
  config.model_config.num_threads = 1;
  config.model_config.provider = "rknn";
  config.decoding_method = "greedy_search";
  return (void *)SherpaOnnxCreateOfflineRecognizer(&config);
}

void *mb_recognize(void *handle, const float *samples, int32_t count) {
  const SherpaOnnxOfflineRecognizer *recognizer = handle;
  const SherpaOnnxOfflineStream *stream = SherpaOnnxCreateOfflineStream(recognizer);
  if (!stream) return NULL;
  SherpaOnnxAcceptWaveformOffline(stream, 16000, samples, count);
  SherpaOnnxDecodeOfflineStream(recognizer, stream);
  const SherpaOnnxOfflineRecognizerResult *result = SherpaOnnxGetOfflineStreamResult(stream);
  char *text = NULL;
  if (result && result->text) {
    size_t size = strlen(result->text) + 1;
    text = malloc(size);
    if (text) memcpy(text, result->text, size);
  }
  if (result) SherpaOnnxDestroyOfflineRecognizerResult(result);
  SherpaOnnxDestroyOfflineStream(stream);
  return text;
}

void mb_free_text(void *text) { free(text); }
void mb_destroy(void *handle) { SherpaOnnxDestroyOfflineRecognizer(handle); }
