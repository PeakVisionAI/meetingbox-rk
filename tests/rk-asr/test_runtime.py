import multiprocessing
import sys
import time
import struct
import os
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "services" / "rk-asr"))
from runtime import RknnEngine


def worker_exit(connection):
    connection.recv()
    connection.close()


def worker_reply(connection):
    connection.recv()
    connection.send({"text": "你好"})
    connection.close()


def worker_never_reads(connection):
    time.sleep(10)


def worker_partial_reply(connection):
    os.write(connection.fileno(), struct.pack("!i", 10000))
    time.sleep(10)


class RuntimeTests(unittest.TestCase):
    def engine(self, target):
        engine = RknnEngine.__new__(RknnEngine)
        context = multiprocessing.get_context("spawn")
        engine.connection, child = context.Pipe()
        engine.process = context.Process(target=target, args=(child,))
        engine.process.start()
        child.close()
        return engine

    def test_native_process_exit_is_failure(self):
        engine = self.engine(worker_exit)
        try:
            with self.assertRaises(RuntimeError):
                engine.recognize([0.1] * 1600, "auto")
            self.assertFalse(engine.process.is_alive())
        finally:
            engine.close()

    def test_text_copied_across_process_boundary(self):
        engine = self.engine(worker_reply)
        try:
            self.assertEqual(engine.recognize([0.1] * 1600, "auto"), "你好")
        finally:
            engine.close()

    def test_timeout_covers_blocked_send_and_partial_reply(self):
        for worker, send in ((worker_never_reads, True), (worker_partial_reply, False)):
            with self.subTest(worker=worker.__name__):
                engine = self.engine(worker)
                started = time.monotonic()
                try:
                    with self.assertRaisesRegex(RuntimeError, "timed out"):
                        engine.exchange(([0.1] * 480000, "auto"), 0.15, send=send)
                    self.assertFalse(engine.process.is_alive())
                    self.assertLess(time.monotonic() - started, 2)
                finally:
                    engine.close()

    def test_receive_timeout_and_close(self):
        engine = self.engine(worker_exit)
        try:
            with self.assertRaisesRegex(RuntimeError, "timed out"):
                engine.receive(0.01)
        finally:
            engine.close()
        self.assertFalse(engine.process.is_alive())


if __name__ == "__main__":
    unittest.main()
