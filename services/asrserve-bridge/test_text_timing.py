"""Tests for clause splitting, proportional timing and punctuation fallback."""

import unittest

from text_timing import Punctuator, ensure_ending, split_clauses, utterance_timing


class SplitClausesTest(unittest.TestCase):
    def test_splits_on_chinese_and_english_delimiters(self):
        self.assertEqual(
            split_clauses("今天讨论预算，下周确认。还有别的问题吗？"),
            ["今天讨论预算，", "下周确认。", "还有别的问题吗？"],
        )

    def test_keeps_trailing_text_without_delimiter(self):
        self.assertEqual(split_clauses("没有标点的句子"), ["没有标点的句子"])

    def test_empty_input(self):
        self.assertEqual(split_clauses("   "), [])


class EnsureEndingTest(unittest.TestCase):
    def test_appends_full_stop(self):
        self.assertEqual(ensure_ending("预算没问题"), "预算没问题。")

    def test_keeps_existing_sentence_end(self):
        self.assertEqual(ensure_ending("预算没问题！"), "预算没问题！")

    def test_promotes_clause_end_to_sentence_end(self):
        self.assertEqual(ensure_ending("预算没问题，"), "预算没问题。")


class UtteranceTimingTest(unittest.TestCase):
    def test_timing_stays_inside_the_utterance(self):
        result = utterance_timing("第一句，第二句。第三句。", 1000, 7000)
        clauses = result["clauses"]
        self.assertEqual(len(clauses), 3)
        self.assertEqual(clauses[0]["start_ms"], 1000)
        self.assertEqual(clauses[-1]["end_ms"], 7000)
        for left, right in zip(clauses, clauses[1:]):
            self.assertGreaterEqual(right["start_ms"], left["start_ms"])
            self.assertLessEqual(left["end_ms"], right["start_ms"])

    def test_longer_clause_gets_more_time(self):
        result = utterance_timing("短。这是一个明显更长的句子。", 0, 10000)
        short, long = result["clauses"]
        self.assertGreater(long["end_ms"] - long["start_ms"], short["end_ms"] - short["start_ms"])

    def test_token_times_cover_every_character(self):
        text = "预算通过。"
        result = utterance_timing(text, 0, 4000)
        self.assertEqual(len(result["token_times_ms"]), len(text))
        self.assertEqual(result["token_times_ms"][0], 0)
        self.assertLessEqual(result["token_times_ms"][-1], 4000)

    def test_degenerate_range_returns_empty(self):
        self.assertEqual(utterance_timing("文本", 500, 500)["clauses"], [])


class PunctuatorFallbackTest(unittest.TestCase):
    def test_without_model_it_still_terminates_sentences(self):
        punctuator = Punctuator(model_path="")
        punctuator.model_path = ""
        self.assertFalse(punctuator.available)
        self.assertEqual(punctuator.process("预算通过"), "预算通过。")

    def test_empty_text_is_untouched(self):
        self.assertEqual(Punctuator(model_path="").process("  "), "")

    def test_missing_model_file_is_reported(self):
        punctuator = Punctuator(model_path="/nonexistent/punct.onnx")
        self.assertFalse(punctuator.available)
        self.assertIn("missing", punctuator.reason)


if __name__ == "__main__":
    unittest.main()
