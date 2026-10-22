"""Functional guard tests; these do not collect performance measurements."""
import copy
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import subprocess

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import benchmark_suite as suite


class SuiteGuards(unittest.TestCase):
    def test_desktop_conditions_explicitly_accept_recorded_background_load(self):
        quiet = suite.condition_policy("quiet")
        desktop = suite.condition_policy("desktop", "brave-playing")
        self.assertEqual(quiet["maxExternalCpuPercent"], 5)
        self.assertEqual(quiet["maxExternalGpuPercent"], 5)
        self.assertEqual(desktop["maxExternalCpuPercent"], 20)
        self.assertEqual(desktop["maxExternalGpuPercent"], 20)
        self.assertEqual(desktop["backgroundCondition"], "brave-playing")
        observed = {"externalCpuEstimatePercent": 12, "externalGpuPercent": 11,
                    "availableMemoryBytes": 4 * 1024**3}
        self.assertEqual(suite.stability_reasons([observed], desktop, gpu=True), [])
        self.assertTrue(suite.stability_reasons([observed], quiet, gpu=True))
        busy = {**observed, "externalCpuEstimatePercent": 80, "externalGpuPercent": 75}
        self.assertEqual(suite.stability_reasons([busy], desktop, gpu=True), [])
        self.assertEqual(desktop["utilizationEnforcement"], "record-only")
        self.assertEqual(quiet["utilizationEnforcement"], "reject")
        self.assertEqual(len(suite.condition_warnings([busy], desktop, gpu=True)), 2)

    def test_desktop_load_does_not_excuse_invalid_evidence(self):
        policy = suite.condition_policy("desktop", "brave-playing")
        observed = {"externalCpuEstimatePercent": 80, "externalGpuPercent": 75,
                    "availableMemoryBytes": 4 * 1024**3}
        for invalid in ({"externalCpuEstimatePercent": None}, {"externalCpuEstimatePercent": 101},
                        {"externalGpuPercent": None}, {"externalGpuPercent": -1},
                        {"availableMemoryBytes": 1024**3}, {"error": "sensor failed"}):
            with self.subTest(invalid=invalid):
                self.assertTrue(suite.stability_reasons([{**observed, **invalid}], policy, gpu=True))

    def test_desktop_conditions_require_a_valid_background_label(self):
        for profile, label in [("unknown", None), ("desktop", None), ("desktop", ""),
                               ("desktop", "../elsewhere"), ("quiet", "brave-playing")]:
            with self.subTest(profile=profile, label=label), self.assertRaises(ValueError):
                suite.condition_policy(profile, label)

    def test_desktop_limits_are_explicit_finite_and_profile_bound(self):
        policy = suite.condition_policy("desktop", "brave-playing", cpu_limit=25, gpu_limit=30)
        self.assertEqual((policy["maxExternalCpuPercent"], policy["maxExternalGpuPercent"]), (25, 30))
        for value in (None, True, -1, 101, float("nan"), float("inf")):
            if value is None:
                continue
            with self.subTest(value=value), self.assertRaises(ValueError):
                suite.condition_policy("desktop", "brave-playing", cpu_limit=value)
        with self.assertRaises(ValueError):
            suite.condition_policy("quiet", cpu_limit=25)

    def test_run_requires_idle_acknowledgement_before_any_side_effect(self):
        with self.assertRaisesRegex(ValueError, "idle"):
            suite.require_idle(False)

    def test_labels_cannot_escape_or_use_windows_reserved_names(self):
        for value in ["../escape", "C:/elsewhere", ".", "CON", "nul", "LPT1", "trailing.", "space name", ""]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                suite.validate_label(value)
        self.assertEqual(suite.validate_label("baseline-20261009"), "baseline-20261009")

    def test_artifact_group_is_anchored_to_existing_revenant_grouping(self):
        checkout = Path("/projects/revenant")
        self.assertEqual(suite.artifact_root(checkout), Path("/projects/revenant-source/acceptance/benchmarks"))
        self.assertEqual(suite.build_root(checkout), checkout / "target/benchmark-suite/prepared")

    def test_cpu_noise_excludes_benchmark_process_but_detects_external_work(self):
        previous = {"idleTicks": 100, "kernelTicks": 100, "userTicks": 0, "processTicks": 0}
        # 1,000 aggregate CPU ticks: 300 busy ticks are all benchmark work.
        current = {"idleTicks": 800, "kernelTicks": 800, "userTicks": 300, "processTicks": 300}
        observed = suite.cpu_interval(previous, current)
        self.assertEqual(observed["systemCpuPercent"], 30)
        self.assertEqual(observed["benchmarkCpuPercent"], 30)
        self.assertEqual(observed["externalCpuEstimatePercent"], 0)
        current["processTicks"] = 100
        self.assertEqual(suite.cpu_interval(previous, current)["externalCpuEstimatePercent"], 20)
        with self.assertRaises(ValueError):
            suite.cpu_interval(current, previous)

    def test_uncertain_or_busy_machine_is_rejected(self):
        policy = suite.DEFAULT_POLICY
        clean = {"externalCpuEstimatePercent": 1.0, "availableMemoryBytes": 4 * 1024**3}
        self.assertEqual(suite.stability_reasons([clean] * 10, policy), [])
        for bad in [{**clean, "externalCpuEstimatePercent": 15},
                    {**clean, "availableMemoryBytes": 1024**3},
                    {**clean, "externalCpuEstimatePercent": None}]:
            self.assertTrue(suite.stability_reasons([bad], policy))
        self.assertTrue(suite.stability_reasons([], policy))

    def test_verify_outputs_cannot_masquerade_as_measured_reports(self):
        component = {
            "schemaVersion": 1, "suite": "revenant-optimization", "component": "native",
            "mode": "verify", "workloadVersion": 1, "warmups": 0, "repeats": 1,
            "cases": [{"id": "dispatch.light", "parameters": {"calls": 1}, "metrics": [],
                       "correctness": {"passed": True}}], "limitations": [],
        }
        suite.validate_component(component, "native", "verify", {"dispatch.light"}, 0, 1)
        with self.assertRaises(ValueError):
            suite.validate_component(component, "native", "measure", {"dispatch.light"}, 3, 15)
        contaminated = copy.deepcopy(component)
        contaminated["cases"][0]["metrics"] = [{"name": "latency", "samples": [123]}]
        with self.assertRaises(ValueError):
            suite.validate_component(contaminated, "native", "verify", {"dispatch.light"}, 0, 1)

    def test_diagnosis_rejects_timing_contamination_and_malformed_allocations(self):
        case_id = "task.progress.bytes60000.listeners0"
        good = {"schemaVersion": 1, "suite": suite.SUITE, "component": "native",
                "mode": "diagnose", "workloadVersion": 1, "warmups": 0, "repeats": 1,
                "cases": [{"id": case_id, "parameters": {}, "metrics": [],
                           "correctness": {"passed": True}, "rawObservations": [{
                               "kind": "allocation-diagnosis", "iterations": 100,
                               "allocationCalls": 300, "cumulativeRequestedBytes": 6004700}]}]}
        suite.validate_component(good, "native", "diagnose", {case_id}, 0, 1)
        for change in ("time", "metric", "duplicate", "missing", "bool", "negative", "float", "iterations", "case"):
            bad = copy.deepcopy(good)
            case = bad["cases"][0]
            observation = case["rawObservations"][0]
            if change == "time": observation["elapsedNs"] = 1
            elif change == "metric": case["metrics"] = [{"name": "elapsed", "unit": "ns"}]
            elif change == "duplicate": case["rawObservations"].append(copy.deepcopy(observation))
            elif change == "missing": observation.pop("allocationCalls")
            elif change == "bool": observation["allocationCalls"] = True
            elif change == "negative": observation["allocationCalls"] = -1
            elif change == "float": observation["allocationCalls"] = 1.5
            elif change == "iterations": observation["iterations"] = 5000
            elif change == "case": case["id"] = "dispatch.light"
            with self.subTest(change=change), self.assertRaises(ValueError):
                suite.validate_component(bad, "native", "diagnose", {case["id"]}, 0, 1)

    def test_diagnosis_uses_only_copied_native_driver_and_separate_output(self):
        root = SCRIPTS.parent / "target/benchmark-suite/unit-tests"
        root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=root) as directory:
            checkout = Path(directory)
            ids = [f"task.progress.bytes{size}.listeners{count}"
                   for size in (0, 1024, 60000) for count in (0, 1, 8)]
            manifest = {"drivers": {"native": {"path": "copied-driver", "descriptor": {
                "cases": [{"id": case_id} for case_id in ids]}}}}

            def call(entry, request, log):
                self.assertEqual(entry["path"], "copied-driver")
                self.assertEqual(request["mode"], "diagnose")
                self.assertEqual(request["cases"], ids)
                return {"schemaVersion": 1, "suite": suite.SUITE, "component": "native",
                        "mode": "diagnose", "workloadVersion": 1, "warmups": 0, "repeats": 1,
                        "cases": [{"id": case_id, "parameters": {}, "metrics": [],
                                   "correctness": {"passed": True}, "rawObservations": [{
                                       "kind": "allocation-diagnosis", "iterations": 100,
                                       "allocationCalls": 0, "cumulativeRequestedBytes": 0}]} for case_id in ids]}

            with patch.object(suite, "load_bundle", return_value=(checkout, manifest)), \
                 patch.object(suite, "protocol_call", side_effect=call) as driver, \
                 patch.object(suite, "preflight") as preflight:
                output = suite.diagnose(checkout, "diagnosis-test")
                self.assertEqual(driver.call_count, 1)
                preflight.assert_not_called()
                self.assertTrue(suite.read_json(output)["valid"])
                with self.assertRaises(FileExistsError):
                    suite.diagnose(checkout, "diagnosis-test")
                retry = suite.diagnose(checkout, "diagnosis-test", "diagnosis-test-retry")
                self.assertNotEqual(output, retry)
                self.assertEqual(suite.read_json(retry)["build"], manifest)

    def test_failed_incomplete_or_duplicate_component_outputs_are_rejected(self):
        good = {
            "schemaVersion": 1, "suite": "revenant-optimization", "component": "native",
            "mode": "measure", "workloadVersion": 1, "warmups": 3, "repeats": 2,
            "cases": [{"id": "dispatch.light", "parameters": {"calls": 1},
                       "metrics": [{"name": "latency", "unit": "ns", "better": "lower", "samples": [10, 20]}],
                       "correctness": {"passed": True}}], "limitations": [],
        }
        suite.validate_component(good, "native", "measure", {"dispatch.light"}, 3, 2)
        for change in ["missing", "duplicate", "failed", "nan", "short", "bool"]:
            bad = copy.deepcopy(good)
            if change == "missing": bad["cases"] = []
            elif change == "duplicate": bad["cases"].append(copy.deepcopy(bad["cases"][0]))
            elif change == "failed": bad["cases"][0]["correctness"]["passed"] = False
            elif change == "nan": bad["cases"][0]["metrics"][0]["samples"][0] = float("nan")
            elif change == "short": bad["cases"][0]["metrics"][0]["samples"] = [10]
            elif change == "bool": bad["cases"][0]["metrics"][0]["samples"][0] = True
            with self.subTest(change=change), self.assertRaises(ValueError):
                suite.validate_component(bad, "native", "measure", {"dispatch.light"}, 3, 2)

    def test_prepared_executable_tampering_is_detected(self):
        temporary_root = SCRIPTS.parent / "target/benchmark-suite/unit-tests"
        temporary_root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=temporary_root) as directory:
            path = Path(directory) / "driver.exe"
            path.write_bytes(b"original")
            entry = {"path": str(path), "sha256": suite.file_hash(path)}
            suite.verify_executable(entry)
            path.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "hash"):
                suite.verify_executable(entry)

    def test_resolved_paths_reject_escape(self):
        root = SCRIPTS.parent / "target/benchmark-suite"
        with self.assertRaises(ValueError):
            suite.contained(root / "../outside", root)
        self.assertEqual(suite.contained(root / "prepared/label", root), (root / "prepared/label").resolve())

    def test_gpu_profile_requires_reliable_quiet_telemetry(self):
        clean = {"externalCpuEstimatePercent": 0, "availableMemoryBytes": 4 * 1024**3,
                 "externalGpuPercent": 0}
        self.assertEqual(suite.stability_reasons([clean], suite.DEFAULT_POLICY, gpu=True), [])
        for value in (None, 12, float("nan"), True):
            with self.subTest(value=value):
                self.assertTrue(suite.stability_reasons([{**clean, "externalGpuPercent": value}], suite.DEFAULT_POLICY, gpu=True))

    def test_huge_numbers_fail_without_overflowing_admission(self):
        bad = synthetic_component()
        bad["cases"][0]["metrics"][0]["samples"][0] = 10**400
        with self.assertRaises(ValueError):
            suite.validate_component(bad, "native", "measure", {"dispatch.light"}, 3, 15)

    def test_interrupted_preflight_keeps_collected_observations(self):
        observations = []
        with patch.object(suite.time, "sleep", side_effect=[None, KeyboardInterrupt()]):
            samples, reasons = suite.preflight(FakeHost(), suite.DEFAULT_POLICY, samples=observations)
        self.assertIs(samples, observations)
        self.assertEqual(len(samples), 1)
        self.assertTrue(any("interrupted" in reason for reason in reasons))


