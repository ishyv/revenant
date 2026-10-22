"""Synthetic correctness tests; these do not measure this machine's performance."""

import copy
import importlib.util
import math
from pathlib import Path
import unittest


MODULE = Path(__file__).resolve().parents[1] / "benchmark_compare.py"
if MODULE.exists():
    spec = importlib.util.spec_from_file_location("benchmark_compare", MODULE)
    comparison = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(comparison)
else:
    comparison = None


def report(samples=None):
    samples = [100] * 15 if samples is None else samples
    return {
        "schemaVersion": 1,
        "suite": "revenant-optimization",
        "mode": "measure",
        "valid": True,
        "invalidationReasons": [],
        "label": "baseline",
        "workloadDigest": "a" * 64,
        "machine": {"fingerprint": "b" * 64},
        "build": {
            "compiler": "rustc synthetic 1",
            "cargo": "cargo synthetic 1",
            "profile": "release",
            "features": {"native": [], "graphics": []},
            "source": {"gitHead": "c" * 40, "dirty": True, "digest": "d" * 64},
            "executables": [{"sha256": "e" * 64}],
        },
        "policy": {"warmups": 3, "repeats": 15, "stability": "explicit-idle-window"},
        "components": [{
            "schemaVersion": 1,
            "suite": "revenant-optimization",
            "component": "native",
            "mode": "measure",
            "workloadVersion": 1,
            "warmups": 3,
            "repeats": 15,
            "cases": [{
                "id": "scheduler.cpu",
                "parameters": {"entries": 500000, "owners": 1},
                "metrics": [{"name": "elapsed", "unit": "ns", "better": "lower", "samples": samples}],
                "correctness": {"passed": True, "checksum": 378},
            }],
            "limitations": [],
        }],
    }


class ModuleTests(unittest.TestCase):
    def test_comparison_api_exists(self):
        self.assertIsNotNone(comparison, "the comparison module has not been implemented")


