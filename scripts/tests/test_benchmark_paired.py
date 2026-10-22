"""Paired sampling guards; synthetic fixtures never claim performance evidence."""
import copy
import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import benchmark_suite as suite


def component(value=100, allocation=10):
    return {"schemaVersion": 1, "suite": suite.SUITE, "component": "native",
            "mode": "measure", "workloadVersion": 1, "warmups": 3, "repeats": 15,
            "cases": [{"id": "dispatch.progress", "parameters": {"calls": 5000},
                       "correctness": {"passed": True}, "metrics": [
                           {"name": "latency", "unit": "ns", "better": "lower", "samples": [value] * 15},
                           {"name": "allocation_calls", "unit": "count", "better": "lower", "samples": [allocation] * 15}]}]}


def descriptor():
    case = copy.deepcopy(component()["cases"][0])
    case.pop("correctness")
    for metric in case["metrics"]:
        metric.pop("samples")
    return case


def pairs():
    return [{"index": index, "order": ["baseline", "candidate"] if index < 8 else ["candidate", "baseline"],
             "baseline": {"component": component()},
             "candidate": {"component": component(90, 0)}} for index in range(15)]


class PairedStatistics(unittest.TestCase):
    def setUp(self):
        import benchmark_paired
        self.paired = benchmark_paired

    def test_counterbalanced_schedule_is_fixed_reproducible_and_complete(self):
        schedule = self.paired.schedule()
        self.assertEqual(schedule, self.paired.schedule())
        self.assertEqual([row["index"] for row in schedule], list(range(15)))
        self.assertEqual(sum(row["order"] == ["baseline", "candidate"] for row in schedule), 8)
        self.assertEqual(sum(row["order"] == ["candidate", "baseline"] for row in schedule), 7)
        self.assertNotEqual([row["order"] for row in schedule], [row["order"] for row in pairs()])

    def test_equal_and_faster_timings_have_known_paired_intervals(self):
        for candidate, expected in ((100, 1.0), (90, 0.9), (105, 1.05), (120, 1.2)):
            rows = pairs()
            for row in rows:
                row["candidate"]["component"] = component(candidate, 0)
            report = self.paired.compare_pairs(rows, [descriptor()], resamples=128)
            timing = report["metrics"][0]
            self.assertEqual(timing["ratio95Envelope"], [expected, expected])
            self.assertEqual(report["decision"], "reject" if candidate > 105 else "adopt")

    def test_pairing_cancels_correlated_load_without_normalizing_by_cpu(self):
        rows = pairs()
        for index, row in enumerate(rows):
            load = 10 ** (index % 5)
            row["baseline"]["component"] = component(100 * load)
            row["candidate"]["component"] = component(90 * load, 0)
        report = self.paired.compare_pairs(rows, [descriptor()], resamples=256)
        self.assertEqual(report["metrics"][0]["ratio95Envelope"], [0.9, 0.9])
        self.assertEqual(report["bootstrap"]["blockLengths"], [1, 2, 3])
        self.assertEqual(report, self.paired.compare_pairs(rows, [descriptor()], resamples=256))

    def test_all_raw_samples_and_slow_processes_are_retained(self):
        rows = pairs()
        rows[-1]["candidate"]["component"] = component(10000, 0)
        rows[0]["baseline"]["component"]["cases"][0]["metrics"][0]["samples"][-1] = 100000
        before = copy.deepcopy(rows)
        report = self.paired.compare_pairs(rows, [descriptor()], resamples=128)
        self.assertEqual(rows, before)
        timing = report["metrics"][0]
        self.assertEqual(timing["chronological"][-1]["candidateMedian"], 10000)
        self.assertEqual(len(timing["chronological"]), 15)
        self.assertIn("AB", timing["orderEffects"])
        self.assertIn("BA", timing["orderEffects"])

    def test_uncertain_latency_keeps_candidate_pending(self):
        rows = pairs()
        for index, row in enumerate(rows):
            row["candidate"]["component"] = component(50 if index < 8 else 200, 0)
        report = self.paired.compare_pairs(rows, [descriptor()], resamples=512)
        self.assertEqual(report["decision"], "inconclusive")
        self.assertGreater(report["metrics"][0]["ratio95Envelope"][1], 1.05)

    def test_counter_metrics_do_not_invent_ratios_at_zero_baseline(self):
        rows = pairs()
        for row in rows:
            row["baseline"]["component"] = component(100, 0)
        report = self.paired.compare_pairs(rows, [descriptor()], resamples=64)
        diagnostic = report["metrics"][1]
        self.assertEqual(diagnostic["classification"], "diagnostic")
        self.assertIsNone(diagnostic["candidateBaselineRatio"])
        self.assertIsNone(diagnostic["ratio95Envelope"])

    def test_each_raw_retention_repetition_must_match_the_baseline(self):
        for name in ("retained_update_json", "retained_result_json", "active_scopes_after",
                     "retained_result_json_after", "known_scratch_after"):
            with self.subTest(metric=name):
                rows = pairs()
                fixed = descriptor()
                fixed["metrics"].append({"name": name, "unit": "count", "better": "lower"})
                for row in rows:
                    for side in ("baseline", "candidate"):
                        samples = [0] * 15 if side == "baseline" else [0] * 8 + [1_000_000_000] * 7
                        row[side]["component"]["cases"][0]["metrics"].append(
                            {**fixed["metrics"][-1], "samples": samples})
                before = copy.deepcopy(rows)
                report = self.paired.compare_pairs(rows, [fixed], resamples=64)
                self.assertEqual(rows, before)
                self.assertEqual(report["metrics"][-1]["candidateMedian"], 0)
                self.assertFalse(report["retentionBounded"])
                self.assertEqual(report["decision"], "reject")

    def test_allocation_benefit_without_latency_cannot_adopt(self):
        rows = pairs()
        fixed = descriptor()
        fixed["metrics"].pop(0)
        for row in rows:
            for side in ("baseline", "candidate"):
                row[side]["component"]["cases"][0]["metrics"].pop(0)
        report = self.paired.compare_pairs(rows, [fixed], resamples=64)
        self.assertTrue(report["allocationBenefit"])
        self.assertFalse(report["timingNonRegressionAdmitted"])
        self.assertEqual(report["decision"], "inconclusive")

    def test_whole_process_cpu_is_separate_diagnostic_with_missing_pairs_counted(self):
        rows = pairs()
        for row in rows:
            row["baseline"]["processCpu"] = {"kernelNs": 10, "userNs": 20, "totalNs": 30}
            row["candidate"]["processCpu"] = {"kernelNs": 5, "userNs": 10, "totalNs": 15}
        rows[-1]["candidate"]["processCpu"] = None
        report = self.paired.compare_pairs(rows, [descriptor()], resamples=64)
        cpu = report["wholeProcessCpu"]
        self.assertEqual(cpu["completePairs"], 14)
        self.assertEqual(cpu["metrics"][2]["candidateBaselineRatio"], 0.5)
        self.assertEqual(report["decision"], "adopt")

    def test_incomplete_duplicate_wrong_workload_and_bad_samples_are_rejected(self):
        for change in ("incomplete", "duplicate", "failed", "samples", "nan", "workload", "direction", "missing", "order"):
            rows = pairs()
            if change == "incomplete": rows.pop()
            elif change == "duplicate": rows[-1]["index"] = 0
            elif change == "order": rows[0]["order"] = ["baseline", "baseline"]
            elif change == "missing": rows[0].pop("candidate")
            else:
                case = rows[0]["candidate"]["component"]["cases"][0]
                if change == "failed": case["correctness"]["passed"] = False
                elif change == "samples": case["metrics"][0]["samples"].pop()
                elif change == "nan": case["metrics"][0]["samples"][0] = float("nan")
                elif change == "workload": case["parameters"]["calls"] = 4999
                elif change == "direction": case["metrics"][0]["better"] = "higher"
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.paired.compare_pairs(rows, [descriptor()], resamples=64)


