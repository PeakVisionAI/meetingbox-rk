"""Native sherpa-onnx RKNN bridge; never switches execution providers."""
import ctypes
import multiprocessing
import os
import platform
import queue
import threading
from pathlib import Path
from assets import verify_assets


class NativeEngine:
    def __init__(self, directory):
        if platform.system() != "Linux" or platform.machine() not in ("aarch64", "arm64"):
            raise RuntimeError("RKNN service requires Linux aarch64 on RK3588")
        self.directory = Path(directory).resolve()
        manifest = verify_assets(self.directory)
        library = self.directory / "libmeetingbox_rknn.so"
        self.native = ctypes.CDLL(str(library))
        self.native.mb_create.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
        self.native.mb_create.restype = ctypes.c_void_p
        self.native.mb_recognize.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_float), ctypes.c_int32]
        self.native.mb_recognize.restype = ctypes.c_void_p
        self.native.mb_free_text.argtypes = [ctypes.c_void_p]
        self.native.mb_free_text.restype = None
        self.native.mb_destroy.argtypes = [ctypes.c_void_p]
        self.native.mb_destroy.restype = None
        self.handle = self.native.mb_create(
            str(self.directory / manifest["model_file"]).encode(),
            str(self.directory / manifest["tokens_file"]).encode(),
        )
        if not self.handle:
            raise RuntimeError("sherpa-onnx RKNN initialization failed")
        self.model_id = manifest["model_id"]
        self.runtime_info = {"sherpa_onnx": manifest["sherpa_onnx_version"],
                             "model_sha256": manifest["files"][manifest["model_file"]]["sha256"],
                             "rknn_runtime": "2.2.0", "max_seconds": 30}

    def recognize(self, samples, language):
        # meetingbox's RK client currently sends no language; model auto-detects.
        if language != "auto":
            raise ValueError("this service uses automatic language detection")
        audio = (ctypes.c_float * len(samples))(*samples)
        text = self.native.mb_recognize(self.handle, audio, len(samples))
        if not text:
            raise RuntimeError("sherpa-onnx RKNN decode failed")
        try:
            return ctypes.string_at(text).decode("utf-8")
        finally:
            self.native.mb_free_text(text)

    def close(self):
        if self.handle:
            self.native.mb_destroy(self.handle)
            self.handle = None


def native_worker(connection, directory):
    # Upstream RKNN errors can _Exit(); isolate them from the HTTP server.
    # Suppress upstream logs which may contain model paths or decoded content.
    with open(os.devnull, "w") as sink:
        os.dup2(sink.fileno(), 1)
        os.dup2(sink.fileno(), 2)
    engine = None
    try:
        engine = NativeEngine(directory)
        connection.send({"model": engine.model_id, "runtime": engine.runtime_info})
        while True:
            request = connection.recv()
            if request is None:
                break
            connection.send({"text": engine.recognize(*request)})
    except Exception:
        try:
            connection.send({"error": "native worker failed"})
        except (OSError, EOFError):
            pass
    finally:
        if engine:
            engine.close()
        connection.close()


class RknnEngine:
    def __init__(self, directory):
        directory = Path(directory).resolve()
        manifest = verify_assets(directory)
        if "libmeetingbox_rknn.so" not in manifest["files"]:
            raise RuntimeError("build and inventory the native bridge first")
        context = multiprocessing.get_context("spawn")
        self.connection, child = context.Pipe()
        self.process = context.Process(target=native_worker, args=(child, str(directory)), daemon=True)
        previous = os.environ.get("LD_LIBRARY_PATH")
        os.environ["LD_LIBRARY_PATH"] = str(directory / "lib")
        try:
            self.process.start()
        finally:
            if previous is None:
                os.environ.pop("LD_LIBRARY_PATH", None)
            else:
                os.environ["LD_LIBRARY_PATH"] = previous
        child.close()
        try:
            status = self.receive(300)
            if "model" not in status:
                raise RuntimeError("RKNN initialization failed")
            self.model_id = status["model"]
            self.runtime_info = status["runtime"]
        except BaseException:
            self.close()
            raise

    @property
    def ready(self):
        return self.process.is_alive()

    def exchange(self, request, timeout, send=False):
        results = queue.Queue(maxsize=1)

        def communicate():
            try:
                if send:
                    self.connection.send(request)
                results.put((self.connection.recv(), None))
            except Exception as error:
                results.put((None, error))

        # Bound the complete IPC exchange, including send and partial receive.
        thread = threading.Thread(target=communicate, daemon=True)
        thread.start()
        try:
            result, error = results.get(timeout=timeout)
        except queue.Empty as error:
            self.close()
            thread.join(timeout=1)
            raise RuntimeError("native RKNN worker timed out") from error
        thread.join(timeout=1)
        if error:
            raise RuntimeError("native RKNN worker exited") from error
        if not isinstance(result, dict) or "error" in result:
            raise RuntimeError("native RKNN worker failed")
        return result

    def receive(self, timeout):
        return self.exchange(None, timeout)

    def recognize(self, samples, language):
        try:
            return self.exchange((samples, language), 85, send=True)["text"]
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.process.is_alive():
            self.process.terminate()
        self.process.join(timeout=5)
        if self.process.is_alive():
            self.process.kill()
            self.process.join(timeout=5)
        self.connection.close()