def synthetic_component(mode="measure"):
    case = {"id": "dispatch.light", "parameters": {"calls": 16},
            "metrics": [{"name": "dispatch_roundtrip_mean", "unit": "ns", "better": "lower"}],
            "correctness": {"passed": True}}
    if mode == "measure":
        case["metrics"][0]["samples"] = [100] * 15
    elif mode == "verify":
        case["metrics"] = []
    return {"schemaVersion": 1, "suite": suite.SUITE, "component": "native", "mode": mode,
            "workloadVersion": 1, "warmups": 3 if mode == "measure" else 0,
            "repeats": 15 if mode == "measure" else (1 if mode == "verify" else 0),
            "cases": [case], "limitations": []}


class FakeHost:
    def __init__(self, gpu=False):
        self.ticks = 0
        self.closed = False

    def identity(self, refresh=False):
        return {"fingerprint": "synthetic-machine"}

    def snapshot(self, process=None):
        self.ticks += 1000
        return {"idleTicks": self.ticks, "kernelTicks": self.ticks, "userTicks": 0,
                "processTicks": 0, "availableMemoryBytes": 4 * 1024**3,
                "externalGpuPercent": 0}

    def close(self):
        self.closed = True


class FakeProcess:
    def __init__(self, timeout=False):
        self.returncode = None
        self.timeout = timeout
        self.killed = False

    def wait(self, timeout=None):
        if self.timeout and not self.killed:
            raise subprocess.TimeoutExpired("synthetic", timeout)
        self.returncode = -1 if self.killed else 0
        return self.returncode

    def poll(self):
        return self.returncode

    def kill(self):
        self.killed = True