class PairedCli(unittest.TestCase):
    def test_cli_exposes_paired_without_idle_flag(self):
        result = subprocess.run([sys.executable, str(SCRIPTS / "benchmark_suite.py"), "paired", "--help"],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("--idle-confirmed", result.stdout)
        self.assertIn("--case", result.stdout)


class PairedCollection(unittest.TestCase):
    def setUp(self):
        import benchmark_paired
        self.paired = benchmark_paired

    def fixture(self, directory):
        checkout = Path(directory) / "revenant"
        checkout.mkdir()
        builds = {}
        for label in ("a", "b"):
            manifest = {"schemaVersion": 1, "suite": suite.SUITE, "label": label,
                        "compiler": "rustc-fixture", "cargo": "cargo-fixture", "profile": "release",
                        "features": {"native": []}, "lockfileSha256": "lock", "configuration": {},
                        "drivers": {"native": {"path": "copied-" + label, "sha256": label,
                                               "descriptor": {"cases": [descriptor()]}}}}
            verification = checkout / "target/benchmark-suite/verification" / label / "verification.json"
            verification.parent.mkdir(parents=True)
            suite.write_json(verification, {"valid": True, "build": manifest})
            builds[label] = manifest
        return checkout, builds

    def test_busy_and_missing_telemetry_collect_all_pairs_without_idle_confirmation(self):
        for cpu in (0, 25, 100, None):
            with self.subTest(cpu=cpu), tempfile.TemporaryDirectory() as directory:
                checkout, builds = self.fixture(directory)
                calls = []
                def execute(entry, request, log, host, policy, calls=calls, cpu=cpu):
                    calls.append(entry["path"])
                    self.assertEqual((request["warmups"], request["repeats"]), (3, 15))
                    self.assertEqual(request["cases"], ["dispatch.progress"])
                    raw = component(100 if entry["path"] == "copied-a" else 90,
                                    10 if entry["path"] == "copied-a" else 0)
                    suite.write_json(Path(request["output"]), raw)
                    return {"component": raw, "telemetry": [{"externalCpuEstimatePercent": cpu}],
                            "processCpu": {"kernelNs": 10, "userNs": 20, "totalNs": 30}, "warnings": []}
                with patch.object(suite, "load_bundle", side_effect=lambda root, label, builds=builds: (root, builds[label])), redirect_stdout(io.StringIO()):
                    path, valid = self.paired.collect(checkout, "a", "b", "run", "browser-running",
                                                     ["dispatch.progress"], host_factory=FakeHost, executor=execute)
                report = suite.read_json(path)
                self.assertTrue(valid)
                self.assertEqual(len(calls), 30)
                self.assertEqual(len(report["pairs"]), 15)
                self.assertEqual(report["schedule"], self.paired.schedule())
                self.assertEqual(report["policy"]["utilizationEnforcement"], "record-only")
                self.assertEqual(report["pairs"][0]["baseline"]["telemetry"][0]["externalCpuEstimatePercent"], cpu)
                self.assertEqual(report["comparison"]["decision"], "adopt")
                with self.assertRaises(FileExistsError):
                    self.paired.collect(checkout, "a", "b", "run", "browser-running", host_factory=FakeHost)

    def test_containment_failure_keeps_failed_artifacts_and_stops(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, builds = self.fixture(directory)
            calls = []
            def execute(entry, request, log, host, policy):
                calls.append(entry["path"])
                log.write_text("job attach failed", encoding="utf-8")
                raise OSError("AssignProcessToJobObject")
            with patch.object(suite, "load_bundle", side_effect=lambda root, label: (root, builds[label])), redirect_stdout(io.StringIO()):
                path, valid = self.paired.collect(checkout, "a", "b", "failed", "browser-running",
                                                 ["dispatch.progress"], host_factory=FakeHost, executor=execute)
            report = suite.read_json(path)
            self.assertFalse(valid)
            self.assertEqual(len(calls), 1)
            self.assertIsNone(report["comparison"])
            self.assertIn("AssignProcessToJobObject", report["invalidationReasons"][0])
            self.assertTrue(list(path.parent.rglob("driver.log")))
            self.assertTrue((path.parent / "schedule.json").is_file())

    def test_nonfinite_raw_timing_saves_failed_report_without_changing_raw_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, builds = self.fixture(directory)
            calls = []
            raw_bytes = json.dumps(component()).replace('"samples": [100,', '"samples": [1e999,', 1).encode("utf-8")
            def execute(entry, request, log, host, policy):
                calls.append(entry["path"])
                raw_path = Path(request["output"])
                raw_path.write_bytes(raw_bytes)
                return {"component": suite.read_json(raw_path), "telemetry": [],
                        "processCpu": None, "warnings": []}
            with patch.object(suite, "load_bundle", side_effect=lambda root, label: (root, builds[label])), redirect_stdout(io.StringIO()):
                path, valid = self.paired.collect(checkout, "a", "b", "failed", "browser-running",
                                                 ["dispatch.progress"], host_factory=FakeHost, executor=execute)
            report = suite.read_json(path)
            self.assertFalse(valid)
            self.assertEqual(len(calls), 1)
            self.assertIsNone(report["comparison"])
            self.assertIn("Samples must be finite", report["invalidationReasons"][0])
            result = report["pairs"][0][report["pairs"][0]["order"][0]]
            raw_path = Path(result["rawPath"])
            self.assertEqual(raw_path.read_bytes(), raw_bytes)
            self.assertEqual(result["rawSha256"], suite.file_hash(raw_path))
            self.assertNotIn("component", result)
            self.assertIn("Samples must be finite", (path.parent / "report.md").read_text(encoding="utf-8"))

    def test_nonfinite_auxiliary_raw_evidence_saves_failed_report_without_changing_raw_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, builds = self.fixture(directory)
            calls = []
            raw = component()
            raw["cases"][0]["correctness"]["extraEvidence"] = "unbounded"
            raw_bytes = json.dumps(raw).replace('"unbounded"', '1e999', 1).encode("utf-8")
            def execute(entry, request, log, host, policy):
                calls.append(entry["path"])
                raw_path = Path(request["output"])
                raw_path.write_bytes(raw_bytes)
                return {"component": suite.read_json(raw_path), "telemetry": [],
                        "processCpu": None, "warnings": []}
            with patch.object(suite, "load_bundle", side_effect=lambda root, label: (root, builds[label])), redirect_stdout(io.StringIO()):
                path, valid = self.paired.collect(checkout, "a", "b", "failed", "browser-running",
                                                 ["dispatch.progress"], host_factory=FakeHost, executor=execute)
            report = suite.read_json(path)
            self.assertFalse(valid)
            self.assertEqual(len(calls), 1)
            self.assertIsNone(report["comparison"])
            self.assertIn("JSON compliant", report["invalidationReasons"][0])
            result = report["pairs"][0][report["pairs"][0]["order"][0]]
            raw_path = Path(result["rawPath"])
            self.assertEqual(raw_path.read_bytes(), raw_bytes)
            self.assertEqual(result["rawSha256"], suite.file_hash(raw_path))
            self.assertNotIn("component", result)
            self.assertIn("JSON compliant", (path.parent / "report.md").read_text(encoding="utf-8"))

    def test_nonfinite_manifest_metadata_cannot_poison_failed_report(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout, builds = self.fixture(directory)
            builds["a"]["extraEvidence"] = "unbounded"
            raw_bytes = json.dumps(builds["a"]).replace('"unbounded"', '1e999', 1).encode("utf-8")
            manifest_path = Path(directory) / "manifest.json"
            manifest_path.write_bytes(raw_bytes)
            def load(root, label):
                return root, suite.read_json(manifest_path) if label == "a" else builds[label]
            with patch.object(suite, "load_bundle", side_effect=load):
                path, valid = self.paired.collect(checkout, "a", "b", "failed", "browser-running",
                                                 ["dispatch.progress"], host_factory=FakeHost)
            report = suite.read_json(path)
            self.assertFalse(valid)
            self.assertEqual(report["builds"], {})
            self.assertEqual(report["pairs"], [])
            self.assertIn("JSON compliant", report["invalidationReasons"][0])
            self.assertEqual(manifest_path.read_bytes(), raw_bytes)
            self.assertIn("JSON compliant", (path.parent / "report.md").read_text(encoding="utf-8"))

    def test_build_and_verification_mismatch_cannot_be_collected(self):
        for change in ("compiler", "lock", "verification"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                checkout, builds = self.fixture(directory)
                if change == "compiler": builds["b"]["compiler"] = "other-rustc"
                elif change == "lock": builds["b"]["lockfileSha256"] = "other-lock"
                else:
                    suite.write_json(checkout / "target/benchmark-suite/verification/a/verification.json", {"valid": False})
                with patch.object(suite, "load_bundle", side_effect=lambda root, label, builds=builds: (root, builds[label])):
                    path, valid = self.paired.collect(checkout, "a", "b", "failed", "browser-running",
                                                     ["dispatch.progress"], host_factory=FakeHost)
                self.assertFalse(valid)
                self.assertTrue(suite.read_json(path)["invalidationReasons"])


class FakeHost:
    def identity(self):
        return {"fingerprint": "synthetic", "synthetic": True}

    def close(self):
        pass


class OwnedProcessTimes(unittest.TestCase):
    def test_final_cpu_times_sum_threads_convert_units_and_preserve_zero(self):
        import benchmark_host
        host = benchmark_host.WindowsHost.__new__(benchmark_host.WindowsHost)
        def read(handle, created, exited, kernel, user):
            self.assertEqual(handle.value, 123)
            kernel._obj.low, kernel._obj.high = 0, 1
            user._obj.low, user._obj.high = 7, 0
            return 1
        host._kernel = SimpleNamespace(GetProcessTimes=read)
        result = host.process_times(SimpleNamespace(_handle=123))
        self.assertEqual(result, {"kernelNs": (1 << 32) * 100, "userNs": 700,
                                  "totalNs": ((1 << 32) + 7) * 100})
        host._kernel = SimpleNamespace(GetProcessTimes=lambda *args: 1)
        self.assertEqual(host.process_times(SimpleNamespace(_handle=123)), {"kernelNs": 0, "userNs": 0, "totalNs": 0})

    def test_process_time_api_failure_remains_unavailable(self):
        import benchmark_host
        host = benchmark_host.WindowsHost.__new__(benchmark_host.WindowsHost)
        host._kernel = SimpleNamespace(GetProcessTimes=lambda *args: 0)
        with self.assertRaises(OSError):
            host.process_times(SimpleNamespace(_handle=123))


class ExecuteBlock(unittest.TestCase):
    def test_malformed_optional_cpu_counter_is_null_with_warning_and_child_completes(self):
        import benchmark_paired
        class Host:
            def own(self, process):
                self.process = process
            def snapshot(self, process=None):
                return {"idleTicks": 0, "kernelTicks": float("nan"), "userTicks": 0, "processTicks": 0,
                        "availableMemoryBytes": 100, "workingSetBytes": 10, "privateBytes": 20}
            def process_times(self, process):
                return {"kernelNs": 0, "userNs": 0, "totalNs": 0}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            host = Host()
            request = suite.driver_request("measure", path / "raw.json", path / "work", ["dispatch.progress"])
            def launch(entry, req, log, idle=False):
                suite.write_json(Path(req["output"]), component())
                return subprocess.Popen([sys.executable, "-c", "import time; time.sleep(0.02)"], stdout=log, stderr=log)
            with patch.object(suite, "launch_driver", side_effect=launch):
                result = benchmark_paired.execute_block({}, request, path / "driver.log", host,
                                                       {"processTimeoutSeconds": 5, "sampleIntervalSeconds": 0.01})
            self.assertEqual(host.process.returncode, 0)
            self.assertEqual(result["component"], component())
            self.assertTrue(result["warnings"])
            self.assertTrue(result["telemetry"])
            for row in result["telemetry"]:
                for key in ("systemCpuPercent", "benchmarkCpuPercent", "externalCpuEstimatePercent"):
                    self.assertIsNone(row[key])
                self.assertEqual(row["availableMemoryBytes"], 100)
            self.assertEqual(suite.read_json(path / "observations.json")["telemetry"], result["telemetry"])

    def test_real_short_child_runs_with_missing_optional_telemetry(self):
        import benchmark_paired
        class Host:
            def own(self, process):
                self.owned = process
            def snapshot(self, process=None):
                raise OSError("CPU observer unavailable")
            def process_times(self, process):
                self.assert_exited = process.poll() is not None
                raise OSError("CPU time unavailable")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            host = Host()
            request = suite.driver_request("measure", path / "raw.json", path / "work", ["dispatch.progress"])
            def launch(entry, req, log, idle=False):
                suite.write_json(Path(req["output"]), component())
                return subprocess.Popen([sys.executable, "-c", "pass"], stdout=log, stderr=log)
            with patch.object(suite, "launch_driver", side_effect=launch):
                result = benchmark_paired.execute_block({}, request, path / "driver.log", host,
                                                       {"processTimeoutSeconds": 5, "sampleIntervalSeconds": 0.01})
            self.assertTrue(host.assert_exited)
            self.assertEqual(result["component"], component())
            self.assertIsNone(result["processCpu"])
            self.assertTrue(result["warnings"])
            self.assertTrue(all(row["externalCpuEstimatePercent"] is None for row in result["telemetry"]))

    def test_timeout_terminates_and_joins_owned_child(self):
        import benchmark_paired
        class Host:
            def own(self, process):
                self.process = process
            def snapshot(self, process=None):
                return None
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            host = Host()
            request = suite.driver_request("measure", path / "raw.json", path / "work", ["dispatch.progress"])
            def launch(entry, req, log, idle=False):
                return subprocess.Popen([sys.executable, "-c", "import time; time.sleep(20)"], stdout=log, stderr=log)
            with patch.object(suite, "launch_driver", side_effect=launch), self.assertRaisesRegex(ValueError, "timeout"):
                benchmark_paired.execute_block({}, request, path / "driver.log", host,
                                              {"processTimeoutSeconds": 0.05, "sampleIntervalSeconds": 0.01})
            self.assertIsNotNone(host.process.returncode)


if __name__ == "__main__":
    unittest.main()
