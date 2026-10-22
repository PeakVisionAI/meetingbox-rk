"""免唤醒判断：一句话是不是对设备说的。

移植自 peaknode-voice-demo 的 `peaknode/addressee/infer.py`。模型是微调过的
chinese-roberta-wwm-ext，从数据目录读；输入模板、截断长度、阈值文件都与训练和
评测保持一致，否则线上线下的分数会漂移。

与 demo 的不同：

- 模型缺失时没有“假实现”兜底，直接报不可用——此时唤醒模式照常工作，只是免唤醒开不了。
- 不调用 `torch.set_num_threads`。sidecar 里同一个进程还跑着语音识别，
  改全局线程数会影响它。
- 只用 fp32。动态 int8 量化会把整条分数分布压到约十分之一，阈值要另挑，
  而且实测召回更低（demo 实测：fp32 误触发 4% 时召回 94%，int8 误触发 5% 时召回 89%）。
"""
from __future__ import annotations

import json
import logging
import threading
import time
from pathlib import Path
from typing import Any, Optional

log = logging.getLogger("bossai.voice_front")

__all__ = ["AddresseeClassifier", "AddresseeUnavailable", "build_input", "CTX_TPL", "MAX_LEN"]

#: 训练、评测、推理共用的输入模板。改动它等于换了一个模型。
CTX_TPL = "前文：{ctx}。当前：{text}"
MAX_LEN = 192
_WEIGHT_FILES = ("model.safetensors", "pytorch_model.bin")


class AddresseeUnavailable(RuntimeError):
    """模型文件缺失、阈值缺失或加载失败。"""


def build_input(text: str, ctx: str = "") -> str:
    """把“前文 + 当前句”拼成模型输入；没有前文时只送当前句。"""
    text = (text or "").strip()
    ctx = (ctx or "").strip()
    return CTX_TPL.format(ctx=ctx, text=text) if ctx else text


class AddresseeClassifier:
    def __init__(self, model_dir: str | Path) -> None:
        self.model_dir = Path(model_dir)
        self.threshold: Optional[float] = None
        self._model: Any = None
        self._tokenizer: Any = None
        self._torch: Any = None
        self._load_lock = threading.Lock()
        self.load_ms: Optional[float] = None

    @classmethod
    def files_present(cls, model_dir: str | Path) -> bool:
        base = Path(model_dir)
        return (
            (base / "config.json").exists()
            and (base / "threshold.json").exists()
            and any((base / name).exists() for name in _WEIGHT_FILES)
        )

    @property
    def loaded(self) -> bool:
        return self._model is not None

    def _read_threshold(self) -> float:
        """阈值必须来自训练产物；读不到就不可用，不拿一个默认值凑数。

        模型输出的概率接近双峰，随手填 0.5 会让误触发率和召回率都偏离评测时的工作点。
        """
        try:
            data = json.loads((self.model_dir / "threshold.json").read_text(encoding="utf-8"))
            value = float(data["threshold"])
        except Exception as exc:  # noqa: BLE001
            raise AddresseeUnavailable(f"读不到判定阈值：{type(exc).__name__}") from exc
        if not 0.0 < value < 1.0:
            raise AddresseeUnavailable("判定阈值不在 (0, 1) 之间")
        return value

    def load(self) -> None:
        """加载模型（可重复调用）。失败抛 `AddresseeUnavailable`。"""
        if self._model is not None:
            return
        with self._load_lock:
            if self._model is not None:
                return
            if not self.files_present(self.model_dir):
                raise AddresseeUnavailable("免唤醒判断模型文件不完整")
            threshold = self._read_threshold()
            started = time.perf_counter()
            try:
                import torch
                from transformers import AutoModelForSequenceClassification, AutoTokenizer

                tokenizer = AutoTokenizer.from_pretrained(str(self.model_dir))
                model = AutoModelForSequenceClassification.from_pretrained(str(self.model_dir))
                model.eval()
            except Exception as exc:  # noqa: BLE001
                raise AddresseeUnavailable(f"免唤醒判断模型加载失败：{type(exc).__name__}") from exc
            self._torch = torch
            self._tokenizer = tokenizer
            self.threshold = threshold
            self._model = model
            self.load_ms = (time.perf_counter() - started) * 1000.0
            log.info("voice_front addressee loaded in %.0f ms", self.load_ms)

    def score(self, text: str, context: str = "") -> float:
        """这句话是对设备说的概率。空文本直接返回 0，不跑模型。"""
        text = (text or "").strip()
        if not text:
            return 0.0
        self.load()
        torch = self._torch
        encoded = self._tokenizer([build_input(text, context)], return_tensors="pt",
                                  padding=True, truncation=True, max_length=MAX_LEN)
        with torch.no_grad():
            logits = self._model(**encoded).logits.float()
            return float(torch.softmax(logits, dim=-1)[0, 1].item())

    def is_command(self, probability: float) -> bool:
        if self.threshold is None:
            raise AddresseeUnavailable("免唤醒判断模型尚未加载")
        return probability >= self.threshold