class RunnerIntegration(unittest.TestCase):
    def setUp(self):
        root = SCRIPTS.parent / "target/benchmark-suite/unit-tests"
        root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=root)
        self.addCleanup(self.temp.cleanup)
        self.checkout = Path(self.temp.name) / "revenant"
        self.checkout.mkdir()
        self.descriptor = synthetic_component("describe")
        self.manifest = {"compiler": "synthetic-rustc", "cargo": "synthetic-cargo", "profile": "release",
                         "features": {"native": [], "graphics": []}, "source": {"digest": "synthetic"},
                         "drivers": {"native": {"descriptor": self.descriptor, "path": "unused", "sha256": "unused"}}}

    def fake_launch(self, entry, request, log, idle=False):
        self.assertTrue(idle)
        suite.write_json(Path(request["output"]), synthetic_component())
        return FakeProcess()

    def test_desktop_conditions_are_retained_in_the_measured_report(self):
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([{"externalCpuEstimatePercent": 12}], [])), \
             patch.object(suite, "launch_driver", side_effect=self.fake_launch):
            path, valid = suite.run(self.checkout, "prepared", "desktop-success", idle=True,
                                    host_factory=FakeHost, conditions="desktop", background_label="brave-playing")
        self.assertTrue(valid)
        policy = suite.read_json(path)["policy"]
        self.assertEqual(policy["controlProfile"], "desktop")
        self.assertEqual(policy["backgroundCondition"], "brave-playing")

    def test_busy_desktop_collects_samples_and_preserves_condition_warnings(self):
        class BusyHost(FakeHost):
            def snapshot(self, process=None):
                value = super().snapshot(process)
                value["idleTicks"] = self.ticks // 4
                value["externalGpuPercent"] = 80
                return value

        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite.time, "sleep"), \
             patch.object(suite, "launch_driver", side_effect=self.fake_launch):
            path, valid = suite.run(self.checkout, "prepared", "busy-desktop", idle=True,
                                    host_factory=BusyHost, conditions="desktop", background_label="brave-playing")
        self.assertTrue(valid)
        report = suite.read_json(path)
        self.assertEqual(report["invalidationReasons"], [])
        self.assertTrue(any("CPU" in warning for warning in report["conditionWarnings"]))
        self.assertEqual(len(report["observations"][0]["preflight"]), 10)
        self.assertEqual(report["observations"][0]["execution"][0]["externalCpuEstimatePercent"], 75)

    def test_quickly_finishing_child_gets_a_full_final_observation_interval(self):
        request = suite.driver_request("measure", self.checkout / "raw.json",
                                       self.checkout / "work", ["dispatch.light"])
        with patch.object(suite, "launch_driver", side_effect=self.fake_launch), \
             patch.object(suite.time, "monotonic", return_value=100), \
             patch.object(suite.time, "sleep") as sleep:
            result, samples, reasons = suite.measured_case(
                self.manifest["drivers"]["native"], request, self.checkout / "driver.log",
                FakeHost(), suite.DEFAULT_POLICY)
        sleep.assert_called_once_with(1)
        self.assertEqual(reasons, [])
        self.assertEqual(len(samples), 1)
        self.assertEqual(result["cases"][0]["correctness"]["passed"], True)

    def test_success_preserves_raw_samples_without_running_real_work(self):
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([{"externalCpuEstimatePercent": 0}], [])), \
             patch.object(suite, "launch_driver", side_effect=self.fake_launch):
            path, valid = suite.run(self.checkout, "prepared", "synthetic-success", idle=True, host_factory=FakeHost)
        self.assertTrue(valid)
        report = suite.read_json(path)
        self.assertEqual(report["components"][0]["cases"][0]["metrics"][0]["samples"], [100] * 15)
        self.assertTrue((path.parent / "dispatch-light/raw.json").is_file())

    def test_idle_refusal_precedes_loading_or_creating_files(self):
        with patch.object(suite, "load_bundle") as loader:
            with self.assertRaises(ValueError):
                suite.run(self.checkout, "prepared", "refused", host_factory=FakeHost)
            loader.assert_not_called()
        self.assertFalse(suite.artifact_root(self.checkout).exists())

    def test_busy_preflight_preserves_invalid_report_without_starting_driver(self):
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([{"externalCpuEstimatePercent": 25}], ["External CPU busy"])), \
             patch.object(suite, "launch_driver") as launch:
            path, valid = suite.run(self.checkout, "prepared", "synthetic-busy", idle=True, host_factory=FakeHost)
            launch.assert_not_called()
        self.assertFalse(valid)
        report = suite.read_json(path)
        self.assertIn("busy", report["invalidationReasons"][0])
        self.assertEqual(report["observations"][0]["preflight"][0]["externalCpuEstimatePercent"], 25)

    def test_partial_output_is_retained_but_invalid(self):
        def partial(entry, request, log, idle=False):
            Path(request["output"]).write_text('{"cases":', encoding="utf-8")
            return FakeProcess()
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([], [])), \
             patch.object(suite, "launch_driver", side_effect=partial):
            path, valid = suite.run(self.checkout, "prepared", "synthetic-partial", idle=True, host_factory=FakeHost)
        self.assertFalse(valid)
        self.assertTrue((path.parent / "dispatch-light/raw.json").is_file())
        self.assertTrue(any("malformed" in reason for reason in suite.read_json(path)["invalidationReasons"]))

    def test_timeout_kills_only_owned_driver_and_preserves_invalid_report(self):
        process = FakeProcess(timeout=True)
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([], [])), \
             patch.object(suite, "launch_driver", return_value=process), \
             patch.object(suite.time, "monotonic", side_effect=[0, 0, 0, 601]):
            path, valid = suite.run(self.checkout, "prepared", "synthetic-timeout", idle=True, host_factory=FakeHost)
        self.assertFalse(valid)
        self.assertTrue(process.killed)
        self.assertTrue(any("timeout" in reason for reason in suite.read_json(path)["invalidationReasons"]))

    def test_changed_workload_cannot_become_valid(self):
        def changed(entry, request, log, idle=False):
            report = synthetic_component()
            report["cases"][0]["parameters"]["calls"] = 17
            suite.write_json(Path(request["output"]), report)
            return FakeProcess()
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([], [])), \
             patch.object(suite, "launch_driver", side_effect=changed):
            path, valid = suite.run(self.checkout, "prepared", "synthetic-changed", idle=True, host_factory=FakeHost)
        self.assertFalse(valid)
        self.assertTrue(any("descriptor" in reason for reason in suite.read_json(path)["invalidationReasons"]))

    def test_cleanup_failure_does_not_lose_invalid_report(self):
        class BrokenCleanup(FakeHost):
            def close(self):
                raise OSError("synthetic cleanup error")
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([], ["busy"])):
            path, valid = suite.run(self.checkout, "prepared", "synthetic-cleanup", idle=True, host_factory=BrokenCleanup)
        self.assertFalse(valid)
        self.assertTrue(any("cleanup" in reason for reason in suite.read_json(path)["invalidationReasons"]))

    def test_changed_machine_identity_invalidates_collected_results(self):
        class ChangedMachine(FakeHost):
            def identity(self, refresh=False):
                return {"fingerprint": "changed" if refresh else "initial"}
        with patch.object(suite, "load_bundle", return_value=(self.checkout, self.manifest)), \
             patch.object(suite, "preflight", return_value=([], [])), \
             patch.object(suite, "launch_driver", side_effect=self.fake_launch):
            path, valid = suite.run(self.checkout, "prepared", "synthetic-machine", idle=True, host_factory=ChangedMachine)
        self.assertFalse(valid)
        self.assertEqual(suite.read_json(path)["finalMachine"]["fingerprint"], "changed")

    def test_wrong_json_shapes_are_rejected_as_values(self):
        for field, value in (("metrics", [7]), ("correctness", []), ("id", {})):
            bad = synthetic_component()
            bad["cases"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                suite.validate_component(bad, "native", "measure", {"dispatch.light"}, 3, 15)

    def test_interrupted_verification_settles_owned_driver(self):
        class InterruptedProcess(FakeProcess):
            def wait(self, timeout=None):
                if not self.killed:
                    raise KeyboardInterrupt()
                return super().wait(timeout)
        process = InterruptedProcess()
        request = suite.driver_request("verify", self.checkout / "out.json", self.checkout / "work", ["dispatch.light"])
        with patch.object(suite, "launch_driver", return_value=process), \
             patch.object(suite, "own_process", return_value=None):
            with self.assertRaises(KeyboardInterrupt):
                suite.protocol_call({}, request, self.checkout / "verify.log")
        self.assertTrue(process.killed)
        self.assertIsNotNone(process.returncode)

    def test_existing_label_is_never_overwritten(self):
        root = suite.artifact_root(self.checkout)
        path = suite.new_directory(root, "reserved")
        sentinel = path / "keep.txt"
        sentinel.write_text("untouched", encoding="utf-8")
        with self.assertRaises(FileExistsError):
            suite.new_directory(root, "reserved")
        self.assertEqual(sentinel.read_text(encoding="utf-8"), "untouched")

    def test_comparison_writes_only_in_grouped_artifact_directory(self):
        wrapper = {"schemaVersion": 1, "suite": suite.SUITE, "mode": "measure", "valid": True,
                   "invalidationReasons": [], "label": "synthetic", "workloadDigest": "fixed",
                   "machine": {"fingerprint": "synthetic-machine"}, "build": self.manifest,
                   "policy": suite.DEFAULT_POLICY, "components": [synthetic_component()]}
        root = suite.artifact_root(self.checkout)
        root.mkdir(parents=True)
        baseline, candidate = root / "baseline.json", root / "candidate.json"
        suite.write_json(baseline, wrapper)
        wrapper = copy.deepcopy(wrapper)
        wrapper["components"][0]["cases"][0]["metrics"][0]["samples"] = [80] * 15
        suite.write_json(candidate, wrapper)
        output = suite.compare(self.checkout, baseline, candidate, "synthetic-comparison")
        self.assertEqual(output.parent.parent, root)
        self.assertIn("improvement", output.read_text(encoding="utf-8"))
        with self.assertRaises(ValueError):
            suite.compare(self.checkout, self.checkout / "outside.json", candidate, "escape")

    def test_preparation_rejects_source_drift_and_leaves_no_admitted_manifest(self):
        (self.checkout / "Cargo.lock").write_text("synthetic", encoding="utf-8")
        (self.checkout / "Cargo.toml").write_text('[workspace]\nmembers=[]\n', encoding="utf-8")
        executable = Path(sys.executable)
        artifact = suite.canonical({"reason": "compiler-artifact", "target": {"name": "optimization_benchmarks"},
                                    "executable": str(executable), "profile": {"opt_level": "3"}}) + "\n"
        class FakeBuild(FakeProcess):
            def __init__(self):
                import io
                super().__init__()
                self.stdout = io.StringIO(artifact)
        def describe(entry, request, log_path, timeout=600):
            report = synthetic_component("describe")
            component = Path(entry["path"]).stem
            report["component"] = component
            if component == "graphics":
                report["cases"][0]["id"] = "graphics.scene.replace"
            return report
        with patch.object(suite, "source_identity", side_effect=[{"digest": "before"}, {"digest": "after"}]), \
             patch.object(suite, "command_output", return_value="synthetic"), \
             patch.object(suite.subprocess, "Popen", side_effect=lambda *args, **kwargs: FakeBuild()), \
             patch.object(suite, "own_process", return_value=None), \
             patch.object(suite, "protocol_call", side_effect=describe):
            with self.assertRaisesRegex(ValueError, "changed"):
                suite.prepare(self.checkout, "synthetic-drift")
        self.assertFalse((suite.build_root(self.checkout) / "synthetic-drift/manifest.json").exists())

    def test_compiler_override_refusal_precedes_build_side_effects(self):
        with patch.dict(suite.os.environ, {"RUSTC": "different-compiler"}):
            with self.assertRaisesRegex(ValueError, "overrides"):
                suite.prepare(self.checkout, "refused-compiler")
        self.assertFalse(suite.build_root(self.checkout).exists())

    def test_interrupted_execution_keeps_samples_and_settles_child(self):
        class InterruptedAfterSample(FakeProcess):
            def __init__(self):
                super().__init__()
                self.waits = 0
            def wait(self, timeout=None):
                if self.killed:
                    return super().wait(timeout)
                self.waits += 1
                if self.waits == 1:
                    raise subprocess.TimeoutExpired("synthetic", timeout)
                raise KeyboardInterrupt()
        process = InterruptedAfterSample()
        observations = []
        request = suite.driver_request("measure", self.checkout / "raw.json", self.checkout / "work", ["dispatch.light"])
        with patch.object(suite, "launch_driver", return_value=process):
            _, samples, reasons = suite.measured_case({}, request, self.checkout / "driver.log", FakeHost(), suite.DEFAULT_POLICY, observations)
        self.assertIs(samples, observations)
        self.assertEqual(len(samples), 1)
        self.assertTrue(process.killed)
        self.assertTrue(any("interrupted" in reason for reason in reasons))


if __name__ == "__main__":
    unittest.main()