@unittest.skipIf(comparison is None, "comparison module not implemented")
class ComparisonTests(unittest.TestCase):
    def test_desktop_background_conditions_cannot_change_between_runs(self):
        baseline, candidate = report(), report()
        for data in (baseline, candidate):
            data["policy"].update(controlProfile="desktop", backgroundCondition="brave-playing",
                                  maxExternalCpuPercent=20, maxExternalGpuPercent=20)
        self.assertTrue(comparison.compare_reports(baseline, candidate)["metrics"])
        candidate["policy"]["backgroundCondition"] = "brave-paused"
        with self.assertRaisesRegex(ValueError, "policy"):
            comparison.compare_reports(baseline, candidate)

    def test_utilization_enforcement_cannot_change_between_runs(self):
        baseline, candidate = report(), report()
        for data in (baseline, candidate):
            data["policy"].update(controlProfile="desktop", backgroundCondition="brave-playing",
                                  utilizationEnforcement="record-only")
        candidate["policy"]["utilizationEnforcement"] = "reject"
        with self.assertRaisesRegex(ValueError, "policy"):
            comparison.compare_reports(baseline, candidate)

    def test_desktop_condition_warnings_remain_visible_in_comparison(self):
        baseline, candidate = report(), report()
        baseline["conditionWarnings"] = ["External CPU utilization exceeds diagnostic threshold 20%"]
        baseline["limitations"] = ["Desktop timing includes background interference."]
        result = comparison.compare_reports(baseline, candidate)
        self.assertEqual(result["conditionWarnings"]["baseline"], baseline["conditionWarnings"])
        rendered = comparison.render_comparison(result)
        self.assertIn("External CPU utilization", rendered)
        self.assertIn("Desktop timing includes background interference", rendered)

    def test_short_matched_policy_cannot_claim_confidence(self):
        baseline = report([100])
        baseline["policy"]["repeats"] = 1
        baseline["components"][0]["repeats"] = 1
        with self.assertRaises(ValueError):
            comparison.compare_reports(baseline, copy.deepcopy(baseline))

    def test_compiler_configuration_changes_require_separate_experiment(self):
        baseline, candidate = report(), report()
        baseline["build"]["configuration"] = {"environment": {"RUSTFLAGS": ""}}
        candidate["build"]["configuration"] = {"environment": {"RUSTFLAGS": "-C target-cpu=native"}}
        with self.assertRaises(ValueError):
            comparison.compare_reports(baseline, candidate)

    def test_selected_gpu_hardware_must_be_present_and_match(self):
        baseline, candidate = report(), report()
        for data in (baseline, candidate):
            case = data["components"][0]["cases"][0]
            case["requiredProfile"] = "gpu"
            case["hardwareIdentity"] = {"adapter": "GPU A", "backend": "DX12"}
        self.assertTrue(comparison.compare_reports(baseline, candidate)["metrics"])
        candidate["components"][0]["cases"][0]["hardwareIdentity"]["adapter"] = "GPU B"
        with self.assertRaises(ValueError):
            comparison.compare_reports(baseline, candidate)
        del candidate["components"][0]["cases"][0]["hardwareIdentity"]
        with self.assertRaises(ValueError):
            comparison.compare_reports(candidate, candidate)

    def result(self, baseline, candidate):
        return comparison.compare_reports(baseline, candidate)["metrics"][0]

    def test_source_and_executable_changes_are_comparable(self):
        baseline, candidate = report(), report([80] * 15)
        candidate["build"]["source"] = {"gitHead": "f" * 40, "dirty": False, "digest": "0" * 64}
        candidate["build"]["executables"][0]["sha256"] = "1" * 64
        result = self.result(baseline, candidate)
        self.assertEqual(result["classification"], "improvement")
        self.assertEqual(result["absoluteChange"], -20)
        self.assertAlmostEqual(result["percentChange"], -20)
        self.assertAlmostEqual(result["medianRatio95"][0], 0.8)
        self.assertEqual(result["baseline"]["p95"], 100)

    def test_regression_is_kept_alongside_improvement(self):
        baseline, candidate = report(), report([80] * 15)
        for data, samples in [(baseline, [100] * 15), (candidate, [125] * 15)]:
            data["components"][0]["cases"][0]["metrics"].append({
                "name": "completion", "unit": "ns", "better": "lower", "samples": samples,
            })
        result = comparison.compare_reports(baseline, candidate)
        self.assertEqual([metric["classification"] for metric in result["metrics"]], ["regression", "improvement"])
        text = comparison.render_comparison(result)
        self.assertIn("regression", text)
        self.assertIn("improvement", text)
        self.assertIn("completion", text)
        self.assertIn("empirical", text)

    def test_practical_threshold_prevents_tiny_win_claim(self):
        result = self.result(report(), report([96] * 15))
        self.assertEqual(result["classification"], "inconclusive")

    def test_exact_practical_threshold_is_inconclusive_in_both_directions(self):
        for better in ("lower", "higher"):
            for samples in ([95] * 15, [105] * 15):
                baseline, candidate = report(), report(samples)
                for data in (baseline, candidate):
                    data["components"][0]["cases"][0]["metrics"][0]["better"] = better
                with self.subTest(better=better, median=samples[0]):
                    self.assertEqual(self.result(baseline, candidate)["classification"], "inconclusive")

    def test_independent_intervals_ignore_sample_order(self):
        baseline = report(list(range(1, 16)))
        candidate = report(list(range(12, 27)))
        reordered = copy.deepcopy(candidate)
        reordered["components"][0]["cases"][0]["metrics"][0]["samples"].reverse()
        self.assertEqual(self.result(baseline, candidate), self.result(baseline, reordered))

    def test_uncertain_median_effect_is_inconclusive_and_reproducible(self):
        baseline = report([10] * 7 + [100] + [1000] * 7)
        candidate = report([5] * 7 + [80] + [1500] * 7)
        first = comparison.compare_reports(baseline, candidate)
        self.assertEqual(first, comparison.compare_reports(baseline, candidate))
        result = first["metrics"][0]
        self.assertLess(result["medianRatio95"][0], 0.95)
        self.assertGreater(result["medianRatio95"][1], 1.05)
        self.assertEqual(result["classification"], "inconclusive")

    def test_higher_direction_reverses_classification(self):
        baseline, candidate = report(), report([120] * 15)
        for data in (baseline, candidate):
            data["components"][0]["cases"][0]["metrics"][0]["better"] = "higher"
        result = self.result(baseline, candidate)
        self.assertEqual(result["classification"], "improvement")
        self.assertAlmostEqual(result["normalizedPercentChange"], -20)

    def test_p95_is_empirical_nearest_rank_not_population_estimate(self):
        result = self.result(report(list(range(1, 16))), report(list(range(1, 16))))
        self.assertEqual(result["baseline"]["p95"], 15)
        self.assertEqual(result["baseline"]["median"], 8)
        self.assertEqual(result["baseline"]["sampleCount"], 15)

    def test_counters_are_diagnostic_with_no_invented_zero_ratio(self):
        baseline, candidate = report(), report()
        for data, samples in [(baseline, [0] * 15), (candidate, [7] * 15)]:
            data["components"][0]["cases"][0]["metrics"][0].update({
                "unit": "count", "samples": samples,
            })
        result = self.result(baseline, candidate)
        self.assertEqual(result["classification"], "diagnostic")
        self.assertEqual(result["absoluteChange"], 7)
        self.assertIsNone(result["percentChange"])
        self.assertIsNone(result["medianRatio95"])
        self.assertIsNone(result["normalizedPercent95"])

    def test_exact_nonzero_bytes_are_diagnostic(self):
        baseline, candidate = report(), report()
        for data, samples in [(baseline, [2**64] * 15), (candidate, [2**64 + 8192] * 15)]:
            data["components"][0]["cases"][0]["metrics"][0].update({"unit": "bytes", "samples": samples})
        result = self.result(baseline, candidate)
        self.assertEqual(result["absoluteChange"], 8192)
        self.assertEqual(result["classification"], "diagnostic")

    def test_comparison_is_finite_json_with_explicit_counter_nulls(self):
        import json
        baseline, candidate = report(), report()
        for data in (baseline, candidate):
            data["components"][0]["cases"][0]["metrics"][0].update({"unit": "count", "samples": [0] * 15})
        encoded = json.dumps(comparison.compare_reports(baseline, candidate), allow_nan=False)
        self.assertIn('"medianRatio95": null', encoded)

    def test_unrepresentable_counter_percentage_is_refused(self):
        baseline, candidate = report([1e-308] * 15), report([1e308] * 15)
        for data in (baseline, candidate):
            data["components"][0]["cases"][0]["metrics"][0]["unit"] = "count"
        with self.assertRaises(ValueError):
            comparison.compare_reports(baseline, candidate)

    def test_reordered_cases_and_metrics_remain_comparable(self):
        baseline = report()
        second = copy.deepcopy(baseline["components"][0]["cases"][0])
        second["id"] = "scheduler.io"
        baseline["components"][0]["cases"].append(second)
        candidate = copy.deepcopy(baseline)
        candidate["components"][0]["cases"].reverse()
        self.assertEqual(len(comparison.compare_reports(baseline, candidate)["metrics"]), 2)


