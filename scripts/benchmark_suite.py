"""Prepare, verify, diagnose and compare controlled Revenant optimization benchmarks.

Measurement is intentionally separate from building and functional verification.
Only the run command requires explicit authorization of its machine conditions.
"""

import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib
from datetime import datetime, timezone

SUITE = "revenant-optimization"
CHECKOUT = Path(__file__).resolve().parents[1]
DEFAULT_POLICY = {
    "warmups": 3, "repeats": 15, "preflightSeconds": 10,
    "sampleIntervalSeconds": 1, "maxExternalCpuPercent": 5,
    "minAvailableMemoryBytes": 2 * 1024**3, "maxExternalGpuPercent": 5,
    "caseTimeoutSeconds": 600,
    "finalObservation": "full-sampling-interval-including-post-exit-tail",
    "utilizationEnforcement": "reject",
}
DRIVERS = {"native": "revenant-desktop", "graphics": "revenant-graphics"}
TEST_ARGS = ["--ignored", "--exact", "optimization_benchmark_protocol",
             "--nocapture", "--test-threads=1"]


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)


def digest(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def read_json(path):
    def reject(value):
        raise ValueError(f"Non-finite JSON number: {value}")
    return json.loads(path.read_text(encoding="utf-8"), parse_constant=reject)


def finite_json(value):
    """Keep failed observer evidence serializable; invalid numbers remain unknown."""
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if isinstance(value, dict):
        return {k: finite_json(v) for k, v in value.items()}
    if isinstance(value, list):
        return [finite_json(v) for v in value]
    return value


def finite_number(value):
    if type(value) not in (int, float):
        return False
    try:
        return math.isfinite(value)
    except OverflowError:
        return False


def require_idle(confirmed):
    if confirmed is not True:
        raise ValueError("run requires --idle-confirmed after the user authorizes the declared machine conditions")


def condition_policy(profile="quiet", background_label=None, cpu_limit=None, gpu_limit=None):
    """Explicit, comparable machine conditions; quiet defaults never change."""
    if profile not in ("quiet", "desktop"):
        raise ValueError("Unknown machine condition profile")
    if profile == "desktop":
        validate_label(background_label)
    elif background_label is not None:
        raise ValueError("A background label requires desktop conditions")
    if profile == "quiet" and (cpu_limit is not None or gpu_limit is not None):
        raise ValueError("Custom external limits require desktop conditions")
    policy = copy.deepcopy(DEFAULT_POLICY)
    policy.update(controlProfile=profile, backgroundCondition=background_label or "none")
    if profile == "desktop":
        policy.update(maxExternalCpuPercent=20, maxExternalGpuPercent=20,
                      utilizationEnforcement="record-only")
        for key, limit in (("maxExternalCpuPercent", cpu_limit), ("maxExternalGpuPercent", gpu_limit)):
            if limit is not None:
                if not finite_number(limit) or not 0 <= limit <= 100:
                    raise ValueError("External utilization limits must be finite percentages from 0 to 100")
                policy[key] = limit
    return policy


def validate_label(value):
    reserved = {"CON", "PRN", "AUX", "NUL", *[f"{prefix}{n}" for prefix in ("COM", "LPT") for n in range(1, 10)]}
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", value) or value.upper() in reserved:
        raise ValueError("label must be 1–64 letters/digits/underscores/hyphens, without reserved Windows names")
    return value


def artifact_root(checkout):
    return checkout.parent / "revenant-source/acceptance/benchmarks"


def build_root(checkout):
    return checkout / "target/benchmark-suite/prepared"


def contained(path, root):
    path, root = Path(path).resolve(), Path(root).resolve()
    if not path.is_relative_to(root) or path == root:
        raise ValueError(f"Path escapes artifact directory: {path}")
    return path


def new_directory(root, label):
    validate_label(label)
    path = contained(root / label, root)
    path.mkdir(parents=True, exist_ok=False)
    return path


def file_hash(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def verify_executable(entry):
    path = Path(entry["path"])
    if not path.is_file() or file_hash(path) != entry["sha256"]:
        raise ValueError(f"Prepared executable hash mismatch: {path}")


def command_output(args, checkout=CHECKOUT):
    return subprocess.check_output(args, cwd=checkout, text=True, encoding="utf-8", errors="replace").strip()


def source_identity(checkout):
    paths = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=checkout).decode("utf-8").split("\0")
    entries = []
    for relative in sorted(set(paths) - {""}):
        if "__pycache__" in Path(relative).parts or relative.endswith((".pyc", ".pyo")):
            continue
        path = checkout / relative
        entries.append([relative, file_hash(path) if path.is_file() else "deleted"])
    return {"gitHead": command_output(["git", "rev-parse", "HEAD"], checkout),
            "dirty": bool(command_output(["git", "status", "--porcelain"], checkout)),
            "digest": digest(entries), "fileCount": len(entries)}


def cpu_interval(previous, current):
    differences = {key: current[key] - previous[key] for key in ("idleTicks", "kernelTicks", "userTicks", "processTicks")}
    total = differences["kernelTicks"] + differences["userTicks"]
    if total <= 0 or min(differences.values()) < 0 or differences["idleTicks"] > total:
        raise ValueError("CPU counters unavailable or reversed")
    busy = total - differences["idleTicks"]
    process = differences["processTicks"]
    if process > total:
        raise ValueError("Process CPU counters exceed the sampled system interval")
    return {"systemCpuPercent": 100 * busy / total,
            "benchmarkCpuPercent": 100 * process / total,
            "externalCpuEstimatePercent": max(0, 100 * (busy - process) / total)}


def condition_findings(samples, policy, gpu=False):
    """Separate invalid evidence from explicitly permitted desktop utilization."""
    reasons = set()
    warnings = set()
    record_only = policy.get("utilizationEnforcement", "reject") == "record-only"
    if not samples:
        reasons.add("No stability observations")
    for sample in samples:
        for field, bound, minimum in (
            ("externalCpuEstimatePercent", policy["maxExternalCpuPercent"], False),
            ("availableMemoryBytes", policy["minAvailableMemoryBytes"], True),
        ):
            value = sample.get(field)
            if not finite_number(value) or value < 0 or (not minimum and value > 100):
                reasons.add(f"Unavailable {field}")
            elif (minimum and value < bound) or (not minimum and value > bound):
                if record_only and not minimum:
                    warnings.add(f"External CPU utilization exceeds diagnostic threshold {bound}%")
                else:
                    reasons.add(f"{field} outside stability limit {bound}")
        if gpu:
            value = sample.get("externalGpuPercent")
            if not finite_number(value) or not 0 <= value <= 100:
                reasons.add("Unavailable externalGpuPercent")
            elif value > policy["maxExternalGpuPercent"]:
                if record_only:
                    warnings.add(f"External GPU utilization exceeds diagnostic threshold {policy['maxExternalGpuPercent']}%")
                else:
                    reasons.add("External GPU work exceeds stability limit")
        if sample.get("error"):
            reasons.add(f"Observer error: {sample['error']}")
    return sorted(reasons), sorted(warnings)


def stability_reasons(samples, policy, gpu=False):
    return condition_findings(samples, policy, gpu)[0]


def condition_warnings(samples, policy, gpu=False):
    return condition_findings(samples, policy, gpu)[1]


def validate_component(report, component, mode, expected_cases, warmups, repeats, descriptors=None):
    expected = {"schemaVersion": 1, "suite": SUITE, "component": component,
                "mode": mode, "workloadVersion": 1, "warmups": warmups, "repeats": repeats}
    if not isinstance(report, dict) or any(type(report.get(k)) is not type(v) or report.get(k) != v for k, v in expected.items()):
        raise ValueError("Component report identity/mode/repetition mismatch")
    cases = report.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError("Missing component cases")
    ids = [case.get("id") for case in cases if isinstance(case, dict)]
    if any(not isinstance(case_id, str) or not case_id for case_id in ids):
        raise ValueError("Malformed case ID")
    if len(ids) != len(cases) or len(set(ids)) != len(ids) or set(ids) != set(expected_cases):
        raise ValueError("Missing, duplicate or unexpected case IDs")
    for case in cases:
        if not isinstance(case.get("parameters"), dict) or case.get("requiredProfile", "standard") not in ("standard", "gpu"):
            raise ValueError("Malformed case parameters/profile")
        canonical(case["parameters"])
        metrics = case.get("metrics")
        if not isinstance(metrics, list) or (mode == "measure" and not metrics):
            raise ValueError("Missing metric definitions")
        if mode == "verify" and metrics:
            raise ValueError("Functional verification cannot contain timing or allocation samples")
        if mode == "diagnose":
            if component != "native" or not re.fullmatch(r"task\.progress\.bytes(0|1024|60000)\.listeners(0|1|8)", case["id"]) or metrics:
                raise ValueError("Allocation diagnosis only accepts sample-free core progress cases")
            observations = case.get("rawObservations")
            if not isinstance(observations, list) or len(observations) != 1:
                raise ValueError("Diagnosis requires one separate allocation observation")
            observation = observations[0]
            fields = {"kind", "iterations", "allocationCalls", "cumulativeRequestedBytes"}
            if not isinstance(observation, dict) or set(observation) != fields or observation["kind"] != "allocation-diagnosis" or type(observation["iterations"]) is not int or observation["iterations"] != 100:
                raise ValueError("Malformed diagnosis or changed verification workload")
            if any(type(observation[key]) is not int or observation[key] < 0 for key in ("allocationCalls", "cumulativeRequestedBytes")):
                raise ValueError("Allocation observations must be nonnegative integer counts")
        if mode != "describe" and (not isinstance(case.get("correctness"), dict) or case["correctness"].get("passed") is not True):
            raise ValueError("Correctness did not pass")
        if mode == "measure" and case.get("requiredProfile") == "gpu":
            hardware = case.get("hardwareIdentity")
            if not isinstance(hardware, dict) or any(not isinstance(hardware.get(k), str) or not hardware[k] for k in ("adapter", "backend")):
                raise ValueError("GPU measurement requires actual selected hardware identity")
        names = set()
        for metric in metrics:
            if not isinstance(metric, dict):
                raise ValueError("Metric must be an object")
            name = metric.get("name")
            if not isinstance(name, str) or not name or name in names or metric.get("unit") not in ("ns", "count", "bytes") or metric.get("better") not in ("lower", "higher"):
                raise ValueError("Malformed/duplicate metric definition")
            names.add(name)
            if mode == "describe":
                if metric.get("samples"):
                    raise ValueError("Describe cannot contain samples")
                continue
            samples = metric.get("samples")
            if not isinstance(samples, list) or len(samples) != repeats:
                raise ValueError("Unexpected sample count")
            if any(not finite_number(x) or x < 0 or (metric["unit"] == "ns" and x == 0) for x in samples):
                raise ValueError("Samples must be finite, nonnegative numbers; timings must be positive")
        if descriptors is not None and mode == "measure":
            reference = descriptors[case["id"]]
            observed = [(m["name"], m["unit"], m["better"]) for m in metrics]
            fixed = [(m["name"], m["unit"], m["better"]) for m in reference["metrics"]]
            if canonical(case["parameters"]) != canonical(reference["parameters"]) or observed != fixed or case.get("requiredProfile", "standard") != reference.get("requiredProfile", "standard"):
                raise ValueError("Measured workload differs from prepared descriptor")


def driver_request(mode, output, work, cases, profile="standard"):
    return {"mode": mode, "output": str(output.resolve()), "workDirectory": str(work.resolve()),
            "warmups": 3, "repeats": 15, "profile": profile, "cases": cases}


def launch_driver(entry, request, log, idle=False):
    verify_executable(entry)
    work = Path(request["workDirectory"])
    work.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["REVENANT_BENCHMARK_REQUEST"] = canonical(request)
    env.pop("REVENANT_BENCHMARK_IDLE_CONFIRMED", None)
    if idle:
        env["REVENANT_BENCHMARK_IDLE_CONFIRMED"] = "1"
    for key in ("TMP", "TEMP", "TMPDIR"):
        env[key] = str(work)
    # Child execution starts only after own_process/WindowsHost.own assigns its
    # kill-on-close job. Even describe/diagnose drivers can exit before assignment.
    flags = 0x00000004 if os.name == "nt" else 0  # CREATE_SUSPENDED
    return subprocess.Popen([entry["path"], *TEST_ARGS], cwd=work, env=env,
                            stdout=log, stderr=subprocess.STDOUT, creationflags=flags)


def own_process(process):
    if os.name == "nt":
        from benchmark_host import OwnedJob
        return OwnedJob(process, resume=True)
    return None


def settle_process(process, job=None):
    try:
        if job is not None:
            job.close()
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()


def protocol_call(entry, request, log_path, timeout=600):
    with log_path.open("wb") as log:
        process = launch_driver(entry, request, log)
        job = None
        try:
            job = own_process(process)
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            raise ValueError("Driver functional/describe timeout") from None
        finally:
            settle_process(process, job)
    if process.returncode:
        raise ValueError(f"Driver exited {process.returncode}; see {log_path}")
    return read_json(Path(request["output"]))


def load_bundle(checkout, label):
    directory = contained(build_root(checkout) / validate_label(label), build_root(checkout))
    manifest = read_json(directory / "manifest.json")
    if manifest.get("schemaVersion") != 1 or manifest.get("suite") != SUITE or manifest.get("label") != label:
        raise ValueError("Invalid prepared manifest identity")
    if set(manifest.get("drivers", {})) != set(DRIVERS):
        raise ValueError("Prepared manifest must include native and graphics drivers")
    for name, entry in manifest["drivers"].items():
        contained(entry["path"], directory)
        verify_executable(entry)
        descriptors = entry["descriptor"]
        ids = {case["id"] for case in descriptors["cases"]}
        validate_component(descriptors, name, "describe", ids, 0, 0)
    return directory, manifest


def prepare(checkout, label, gpu=False):
    compiler_keys = ("RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC",
                     "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
    if any(os.environ.get(key) for key in compiler_keys):
        raise ValueError("Compiler/wrapper overrides are unsupported by the controlled v1 profile")
    directory = new_directory(build_root(checkout), label)
    before = source_identity(checkout)
    manifest = {"schemaVersion": 1, "suite": SUITE, "label": label, "drivers": {},
                "compiler": command_output(["rustc", "-vV"], checkout),
                "cargo": command_output(["cargo", "-V"], checkout),
                "profile": "release", "features": {"native": [], "graphics": ["gpu"] if gpu else []},
                "source": before, "lockfileSha256": file_hash(checkout / "Cargo.lock")}
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    configurations = [path / ".cargo" / name for path in (checkout, *checkout.parents)
                      for name in ("config.toml", "config")]
    configurations += [cargo_home / "config.toml", cargo_home / "config"]
    manifest["configuration"] = {
        "environment": {key: value for key, value in sorted(os.environ.items())
                        if key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN") or key.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "CARGO_PROFILE_"))},
        "cargoConfigHashes": [file_hash(path) if path.is_file() else None for path in configurations],
        "profiles": tomllib.loads((checkout / "Cargo.toml").read_text(encoding="utf-8")).get("profile", {}),
        "artifactProfiles": {},
        "resolvedFeatures": {},
    }
    for path in configurations:
        if path.is_file():
            settings = tomllib.loads(path.read_text(encoding="utf-8"))
            build = settings.get("build", {})
            if any(build.get(key) for key in ("rustc", "rustc-wrapper", "rustc-workspace-wrapper")):
                raise ValueError("Cargo-config compiler/wrapper overrides are unsupported by the controlled v1 profile")
    for component, package in DRIVERS.items():
        args = ["cargo", "test", "-p", package, "--test", "optimization_benchmarks",
                "--release", "--locked", "--no-run", "--message-format=json"]
        if component == "native":
            args.append("--no-default-features")
        if component == "graphics" and gpu:
            args += ["--features", "gpu"]
        manifest.setdefault("commands", {})[component] = args
        artifacts = []
        with (directory / f"{component}-build.log").open("w", encoding="utf-8") as log:
            process = subprocess.Popen(args, cwd=checkout, stdout=subprocess.PIPE, stderr=log, text=True, encoding="utf-8", errors="replace")
            job = None
            try:
                job = own_process(process)
                for line in process.stdout:
                    log.write(line)
                    try:
                        value = json.loads(line)
                    except ValueError:
                        continue
                    if value.get("reason") == "compiler-artifact" and value.get("target", {}).get("name") == "optimization_benchmarks" and value.get("executable"):
                        artifacts.append(Path(value["executable"]))
                        manifest["configuration"]["artifactProfiles"][component] = value.get("profile")
                        manifest["configuration"]["resolvedFeatures"][component] = sorted(value.get("features", []))
                process.wait()
            finally:
                settle_process(process, job)
                process.stdout.close()
        if process.returncode or len(artifacts) != 1:
            raise ValueError(f"{component} build failed or executable ambiguous; see {directory}")
        destination = directory / (component + artifacts[0].suffix)
        shutil.copy2(artifacts[0], destination)
        entry = {"path": str(destination), "sha256": file_hash(destination)}
        request = driver_request("describe", directory / f"{component}-descriptor.json", directory / "describe-work" / component, [])
        descriptor = protocol_call(entry, request, directory / f"{component}-describe.log")
        validate_component(descriptor, component, "describe", {case["id"] for case in descriptor["cases"]}, 0, 0)
        entry["descriptor"] = descriptor
        manifest["drivers"][component] = entry
    after = source_identity(checkout)
    if before != after or manifest["configuration"]["cargoConfigHashes"] != [file_hash(path) if path.is_file() else None for path in configurations]:
        raise ValueError("Source or Cargo configuration changed during preparation; discard this label and prepare again")
    write_json(directory / "manifest.json", manifest)
    return directory / "manifest.json"


def verify(checkout, label):
    directory, manifest = load_bundle(checkout, label)
    output = new_directory(checkout / "target/benchmark-suite/verification", label)
    components = []
    for component, entry in manifest["drivers"].items():
        ids = [case["id"] for case in entry["descriptor"]["cases"] if case.get("requiredProfile", "standard") == "standard"]
        request = driver_request("verify", output / f"{component}.json", output / "work" / component, ids)
        report = protocol_call(entry, request, output / f"{component}.log")
        validate_component(report, component, "verify", set(ids), 0, 1)
        components.append(report)
    write_json(output / "verification.json", {"schemaVersion": 1, "suite": SUITE,
               "mode": "verify", "valid": True, "build": manifest, "components": components})
    return output / "verification.json"


def diagnose(checkout, label, output_label=None):
    """Allocation-only investigation; never samples elapsed time or machine utilization."""
    _, manifest = load_bundle(checkout, label)
    entry = manifest["drivers"]["native"]
    ids = [case["id"] for case in entry["descriptor"]["cases"] if case["id"].startswith("task.progress.")]
    if len(ids) != 9:
        raise ValueError("Prepared bundle does not contain the complete progress diagnosis matrix")
    output = new_directory(checkout / "target/benchmark-suite/diagnosis", output_label or label)
    request = driver_request("diagnose", output / "native.json", output / "work/native", ids)
    report = protocol_call(entry, request, output / "native.log")
    validate_component(report, "native", "diagnose", set(ids), 0, 1)
    write_json(output / "diagnosis.json", {"schemaVersion": 1, "suite": SUITE,
               "mode": "diagnose", "valid": True, "build": manifest, "components": [report],
               "limitations": ["Allocation calls and cumulative requested bytes are not retained memory or speed evidence."]})
    return output / "diagnosis.json"


def selected_cases(manifest, requested, gpu):
    all_cases = [(name, case) for name, entry in manifest["drivers"].items() for case in entry["descriptor"]["cases"]]
    ids = [case["id"] for _, case in all_cases]
    if len(set(ids)) != len(ids):
        raise ValueError("Duplicate prepared case IDs")
    if len(set(requested)) != len(requested) or set(requested) - set(ids):
        raise ValueError("Duplicate or unknown --case")
    selected = [(name, case) for name, case in all_cases if (not requested or case["id"] in requested) and (gpu or case.get("requiredProfile", "standard") == "standard")]
    if not selected or (requested and {case["id"] for _, case in selected} != set(requested)):
        raise ValueError("Requested cases require --gpu and a GPU prepared bundle")
    if gpu and not any(case.get("requiredProfile") == "gpu" for _, case in selected):
        raise ValueError("--gpu requires a prepared GPU case selection")
    return sorted(selected, key=lambda item: (item[0], item[1]["id"]))


def sample_interval(host, previous, process=None):
    current = host.snapshot(process)
    observation = {**cpu_interval(previous, current), **{k: v for k, v in current.items() if k not in ("idleTicks", "kernelTicks", "userTicks", "processTicks")},
                   "timestamp": datetime.now(timezone.utc).isoformat()}
    return current, observation


def preflight(host, policy, gpu=False, samples=None):
    samples = [] if samples is None else samples
    reasons = []
    try:
        previous = host.snapshot()
        for _ in range(policy["preflightSeconds"]):
            time.sleep(policy["sampleIntervalSeconds"])
            previous, observed = sample_interval(host, previous)
            samples.append(observed)
    except (Exception, KeyboardInterrupt) as error:
        reasons.append(f"Preflight failed/interrupted: {type(error).__name__}: {error}")
    return samples, sorted(set(reasons + stability_reasons(samples, policy, gpu)))


def measured_case(entry, request, log_path, host, policy, observations=None):
    """Only called by run after acknowledgement; synthetic tests substitute its host/driver."""
    observations = [] if observations is None else observations
    reasons = []
    with log_path.open("wb") as log:
        previous = host.snapshot()
        process = launch_driver(entry, request, log, idle=True)
        started = time.monotonic()
        job = None
        try:
            # Windows Job owns only this child and its descendants. Tests use fake hosts.
            if hasattr(host, "own"):
                job = host.own(process)
            while True:
                remaining = policy["caseTimeoutSeconds"] - (time.monotonic() - started)
                if remaining <= 0:
                    reasons.append("Driver timeout")
                    break
                interval_started = time.monotonic()
                try:
                    process.wait(timeout=min(policy["sampleIntervalSeconds"], remaining))
                except subprocess.TimeoutExpired:
                    pass
                if process.returncode is not None:
                    # GetSystemTimes is quantized. An immediate post-exit sample
                    # can have zero aggregate ticks or magnify a short burst.
                    # Keep the advertised cadence, observing the post-exit tail.
                    tail = policy["sampleIntervalSeconds"] - (time.monotonic() - interval_started)
                    if tail > 0:
                        time.sleep(tail)
                try:
                    previous, sample = sample_interval(host, previous, process)
                    observations.append(sample)
                except (OSError, ValueError) as error:
                    observations.append({"error": str(error)})
                    reasons.append(f"Observer failed: {error}")
                    break
                if process.returncode is not None:
                    break
            if reasons and process.poll() is None:
                process.kill()
                process.wait()
        except (Exception, KeyboardInterrupt) as error:
            reasons.append(f"Driver/observer failed/interrupted: {type(error).__name__}: {error}")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if job is not None:
                job.close()
        if process.returncode:
            reasons.append(f"Driver exited {process.returncode}")
    reasons.extend(stability_reasons(observations, policy, request["profile"] == "gpu"))
    try:
        component = read_json(Path(request["output"]))
    except (OSError, ValueError) as error:
        component = None
        reasons.append(f"Missing/malformed driver output: {error}")
    return component, observations, sorted(set(reasons))


def run(checkout, build_label, label, idle=False, gpu=False, cases=(), host_factory=None,
        conditions="quiet", background_label=None, cpu_limit=None, gpu_limit=None):
    require_idle(idle)  # Must precede even creating a report directory.
    policy = condition_policy(conditions, background_label, cpu_limit, gpu_limit)
    _, manifest = load_bundle(checkout, build_label)
    selected = selected_cases(manifest, list(cases), gpu)
    directory = new_directory(artifact_root(checkout), label)
    policy["gpuMonitoring"] = gpu
    report = {"schemaVersion": 1, "suite": SUITE, "mode": "measure", "valid": False,
              "invalidationReasons": [], "conditionWarnings": [], "label": label, "build": manifest, "policy": policy,
              "workloadDigest": digest([(name, case) for name, case in selected]),
              "components": [], "observations": [], "machine": {},
              "limitations": ["Sampled CPU/GPU stability checks do not establish absence of all interference or thermal throttling.",
                              "Observer overhead is included in conditions; short interference between samples can be missed.",
                              "Process memory and known allocations do not establish total driver/GPU memory."]}
    if conditions == "desktop":
        report["limitations"].append(
            "Desktop conditions record background CPU/GPU utilization without rejecting utilization alone. "
            "Timing differences include interference; matching labels depend on preserving the actual background workload. "
            "These measurements do not establish quiet-host performance or a utilization-adjusted implementation effect.")
    host = None
    try:
        if host_factory is None:
            from benchmark_host import WindowsHost
            host_factory = WindowsHost
        host = host_factory(gpu=gpu)
        report["machine"] = host.identity()
        for component, descriptor in selected:
            case_id = descriptor["id"]
            case_dir = new_directory(directory, case_id.replace(".", "-"))
            observation = {"component": component, "case": case_id, "preflight": [], "execution": []}
            report["observations"].append(observation)
            pre, reasons = preflight(host, policy, gpu, observation["preflight"])
            observation["preflight"] = pre
            report["conditionWarnings"] += [f"{case_id}: {warning}" for warning in condition_warnings(pre, policy, gpu)]
            if reasons:
                report["invalidationReasons"] += [f"{case_id}: {reason}" for reason in reasons]
                break
            profile = descriptor.get("requiredProfile", "standard")
            request = driver_request("measure", case_dir / "raw.json", case_dir / "work", [case_id], profile)
            write_json(case_dir / "request.json", request)
            result, samples, reasons = measured_case(manifest["drivers"][component], request, case_dir / "driver.log", host, policy, observation["execution"])
            observation["execution"] = samples
            report["conditionWarnings"] += [f"{case_id}: {warning}" for warning in condition_warnings(samples, policy, gpu)]
            # When GPU monitoring is selected, CPU cases still require GPU stability.
            reasons += stability_reasons(samples, policy, gpu)
            if result is not None:
                try:
                    validate_component(result, component, "measure", {case_id}, 3, 15, {case_id: descriptor})
                except (ValueError, KeyError, TypeError) as error:
                    reasons.append(str(error))
                else:
                    existing = next((item for item in report["components"] if item["component"] == component), None)
                    if existing is None:
                        report["components"].append(result)
                    else:
                        existing["cases"].extend(result["cases"])
            report["invalidationReasons"] += [f"{case_id}: {reason}" for reason in sorted(set(reasons))]
            if reasons:
                break
        hardware = [case["hardwareIdentity"] for component in report["components"] for case in component["cases"]
                    if case.get("requiredProfile") == "gpu"]
        if len({canonical(item) for item in hardware}) > 1:
            report["invalidationReasons"].append("Selected GPU adapter/backend changed across cases")
        report["conditionWarnings"] = sorted(set(report["conditionWarnings"]))
        report["valid"] = not report["invalidationReasons"] and sum(len(c["cases"]) for c in report["components"]) == len(selected)
        if not report["valid"] and not report["invalidationReasons"]:
            report["invalidationReasons"].append("Incomplete case execution")
        if report["valid"]:
            final_identity = host.identity(refresh=True)
            report["finalMachine"] = final_identity
            if final_identity["fingerprint"] != report["machine"]["fingerprint"]:
                report["valid"] = False
                report["invalidationReasons"].append("Machine identity changed during run")
        if report["valid"]:
            from benchmark_compare import compare_reports
            compare_reports(report, report)  # Apply the same admission rules as future comparisons.
    except (Exception, KeyboardInterrupt) as error:
        report["valid"] = False
        report["invalidationReasons"].append(f"Run interrupted/failed: {error}")
    finally:
        try:
            if host is not None:
                host.close()
        except (Exception, KeyboardInterrupt) as error:
            report["valid"] = False
            report["invalidationReasons"].append(f"Observer cleanup failed: {error}")
        finally:
            write_json(directory / "report.json", finite_json(report))
    return directory / "report.json", report["valid"]


def compare(checkout, baseline, candidate, label):
    from benchmark_compare import compare_reports, render_comparison
    root = artifact_root(checkout)
    baseline, candidate = contained(baseline, root), contained(candidate, root)
    result = compare_reports(read_json(baseline), read_json(candidate))
    directory = new_directory(root, label)
    result["inputs"] = {"baseline": {"path": str(baseline), "sha256": file_hash(baseline)},
                        "candidate": {"path": str(candidate), "sha256": file_hash(candidate)}}
    write_json(directory / "comparison.json", result)
    (directory / "comparison.md").write_text(render_comparison(result), encoding="utf-8")
    return directory / "comparison.md"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("prepare", "verify", "diagnose"):
        command = sub.add_parser(name)
        command.add_argument("build_label", type=validate_label)
        if name == "prepare":
            command.add_argument("--gpu", action="store_true")
        elif name == "diagnose":
            command.add_argument("--label", type=validate_label,
                                 help="fresh output label for another diagnosis of the same immutable bundle")
    command = sub.add_parser("run")
    command.add_argument("build_label", type=validate_label)
    command.add_argument("--label", required=True, type=validate_label)
    command.add_argument("--idle-confirmed", action="store_true")
    command.add_argument("--gpu", action="store_true")
    command.add_argument("--conditions", choices=("quiet", "desktop"), default="quiet")
    command.add_argument("--background-label", type=validate_label)
    command.add_argument("--max-external-cpu-percent", type=float,
                         help="desktop CPU diagnostic warning threshold; utilization does not reject desktop runs")
    command.add_argument("--max-external-gpu-percent", type=float,
                         help="desktop GPU diagnostic warning threshold; utilization does not reject desktop runs")
    command.add_argument("--case", action="append", default=[])
    command = sub.add_parser("paired", help="fresh-process A/B pairs under desktop load; CPU usage is metadata")
    command.add_argument("baseline_build", type=validate_label)
    command.add_argument("candidate_build", type=validate_label)
    command.add_argument("--label", required=True, type=validate_label)
    command.add_argument("--background-label", type=validate_label, default="normal-desktop")
    command.add_argument("--case", action="append", default=[])
    command = sub.add_parser("compare")
    command.add_argument("baseline", type=Path)
    command.add_argument("candidate", type=Path)
    command.add_argument("--label", required=True, type=validate_label)
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            result = prepare(CHECKOUT, args.build_label, args.gpu)
        elif args.command == "verify":
            result = verify(CHECKOUT, args.build_label)
        elif args.command == "diagnose":
            result = diagnose(CHECKOUT, args.build_label, args.label)
        elif args.command == "run":
            result, valid = run(CHECKOUT, args.build_label, args.label, args.idle_confirmed, args.gpu, args.case,
                                conditions=args.conditions, background_label=args.background_label,
                                cpu_limit=args.max_external_cpu_percent, gpu_limit=args.max_external_gpu_percent)
            print(result)
            return 0 if valid else 2
        elif args.command == "paired":
            from benchmark_paired import collect
            result, valid = collect(CHECKOUT, args.baseline_build, args.candidate_build, args.label,
                                    args.background_label, args.case)
            print(result)
            return 0 if valid else 2
        else:
            result = compare(CHECKOUT, args.baseline, args.candidate, args.label)
        print(result)
        return 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        print(f"Benchmark refused: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
