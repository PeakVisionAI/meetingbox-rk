"""Pure-logic tests for RK speaker clustering.

No audio and no sherpa-onnx needed: the embedding model is replaced by fixed
vectors so the assignment policy (threshold, capacity, short utterances) is
verified deterministically.
"""

import unittest

from diarization import OnlineSpeakerCluster, SpeakerEmbedder, cosine


def unit(x, y):
    return [x, y, 0.0]


class OnlineSpeakerClusterTest(unittest.TestCase):
    def test_similar_embeddings_share_a_cluster(self):
        cluster = OnlineSpeakerCluster(threshold=0.8, max_speakers=4, min_utterance_ms=0)
        first = cluster.assign(unit(1.0, 0.0), 3000)
        second = cluster.assign(unit(0.98, 0.2), 3000)
        self.assertEqual(first, 0)
        self.assertEqual(second, 0)
        self.assertEqual(len(cluster.centroids), 1)

    def test_distant_embeddings_create_new_clusters(self):
        cluster = OnlineSpeakerCluster(threshold=0.8, max_speakers=4, min_utterance_ms=0)
        self.assertEqual(cluster.assign(unit(1.0, 0.0), 3000), 0)
        self.assertEqual(cluster.assign(unit(0.0, 1.0), 3000), 1)

    def test_short_utterance_is_not_labelled(self):
        cluster = OnlineSpeakerCluster(threshold=0.8, max_speakers=4, min_utterance_ms=800)
        self.assertIsNone(cluster.assign(unit(1.0, 0.0), 300))
        self.assertEqual(cluster.centroids, [])

    def test_capacity_is_bounded_and_reuses_best_match(self):
        cluster = OnlineSpeakerCluster(threshold=0.99, max_speakers=2, min_utterance_ms=0)
        cluster.assign(unit(1.0, 0.0), 3000)
        cluster.assign(unit(0.0, 1.0), 3000)
        overflow = cluster.assign(unit(0.2, 0.9), 3000)
        self.assertLessEqual(len(cluster.centroids), 2)
        self.assertEqual(overflow, 1)

    def test_empty_embedding_is_rejected(self):
        cluster = OnlineSpeakerCluster(min_utterance_ms=0)
        self.assertIsNone(cluster.assign([], 3000))
        self.assertIsNone(cluster.assign([0.0, 0.0, 0.0], 3000))

    def test_labels_are_one_based_and_none_safe(self):
        self.assertEqual(OnlineSpeakerCluster.label(0), "发言人1")
        self.assertEqual(OnlineSpeakerCluster.label(2), "发言人3")
        self.assertIsNone(OnlineSpeakerCluster.label(None))

    def test_cosine_handles_mismatched_and_degenerate_input(self):
        self.assertEqual(cosine([], [1.0]), 0.0)
        self.assertEqual(cosine([0.0, 0.0], [0.0, 0.0]), 0.0)
        self.assertAlmostEqual(cosine([1.0, 0.0], [1.0, 0.0]), 1.0, places=6)


class SpeakerEmbedderTest(unittest.TestCase):
    def test_missing_model_reports_unavailable_without_raising(self):
        embedder = SpeakerEmbedder(model_path="/nonexistent/model.onnx")
        self.assertFalse(embedder.available)
        self.assertIn("missing", embedder.reason)
        self.assertIsNone(embedder.embed(b"\x00" * 3200))

    def test_unset_model_is_reported(self):
        embedder = SpeakerEmbedder(model_path="")
        embedder.model_path = ""
        self.assertFalse(embedder.available)
        self.assertIn("MEETINGBOX_SPEAKER_MODEL", embedder.reason)

    def test_too_short_audio_is_skipped_before_inference(self):
        embedder = SpeakerEmbedder(model_path="")
        embedder._loaded = True
        embedder._extractor = object()  # pretend a model is present
        self.assertIsNone(embedder.embed(b"\x00" * 1600))


if __name__ == "__main__":
    unittest.main()