@unittest.skipIf(comparison is None, "comparison module not implemented")
class AdmissionTests(unittest.TestCase):
    def reject(self, mutate):
        baseline, candidate = report(), report()
        mutate(candidate)
        with self.assertRaises(ValueError):
            comparison.compare_reports(baseline, candidate)

    def test_rejects_incompatible_measurement_identity(self):
        mutations = {
            "schema": lambda r: r.update(schemaVersion=2),
            "suite": lambda r: r.update(suite="other"),
            "workloads": lambda r: r.update(workloadDigest="0" * 64),
            "machine": lambda r: r["machine"].update(fingerprint="0" * 64),
            "compiler": lambda r: r["build"].update(compiler="rustc synthetic 2"),
            "cargo": lambda r: r["build"].update(cargo="cargo synthetic 2"),
            "profile": lambda r: r["build"].update(profile="debug"),
            "features": lambda r: r["build"]["features"].update(native=["experimental-compute"]),
            "policy": lambda r: r["policy"].update(stability="other"),
            "warmups": lambda r: r["components"][0].update(warmups=4),
            "repeats": lambda r: r["components"][0].update(repeats=14),
            "component": lambda r: r["components"][0].update(component="graphics"),
            "workload version": lambda r: r["components"][0].update(workloadVersion=2),
            "case ids": lambda r: r["components"][0]["cases"][0].update(id="different"),
            "parameters": lambda r: r["components"][0]["cases"][0]["parameters"].update(entries=1000000),
            "metric names": lambda r: r["components"][0]["cases"][0]["metrics"][0].update(name="other"),
            "metric units": lambda r: r["components"][0]["cases"][0]["metrics"][0].update(unit="bytes"),
            "metric directions": lambda r: r["components"][0]["cases"][0]["metrics"][0].update(better="higher"),
            "required profile": lambda r: r["components"][0]["cases"][0].update(requiredProfile="gpu"),
        }
        for label, mutate in mutations.items():
            with self.subTest(label=label):
                self.reject(mutate)

    def test_refuses_invalid_run_in_either_position(self):
        for position in (0, 1):
            for field, value in [("mode", "verify"), ("valid", False), ("invalidationReasons", ["machine in use"])]:
                reports = [report(), report()]
                reports[position][field] = value
                with self.subTest(position=position, field=field), self.assertRaises(ValueError):
                    comparison.compare_reports(*reports)

    def test_rejects_failed_correctness_or_nonmeasurement_component(self):
        for mutate in (
            lambda r: r["components"][0]["cases"][0]["correctness"].update(passed=False),
            lambda r: r["components"][0].update(mode="verify"),
        ):
            self.reject(mutate)

    def test_rejects_missing_or_duplicate_components_cases_and_metrics(self):
        for mutate in (
            lambda r: r.update(components=[]),
            lambda r: r["components"].append(copy.deepcopy(r["components"][0])),
            lambda r: r["components"][0].update(cases=[]),
            lambda r: r["components"][0]["cases"].append(copy.deepcopy(r["components"][0]["cases"][0])),
            lambda r: r["components"][0]["cases"][0].update(metrics=[]),
            lambda r: r["components"][0]["cases"][0]["metrics"].append(copy.deepcopy(r["components"][0]["cases"][0]["metrics"][0])),
        ):
            self.reject(mutate)

    def test_rejects_nonfinite_negative_bool_zero_timing_and_wrong_sample_counts(self):
        for bad in (math.nan, math.inf, -math.inf, -1, True, False, 0, "100", None):
            with self.subTest(bad=bad):
                self.reject(lambda r: r["components"][0]["cases"][0]["metrics"][0]["samples"].__setitem__(0, bad))
        for samples in ([], [100] * 14, [100] * 16):
            with self.subTest(samples=len(samples)):
                self.reject(lambda r: r["components"][0]["cases"][0]["metrics"][0].update(samples=samples))

    def test_rejects_missing_required_identity_without_keyerror(self):
        for field in ("build", "machine", "policy", "workloadDigest", "valid"):
            self.reject(lambda r: r.pop(field))


if __name__ == "__main__":
    unittest.main()
