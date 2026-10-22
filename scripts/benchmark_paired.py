"""Fresh-process paired comparisons under ordinary desktop load.

The native v1 authorization token is retained to reuse frozen executables. It
authorizes measurement here; this collector makes no assertion that the host is
idle. Utilization telemetry never participates in evidence admission.
"""
from __future__ import annotations

import math
import random
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

import benchmark_suite as suite

PAIRS = 15
SEED = 378
RESAMPLES = 10_000
BLOCK_LENGTHS = (1, 2, 3)
COPY_CASES = [f"task.progress.bytes{size}.listeners{listeners}"
              for size in (0, 1024, 60000) for listeners in (0, 1, 8)] + ["dispatch.progress"]


def schedule():
    orders = [["baseline", "candidate"] for _ in range(8)]
    orders += [["candidate", "baseline"] for _ in range(7)]
    random.Random(SEED).shuffle(orders)
    return [{"index": index, "order": order} for index, order in enumerate(orders)]


def _percentile(values, fraction):
    position = (len(values) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    return values[lower] + (values[upper] - values[lower]) * (position - lower)


def _bootstrap_indices(resamples):
    if type(resamples) is not int or resamples < 1:
        raise ValueError("Bootstrap resamples must be positive")
    selections = {}
    rng = random.Random(SEED)
    for length in BLOCK_LENGTHS:
        rows = []
        for _ in range(resamples):
            indices = []
            while len(indices) < PAIRS:
                start = rng.randrange(PAIRS)
                indices.extend((start + offset) % PAIRS for offset in range(length))
            rows.append(indices[:PAIRS])
        selections[length] = rows
    return selections


def _ratio(old, new):
    denominator = statistics.median(old)
    return None if denominator == 0 else statistics.median(new) / denominator


def _cpu_summary(pairs):
    complete = []
    for pair in pairs:
        durations = [pair[side].get("processCpu") for side in ("baseline", "candidate")]
        if all(isinstance(value, dict) and all(type(value.get(key)) is int and value[key] >= 0
                   for key in ("kernelNs", "userNs", "totalNs")) and value["totalNs"] == value["kernelNs"] + value["userNs"]
               for value in durations):
            complete.append(pair)
    metrics = []
    for key in ("kernelNs", "userNs", "totalNs"):
        old = [pair["baseline"]["processCpu"][key] for pair in complete]
        new = [pair["candidate"]["processCpu"][key] for pair in complete]
        metrics.append({"metric": key, "classification": "diagnostic",
                        "baselineMedian": statistics.median(old) if old else None,
                        "candidateMedian": statistics.median(new) if new else None,
                        "candidateBaselineRatio": _ratio(old, new) if old else None})
    return {"completePairs": len(complete), "unavailablePairs": len(pairs) - len(complete),
            "boundary": "owned process lifetime including startup, warmups, counting, setup and teardown",
            "metrics": metrics}


def compare_pairs(pairs, descriptors, resamples=RESAMPLES):
    """Resample complete paired process summaries, never individual callbacks."""
    if not isinstance(pairs, list) or len(pairs) != PAIRS:
        raise ValueError("Paired comparison requires fifteen complete pairs")
    reference = {case["id"]: case for case in descriptors}
    if len(reference) != len(descriptors) or not reference:
        raise ValueError("Missing or duplicate paired descriptors")
    values = {}
    ab_count = 0
    for index, pair in enumerate(pairs):
        if not isinstance(pair, dict) or type(pair.get("index")) is not int or pair["index"] != index:
            raise ValueError("Missing, duplicate or unordered pair index")
        if pair.get("order") not in (["baseline", "candidate"], ["candidate", "baseline"]):
            raise ValueError("Invalid pair order")
        ab_count += pair["order"] == ["baseline", "candidate"]
        for side in ("baseline", "candidate"):
            result = pair.get(side)
            component = result.get("component") if isinstance(result, dict) else None
            suite.validate_component(component, "native", "measure", set(reference), 3, 15, reference)
            for case in component["cases"]:
                for metric in case["metrics"]:
                    key = (case["id"], metric["name"], metric["unit"], metric["better"])
                    record = values.setdefault(key, {"baseline": [], "candidate": [],
                                                      "raw": {"baseline": [], "candidate": []}})
                    record[side].append(statistics.median(metric["samples"]))
                    record["raw"][side].append(metric["samples"])
    if ab_count != 8:
        raise ValueError("Paired schedule must contain eight AB and seven BA pairs")

    selections = _bootstrap_indices(resamples)
    effects = []
    allocation_benefit = False
    timing_admitted = any(key[2] == "ns" for key in values)
    regression = False
    retention_bounded = True
    for key, record in values.items():
        old, new = record["baseline"], record["candidate"]
        ratio = _ratio(old, new)
        effect = {"case": key[0], "metric": key[1], "unit": key[2], "better": key[3],
                  "baselineMedian": statistics.median(old), "candidateMedian": statistics.median(new),
                  "absoluteChange": statistics.median(new) - statistics.median(old),
                  "candidateBaselineRatio": ratio, "classification": "diagnostic", "ratio95Envelope": None,
                  "chronological": [{"pair": index, "order": "AB" if pair["order"][0] == "baseline" else "BA",
                                     "baselineMedian": old[index], "candidateMedian": new[index]}
                                    for index, pair in enumerate(pairs)]}
        effect["orderEffects"] = {}
        for order in ("AB", "BA"):
            indices = [index for index, pair in enumerate(pairs)
                       if (pair["order"][0] == "baseline") == (order == "AB")]
            effect["orderEffects"][order] = {"pairs": len(indices),
                                             "candidateBaselineRatio": _ratio([old[i] for i in indices], [new[i] for i in indices])}
        if key[2] == "ns":
            if key[3] != "lower":
                raise ValueError("Paired v1 timing admission requires lower-is-better latency")
            intervals = []
            for length, indices_rows in selections.items():
                boot = sorted(_ratio([old[i] for i in indices], [new[i] for i in indices]) for indices in indices_rows)
                if any(value is None or not math.isfinite(value) for value in boot):
                    raise ValueError("Nonfinite paired timing ratio")
                intervals.append({"blockLength": length,
                                  "ratio95": [_percentile(boot, 0.025), _percentile(boot, 0.975)]})
            envelope = [min(row["ratio95"][0] for row in intervals), max(row["ratio95"][1] for row in intervals)]
            effect.update(ratio95Envelope=envelope, intervals=intervals,
                          classification="improvement" if envelope[1] < 0.95 else "regression" if envelope[0] > 1.05 else "inconclusive",
                          nonRegressionAdmitted=envelope[1] <= 1.05)
            timing_admitted &= envelope[1] <= 1.05
            regression |= envelope[0] > 1.05
        if key[1] in ("allocation_calls", "cumulative_requested_bytes"):
            allocation_benefit |= statistics.median(new) < statistics.median(old)
            retention_bounded &= all(candidate <= baseline for baseline, candidate in zip(old, new))
        if key[1] in ("retained_update_json", "retained_result_json", "active_scopes_after",
                      "retained_result_json_after", "known_scratch_after"):
            retention_bounded &= record["raw"]["baseline"] == record["raw"]["candidate"]
        effects.append(effect)
    decision = "reject" if regression or not retention_bounded else "adopt" if timing_admitted and allocation_benefit else "inconclusive"
    return {"kind": "paired-comparison", "decision": decision, "allocationBenefit": allocation_benefit,
            "retentionBounded": retention_bounded, "timingNonRegressionAdmitted": timing_admitted,
            "estimator": "ratio of medians of within-process medians",
            "bootstrap": {"seed": SEED, "resamples": resamples, "confidence": 0.95,
                          "blockLengths": list(BLOCK_LENGTHS), "method": "paired circular blocks; envelope across declared lengths"},
            "wholeProcessCpu": _cpu_summary(pairs), "metrics": effects}


def _snapshot(host, process, warnings):
    try:
        return host.snapshot(process)
    except (OSError, ValueError) as error:
        warnings.append(f"Optional utilization telemetry unavailable: {error}")
        return None


def _telemetry(previous, current, warnings):
    row = {"timestamp": datetime.now(timezone.utc).isoformat(), "systemCpuPercent": None,
           "benchmarkCpuPercent": None, "externalCpuEstimatePercent": None,
           "availableMemoryBytes": None, "workingSetBytes": None, "privateBytes": None}
    if current is not None:
        for key in ("availableMemoryBytes", "workingSetBytes", "privateBytes"):
            value = current.get(key)
            row[key] = value if suite.finite_number(value) and value >= 0 else None
    if previous is not None and current is not None:
        try:
            if any(not suite.finite_number(snapshot.get(key)) or snapshot[key] < 0
                   for snapshot in (previous, current)
                   for key in ("idleTicks", "kernelTicks", "userTicks", "processTicks")):
                raise ValueError("CPU counters must be finite, nonnegative numbers")
            interval = suite.cpu_interval(previous, current)
            if any(not suite.finite_number(value) or not 0 <= value <= 100 for value in interval.values()):
                raise ValueError("CPU percentages must be finite and within 0..100")
            row.update(interval)
        except (KeyError, ValueError, TypeError) as error:
            warnings.append(f"Optional utilization interval unavailable: {error}")
    return row


def execute_block(entry, request, log_path, host, policy):
    observations, warnings = [], []
    result = {"telemetry": observations, "warnings": warnings, "processCpu": None,
              "startedAt": datetime.now(timezone.utc).isoformat()}
    with log_path.open("wb") as log:
        previous = _snapshot(host, None, warnings)
        # Frozen v1 drivers require this token; the paired report declares desktop
        # authorization explicitly and never describes it as idle confirmation.
        process = suite.launch_driver(entry, request, log, idle=True)
        started = time.monotonic()
        job = None
        try:
            job = host.own(process)
            while process.poll() is None:
                remaining = policy["processTimeoutSeconds"] - (time.monotonic() - started)
                if remaining <= 0:
                    raise ValueError("Paired driver timeout")
                try:
                    process.wait(timeout=min(policy["sampleIntervalSeconds"], remaining))
                except subprocess.TimeoutExpired:
                    pass
                current = _snapshot(host, process, warnings)
                observations.append(_telemetry(previous, current, warnings))
                previous = current
            try:
                result["processCpu"] = host.process_times(process)
            except (OSError, ValueError) as error:
                warnings.append(f"Optional whole-process CPU time unavailable: {error}")
            if process.returncode:
                raise ValueError(f"Paired driver exited {process.returncode}; see {log_path}")
        finally:
            try:
                suite.settle_process(process, job)
            finally:
                result["finishedAt"] = datetime.now(timezone.utc).isoformat()
                suite.write_json(log_path.with_name("observations.json"), result)
    result["component"] = suite.read_json(Path(request["output"]))
    return result


def _admit_builds(checkout, builds, requested):
    for key in ("compiler", "cargo", "profile", "features", "lockfileSha256", "configuration"):
        if suite.canonical(builds["baseline"][key]) != suite.canonical(builds["candidate"][key]):
            raise ValueError(f"Incomparable paired {key}")
    selections = {}
    for side, manifest in builds.items():
        selected = suite.selected_cases(manifest, requested, False)
        if any(component != "native" for component, _ in selected):
            raise ValueError("Paired v1 accepts native standard cases only")
        selections[side] = [case for _, case in selected]
        verification = suite.read_json(checkout / "target/benchmark-suite/verification" / manifest["label"] / "verification.json")
        if verification.get("valid") is not True or suite.canonical(verification.get("build")) != suite.canonical(manifest):
            raise ValueError("Prepared bundle lacks matching successful functional verification")
    if suite.canonical(selections["baseline"]) != suite.canonical(selections["candidate"]):
        raise ValueError("Incomparable paired workloads")
    return selections["baseline"]


def render(report):
    lines = ["# Paired desktop callback comparison", "",
             f"Collection valid: {report['valid']}. CPU utilization is metadata; no idle prerequisite.", ""]
    if report["invalidationReasons"]:
        lines.extend(["Collection failed:", *[f"- {reason}" for reason in report["invalidationReasons"]], ""])
    comparison = report["comparison"]
    if comparison:
        lines.extend([f"Decision: **{comparison['decision']}**.", "",
                      "Ratios use process medians and paired circular-block interval envelopes.", "",
                      "| Case | Metric | A median | B median | B/A | 95% envelope |",
                      "| --- | --- | ---: | ---: | ---: | --- |"])
        for effect in comparison["metrics"]:
            ratio, interval = effect["candidateBaselineRatio"], effect["ratio95Envelope"]
            ratio_text = "n/a" if ratio is None else f"{ratio:.6g}"
            interval_text = "diagnostic" if interval is None else f"{interval[0]:.6g}–{interval[1]:.6g}"
            lines.append(f"| {effect['case']} | {effect['metric']} ({effect['unit']}) | {effect['baselineMedian']:.6g} | {effect['candidateMedian']:.6g} | {ratio_text} | {interval_text} |")
        cpu = comparison["wholeProcessCpu"]
        lines.extend(["", f"Whole-process CPU diagnostics: {cpu['completePairs']} complete pairs; {cpu['unavailablePairs']} unavailable.", ""])
        for metric in cpu["metrics"]:
            lines.append(f"- {metric['metric']}: A median {metric['baselineMedian']}, B median {metric['candidateMedian']}; ratio {metric['candidateBaselineRatio']}.")
    lines.extend(["", "Whole-process kernel/user CPU time includes startup, warmups, counting, setup and teardown.",
                  "Raw samples, process summaries, ordering, utilization and memory observations remain in report.json.",
                  "These embedded workloads exclude WebView IPC, Svelte presentation and overall application performance.", ""])
    return "\n".join(lines)


def collect(checkout, baseline, candidate, label, background_label="normal-desktop", cases=(),
            host_factory=None, executor=None):
    suite.validate_label(background_label)
    requested = list(cases) if cases else COPY_CASES
    output = suite.new_directory(suite.artifact_root(checkout), label)
    planned = schedule()
    suite.write_json(output / "schedule.json", {"seed": SEED, "pairs": planned, "requestedCases": requested})
    report = {"schemaVersion": 1, "suite": suite.SUITE, "mode": "paired-measure", "valid": False,
              "label": label, "policy": {"controlProfile": "desktop", "backgroundCondition": background_label,
              "utilizationEnforcement": "record-only", "idleConfirmationRequired": False,
              "pairs": PAIRS, "warmupsPerProcess": 3, "repeatsPerProcess": 15,
              "sampleIntervalSeconds": 1, "processTimeoutSeconds": 600},
              "schedule": planned, "pairs": [], "builds": {}, "machine": None,
              "invalidationReasons": [], "warnings": [], "comparison": None,
              "collector": {"python": sys.version, "files": {path.name: suite.file_hash(path) for path in
                            (Path(__file__), Path(suite.__file__), Path(__file__).with_name("benchmark_host.py"))}},
              "limitations": ["Desktop load is observed, not normalized or filtered by CPU utilization.",
                              "Frozen native v1 authorization token permits measurement; it makes no idle-host assertion here.",
                              "Whole-process CPU diagnostics include warmups, counting, setup and teardown.",
                              "Paired interval envelopes describe the recorded workload and desktop conditions, not all future loads."]}
    host = None
    path = output / "report.json"
    try:
        for side, build_label in (("baseline", baseline), ("candidate", candidate)):
            _, manifest = suite.load_bundle(checkout, build_label)
            suite.canonical(manifest)
            report["builds"][side] = manifest
        descriptors = _admit_builds(checkout, report["builds"], requested)
        report["descriptors"] = descriptors
        report["workloadDigest"] = suite.digest(descriptors)
        if host_factory is None:
            from benchmark_host import WindowsHost
            host_factory = WindowsHost
        host = host_factory()
        report["machine"] = host.identity()
        executor = executor or execute_block
        suite.write_json(path, report)
        for planned_pair in planned:
            pair = {**planned_pair}
            report["pairs"].append(pair)
            for side in pair["order"]:
                directory = output / f"pair-{pair['index']:02d}" / side
                directory.mkdir(parents=True)
                request = suite.driver_request("measure", directory / "raw.json", directory / "work", requested)
                suite.write_json(directory / "request.json", request)
                print(f"Pair {pair['index'] + 1}/{PAIRS}: {side}", flush=True)
                result = executor(report["builds"][side]["drivers"]["native"], request,
                                  directory / "driver.log", host, report["policy"])
                result.update(rawPath=str(directory / "raw.json"), rawSha256=suite.file_hash(directory / "raw.json"))
                component = result.pop("component")
                pair[side] = result
                suite.validate_component(component, "native", "measure", set(requested), 3, 15,
                                         {case["id"]: case for case in descriptors})
                suite.canonical(component)
                result["component"] = component
                suite.write_json(path, report)
        report["comparison"] = compare_pairs(report["pairs"], descriptors)
        report["valid"] = True
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        report["invalidationReasons"].append(f"{type(error).__name__}: {error}")
    finally:
        if host is not None:
            try:
                host.close()
            except OSError as error:
                report["valid"] = False
                report["invalidationReasons"].append(f"Host shutdown failed: {error}")
        suite.write_json(path, report)
        (output / "report.md").write_text(render(report), encoding="utf-8")
    return path, report["valid"]
