# Meeting audio front end

WebRTC Audio Processing 0.3 (distribution package) CPU, 16kHz mono, 10ms frames. Conservative high-pass, optional adaptive digital AGC, optional low suppression. This package does not implement AEC3, beamforming or dereverberation. AEC remains disabled without synchronized render reference.

MEETINGBOX_AUDIO_ENHANCE: off / agc / ns / agc_ns. Raw stream.pcm remains immutable input for playback, VAD and diarization; asr-enhanced.pcm is the separate ASR branch. Enhancer failure bypasses to raw audio.

FireRedPunc runs as a separate Windows CPU service (2 threads), official FireRedTeam weights at audio-front/punc, code at punc-code. MEETINGBOX_PUNC_URL selects it. Partial captions arrive immediately; finalized records receive punctuation asynchronously. Word alignment uses unpunctuated raw_asr_text. Only punctuation insertions are accepted; original characters/casing/spacing and technical expressions are retained. Service timeout preserves transcript. Tasks start at logon.

Build base meetingbox-fast:1 first, then this Dockerfile. Preserve original production compose and source before deployment. Set enhancement=off and unset punctuation URL to bypass; restore pre-audio-front files/image to roll back.

Licensing: WebRTC distribution library uses BSD-style license; retain distributor copyright files. FireRed code/weights repository Apache-2.0; retain its LICENSE and source copyright. No private audio is uploaded to third parties.
