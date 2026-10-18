import json
import pathlib
import tempfile
import unittest
from benchmark import digest, freeze, score, split, write


class BenchmarkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.rows = [{"repo": repo, "number": n, "base_sha": "1" * 40, "head_sha": "2" * 40, "label": "bad", "why": "FUTURE_FIX_SECRET"}
                     for repo, n in [("example/first", 1), ("example/first", 2), ("example/second", 3)]]
        write(self.root / "source.json", self.rows)
        self.cohort = self.root / "cohort.json"
        self.labels = self.root / "evaluation" / "labels.json"
        self.frozen = freeze(self.root / "source.json", self.cohort, self.labels)
        self.results = self.root / "results"
        self.results.mkdir()

    def test_labels_and_future_explanations_never_enter_hunter_cohort(self):
        text = self.cohort.read_text()
        self.assertNotIn("FUTURE_FIX_SECRET", text)
        self.assertNotIn('"label"', text)
        self.assertIn("FUTURE_FIX_SECRET", self.labels.read_text())
        labels = json.loads(self.labels.read_text())
        self.assertEqual(labels["cohort_sha256"], digest(self.frozen))

    def test_repository_split_is_stable_case_insensitive_and_disjoint(self):
        a, b, _ = self.frozen["rows"]
        self.assertEqual(a["split"], b["split"])
        self.assertEqual(split("EXAMPLE/FIRST"), split("example/first"))
        self.assertEqual(split("example/first"), a["split"])

    def test_all_selected_prs_and_both_arms_remain_when_nothing_runs(self):
        report = score(self.cohort, self.results)
        self.assertEqual(len(report["rows"]), 6)
        for arm in ("baseline", "hunter"):
            result = report["summary"][arm]
            self.assertEqual(result["denominator"], 3)
            self.assertEqual(result["run_prs"], 0)
            self.assertEqual(result["unsupported_prs"], 3)
            self.assertIsNone(result["median_seconds"])
        self.assertEqual(report["summary"]["hunter"]["cost_known_prs"], 0)

    def test_partial_and_wrong_revision_results_cannot_shrink_the_denominator(self):
        write(self.results / "bad" / "result.json", {"repository": "example/first", "number": 1, "base": "1" * 40, "head": "3" * 40, "results": []})
        report = score(self.cohort, self.results)
        self.assertEqual(len(report["summary"]["invalid_artifacts"]), 1)
        self.assertEqual(len(report["rows"]), 6)
        self.assertEqual(report["summary"]["hunter"]["run_prs"], 0)

    def test_observation_is_not_silently_counted_as_adjudicated_product_success(self):
        write(self.results / "first" / "result.json", {"repository": "example/first", "number": 1, "base": "1" * 40, "head": "2" * 40,
            "cost_usd": None, "results": [{"arm": "hunter", "seconds": 2, "judgments": [{"confirmed": True, "old": ["Passed", "Passed"], "new": ["Failed", "Failed"]}]}]})
        report = score(self.cohort, self.results)
        hunter = report["summary"]["hunter"]
        self.assertEqual(hunter["run_prs"], 1)
        self.assertEqual(hunter["confirmed_candidates"], 1)
        self.assertEqual(hunter["adjudicated_prs"], 0)
        self.assertEqual(hunter["denominator"], 3)
        self.assertEqual(hunter["unsupported_prs"], 2)

    def test_duplicate_prs_are_rejected_before_a_cohort_is_frozen(self):
        write(self.root / "duplicates.json", [self.rows[0], self.rows[0]])
        with self.assertRaises(ValueError):
            freeze(self.root / "duplicates.json", self.root / "bad.json", self.root / "bad-labels.json")

    def test_an_over_budget_confirmation_is_invalid_evidence(self):
        write(self.results / "over" / "result.json", {"repository": "example/first", "number": 1, "base": "1" * 40, "head": "2" * 40,
            "results": [{"arm": "hunter", "seconds": 1801, "judgments": [{"confirmed": True}]}]})
        result = score(self.cohort, self.results)
        self.assertEqual(len(result["summary"]["invalid_artifacts"]), 1)
        self.assertEqual(result["summary"]["hunter"]["confirmed_candidates"], 0)

    def test_duplicate_arms_are_invalid_and_keep_both_missing_rows(self):
        write(self.results / "duplicate" / "result.json", {"repository": "example/first", "number": 1, "base": "1" * 40, "head": "2" * 40,
            "results": [{"arm": "hunter"}, {"arm": "hunter"}]})
        result = score(self.cohort, self.results)
        self.assertEqual(len(result["summary"]["invalid_artifacts"]), 1)
        self.assertEqual(len(result["rows"]), 6)

    def test_adjudication_for_other_revisions_cannot_be_reused(self):
        path = self.root / "stale-review.json"
        write(path, {"cohort_sha256": "a" * 64, "reviews": {}})
        with self.assertRaises(ValueError):
            score(self.cohort, self.results, path)


if __name__ == "__main__":
    unittest.main()
