FROM meetingbox-fast:1
USER root
RUN sed -i 's|http://deb.debian.org/debian|https://mirrors.aliyun.com/debian|g' /etc/apt/sources.list.d/debian.sources && apt-get update -qq && apt-get install -y --no-install-recommends libwebrtc-audio-processing-dev g++ pkg-config && rm -rf /var/lib/apt/lists/*
COPY meeting_audio_apm.cc /tmp/meeting_audio_apm.cc
RUN mkdir -p /audio-front && g++ -O2 -shared -fPIC /tmp/meeting_audio_apm.cc -o /audio-front/libmeeting_apm.so $(pkg-config --cflags --libs webrtc-audio-processing)
COPY meeting_audio.py /app/meeting_audio.py
