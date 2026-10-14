"""语音前段的配置：全部来自环境变量和数据目录，不读任何配置文件。

时限的默认值必须与 `packages/contracts/src/voiceFront.ts` 的 `VOICE_FRONT_TIMING`
一致——页面按那份常量画倒计时，这里按这份常量结句。
"""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path

SAMPLE_RATE = 16_000

KWS_MODEL_DIRNAME = "sherpa-onnx-kws-zipformer-wenetspeech-3.3M-2024-01-01"
VAD_MODEL_FILENAME = "silero_vad.onnx"

DEFAULT_WAKE_WORDS = ("小峰小峰",)
DEFAULT_BARGE_WORDS = ("停一下", "停下", "取消", "等一下")


def env_flag(name: str, default: bool) -> bool:
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def env_float(name: str, default: float) -> float:
    try:
        return float(os.getenv(name, "").strip() or default)
    except ValueError:
        return default


def env_int(name: str, default: int) -> int:
    try:
        return int(os.getenv(name, "").strip() or default)
    except ValueError:
        return default


def _env_words(name: str, default: tuple[str, ...]) -> tuple[str, ...]:
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return default
    return tuple(w.strip() for w in raw.replace("，", ",").split(",") if w.strip())


def default_model_root() -> Path:
    """模型根目录：`<语音模型组数据目录>/front`。

    Server 启动 sidecar 时会把语音模型组的数据目录放进 `BOSSAI_ASR_MODEL_DIR`；
    开发时可以用 `BOSSAI_VOICE_FRONT_MODEL_DIR` 直接指到任意目录。
    """
    explicit = os.getenv("BOSSAI_VOICE_FRONT_MODEL_DIR", "").strip()
    if explicit:
        return Path(explicit)
    base = os.getenv("BOSSAI_ASR_MODEL_DIR", "").strip() or os.getenv("MODELSCOPE_CACHE", "").strip()
    return (Path(base) if base else Path.cwd()) / "front"


@dataclass(frozen=True)
class VoiceFrontSettings:
    #: 总开关。关掉后 `/voice/health` 报 `disabled`，不加载任何东西。
    enabled: bool = True
    model_root: Path = field(default_factory=default_model_root)

    wake_words: tuple[str, ...] = DEFAULT_WAKE_WORDS
    barge_words: tuple[str, ...] = DEFAULT_BARGE_WORDS
    keywords_score: float = 1.0
    keywords_threshold: float = 0.25
    num_trailing_blanks: int = 0

    wake_timeout_s: float = 5.0
    followup_window_s: float = 10.0
    wake_end_silence_s: float = 2.0
    free_end_silence_s: float = 0.7
    min_speech_s: float = 0.25
    max_utterance_s: float = 60.0
    vad_threshold: float = 0.5
    #: 唤醒前只在内存里保留最近这么久的音频，不落盘。
    preroll_s: float = 2.0

    #: 单块音频的硬上限（秒）。契约上限是 0.5 秒，这里留一倍余量给时钟抖动。
    max_chunk_s: float = 1.0
    #: 免唤醒判断的前文：最近这么久之内听到的上一句。
    context_window_s: float = 30.0
    #: 等待识别的句子积压到这么多秒就开始丢新句子。
    max_pending_audio_s: float = 90.0
    #: 一份监听状态闲置这么久就清掉。
    session_idle_ttl_s: float = 600.0
    max_sessions: int = 8

    language: str = "auto"
    use_itn: bool = True

    @property
    def kws_dir(self) -> Path:
        return self.model_root / "kws" / KWS_MODEL_DIRNAME

    @property
    def vad_path(self) -> Path:
        return self.model_root / "vad" / VAD_MODEL_FILENAME

    @property
    def addressee_dir(self) -> Path:
        return self.model_root / "addressee"

    @classmethod
    def from_env(cls) -> "VoiceFrontSettings":
        return cls(
            enabled=env_flag("BOSSAI_VOICE_FRONT", True),
            model_root=default_model_root(),
            wake_words=_env_words("BOSSAI_VOICE_FRONT_WAKE_WORDS", DEFAULT_WAKE_WORDS),
            barge_words=_env_words("BOSSAI_VOICE_FRONT_BARGE_WORDS", DEFAULT_BARGE_WORDS),
            keywords_score=env_float("BOSSAI_VOICE_FRONT_KEYWORDS_SCORE", 1.0),
            keywords_threshold=env_float("BOSSAI_VOICE_FRONT_KEYWORDS_THRESHOLD", 0.25),
            num_trailing_blanks=env_int("BOSSAI_VOICE_FRONT_TRAILING_BLANKS", 0),
            language=os.getenv("BOSSAI_VOICE_FRONT_LANGUAGE", "").strip().lower() or "auto",
            # 关掉数字规整只用于和不带规整的字错率口径对比；正常运行要开，下游靠标点断句。
            use_itn=env_flag("BOSSAI_VOICE_FRONT_ITN", True),
        )
