"""Compare controlled Revenant benchmark runs using only the Python standard library.

Admission is deliberately strict about workloads and measurement conditions, while
implementation/source identities are retained as evidence and may change. No files
are written by this module. The suite runner owns input/output paths.
"""

import hashlib
import json
import math
import random
import statistics


SUITE = "revenant-optimization"
BOOTSTRAP_SEED = 378
BOOTSTRAP_RESAMPLES = 2000
PRACTICAL_THRESHOLD_PERCENT = 5.0


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _object(value, path):
    _require(isinstance(value, dict), f"{path} must be an object")
    return value


def _text(value, path):
    _require(isinstance(value, str) and bool(value.strip()), f"{path} must be a nonempty string")
    return value


def _integer(value, path, minimum=0):
    _require(type(value) is int and value >= minimum, f"{path} must be an integer >= {minimum}")
    return value


def _canonical(value, path):
    try:
        return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{path} must be finite JSON data") from error


def _list(value, path, nonempty=False):
    _require(isinstance(value, list) and (not nonempty or bool(value)), f"{path} must be {'a nonempty' if nonempty else 'an'} array")
    return value


def _samples(metric, repeats, path):
    samples = _list(metric.get("samples"), f"{path}.samples", nonempty=True)
    _require(len(samples) == repeats, f"{path}.samples has {len(samples)} values; expected {repeats}")
    for number in samples:
        _require(type(number) in (int, float), f"{path}.samples must contain numbers, not booleans")
        try:
            finite = math.isfinite(number)
        except OverflowError:
            finite = False
        _require(finite and number >= 0, f"{path}.samples must be finite and nonnegative")
        if metric["unit"] == "ns":
            _require(number > 0, f"{path}.samples timing must be positive")
    return samples


def _admit(report, position):
    """Return comparison identities and indexed measurements, or explain refusal."""
    report = _object(report, position)
    _require(type(report.get("schemaVersion")) is int and report["schemaVersion"] == 1, f"{position}: unsupported schemaVersion")
    _require(report.get("suite") == SUITE, f"{position}: unsupported suite")
    _require(report.get("mode") == "measure", f"{position}: only measurement reports are comparable")
    _require(report.get("valid") is True, f"{position}: run is not valid")
    reasons = _list(report.get("invalidationReasons"), f"{position}.invalidationReasons")
    _require(not reasons, f"{position}: run has invalidation reasons")
    workload_digest = _text(report.get("workloadDigest"), f"{position}.workloadDigest")
    machine = _object(report.get("machine"), f"{position}.machine")
    fingerprint = _text(machine.get("fingerprint"), f"{position}.machine.fingerprint")
    build = _object(report.get("build"), f"{position}.build")
    compiler = _text(build.get("compiler"), f"{position}.build.compiler")
    cargo = _text(build.get("cargo"), f"{position}.build.cargo")
    profile = _text(build.get("profile"), f"{position}.build.profile")
    features = _object(build.get("features"), f"{position}.build.features")
    for component in ("native", "graphics"):
        selected = _list(features.get(component), f"{position}.build.features.{component}")
        _require(all(isinstance(feature, str) and feature for feature in selected), f"{position}: features must be nonempty strings")
        _require(len(selected) == len(set(selected)), f"{position}: duplicate build features")
    policy = _object(report.get("policy"), f"{position}.policy")
    warmups = _integer(policy.get("warmups"), f"{position}.policy.warmups")
    repeats = _integer(policy.get("repeats"), f"{position}.policy.repeats", minimum=1)
    _require((warmups, repeats) == (3, 15), f"{position}: workload version 1 requires three warmups and fifteen repetitions")
    identity = {
        "workloadDigest": workload_digest,
        "machine.fingerprint": fingerprint,
        "build.compiler": compiler,
        "build.cargo": cargo,
        "build.profile": profile,
        "build.features": _canonical(features, f"{position}.build.features"),
        "build.configuration": _canonical(build.get("configuration", {}), f"{position}.build.configuration"),
        "policy": _canonical(policy, f"{position}.policy"),
    }
    components = {}
    cases = {}
    metrics = {}
    all_case_ids = set()
    for component in _list(report.get("components"), f"{position}.components", nonempty=True):
        component = _object(component, f"{position}.components[]")
        name = _text(component.get("component"), f"{position}.component")
        _require(name in ("native", "graphics"), f"{position}: unsupported component {name}")
        _require(name not in components, f"{position}: duplicate component {name}")
        path = f"{position}.{name}"
        _require(type(component.get("schemaVersion")) is int and component["schemaVersion"] == 1, f"{path}: unsupported schemaVersion")
        _require(component.get("suite") == SUITE, f"{path}: unsupported suite")
        _require(component.get("mode") == "measure", f"{path}: component is not a measurement")
        _require(type(component.get("warmups")) is int and component["warmups"] == warmups, f"{path}: warmups differ from policy")
        _require(type(component.get("repeats")) is int and component["repeats"] == repeats, f"{path}: repeats differ from policy")
        components[name] = _integer(component.get("workloadVersion"), f"{path}.workloadVersion", minimum=1)
        for case in _list(component.get("cases"), f"{path}.cases", nonempty=True):
            case = _object(case, f"{path}.cases[]")
            case_id = _text(case.get("id"), f"{path}.case.id")
            _require(case_id not in all_case_ids, f"{position}: duplicate case id {case_id}")
            all_case_ids.add(case_id)
            case_path = f"{path}.{case_id}"
            parameters = _object(case.get("parameters"), f"{case_path}.parameters")
            required_profile = case.get("requiredProfile", "standard")
            _require(required_profile in ("standard", "gpu"), f"{case_path}: unsupported requiredProfile")
            correctness = _object(case.get("correctness"), f"{case_path}.correctness")
            _require(correctness.get("passed") is True, f"{case_path}: correctness did not pass")
            hardware = case.get("hardwareIdentity")
            if required_profile == "gpu":
                hardware = _object(hardware, f"{case_path}.hardwareIdentity")
                _text(hardware.get("adapter"), f"{case_path}.hardwareIdentity.adapter")
                _text(hardware.get("backend"), f"{case_path}.hardwareIdentity.backend")
            cases[(name, case_id)] = (_canonical(parameters, f"{case_path}.parameters"), required_profile,
                                     _canonical(hardware, f"{case_path}.hardwareIdentity"))
            for metric in _list(case.get("metrics"), f"{case_path}.metrics", nonempty=True):
                metric = _object(metric, f"{case_path}.metrics[]")
                metric_name = _text(metric.get("name"), f"{case_path}.metric.name")
                key = (name, case_id, metric_name)
                _require(key not in metrics, f"{case_path}: duplicate metric {metric_name}")
                unit = metric.get("unit")
                direction = metric.get("better")
                _require(unit in ("ns", "bytes", "count"), f"{case_path}.{metric_name}: unsupported unit")
                _require(direction in ("lower", "higher"), f"{case_path}.{metric_name}: unsupported direction")
                samples = _samples(metric, repeats, f"{case_path}.{metric_name}")
                metrics[key] = {"unit": unit, "better": direction, "samples": samples}
    identity["components.workloadVersion"] = components
    identity["cases"] = cases
    identity["metrics"] = {key: (metric["unit"], metric["better"]) for key, metric in metrics.items()}
    return identity, metrics


def _summary(samples):
    ordered = sorted(samples)
    return {
        "median": statistics.median(ordered),
        "p95": ordered[math.ceil(0.95 * len(ordered)) - 1],
        "sampleCount": len(ordered),
        "minimum": ordered[0],
        "maximum": ordered[-1],
    }


def _percentile(ordered, fraction):
    position = (len(ordered) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def _median_ratio_interval(baseline, candidate, key):
    # Per-metric seeds keep intervals unchanged when unrelated rows are added.
    digest = hashlib.sha256("\0".join(key).encode("utf-8")).digest()
    rng = random.Random(BOOTSTRAP_SEED + int.from_bytes(digest, "big"))
    baseline, candidate = sorted(baseline), sorted(candidate)
    ratios = []
    for _ in range(BOOTSTRAP_RESAMPLES):
        original = statistics.median(rng.choices(baseline, k=len(baseline)))
        changed = statistics.median(rng.choices(candidate, k=len(candidate)))
        ratio = changed / original
        _require(math.isfinite(ratio), f"{key}: median ratio is not finite")
        ratios.append(ratio)
    ratios.sort()
    return [_percentile(ratios, 0.025), _percentile(ratios, 0.975)]


def compare_reports(baseline, candidate):
    """Return all per-metric effects; raise ValueError for inadmissible reports.

    The independent bootstrap estimates uncertainty in the median ratio. It is
    conditional on the recorded samples, not proof of stationary machine state.
    Byte/count metrics remain diagnostic even when their direction is declared.
    """
    baseline_identity, baseline_metrics = _admit(baseline, "baseline")
    candidate_identity, candidate_metrics = _admit(candidate, "candidate")
    for field, value in baseline_identity.items():
        _require(value == candidate_identity[field], f"incomparable {field}")
    effects = []
    for key in sorted(baseline_metrics):
        old_metric = baseline_metrics[key]
        new_metric = candidate_metrics[key]
        old = _summary(old_metric["samples"])
        new = _summary(new_metric["samples"])
        difference = new["median"] - old["median"]
        percent = None if old["median"] == 0 else difference / old["median"] * 100
        _require(percent is None or math.isfinite(percent), f"{key}: percentage change exceeds the finite numeric range")
        factor = 1 if old_metric["better"] == "lower" else -1
        interval = None
        normalized_interval = None
        classification = "diagnostic"
        if old_metric["unit"] == "ns":
            interval = _median_ratio_interval(old_metric["samples"], new_metric["samples"], key)
            normalized_interval = sorted((ratio - 1) * 100 * factor for ratio in interval)
            threshold = PRACTICAL_THRESHOLD_PERCENT / 100
            # Compare ratios directly: subtracting 1 can push an exactly 5%
            # boundary across the threshold through floating-point rounding.
            below = interval[1] < 1 - threshold
            above = interval[0] > 1 + threshold
            if (below and factor == 1) or (above and factor == -1):
                classification = "improvement"
            elif (above and factor == 1) or (below and factor == -1):
                classification = "regression"
            else:
                classification = "inconclusive"
        effects.append({
            "component": key[0],
            "case": key[1],
            "metric": key[2],
            "unit": old_metric["unit"],
            "better": old_metric["better"],
            "baseline": old,
            "candidate": new,
            "absoluteChange": difference,
            "percentChange": percent,
            "normalizedPercentChange": None if percent is None else percent * factor,
            "medianRatio95": interval,
            "normalizedPercent95": normalized_interval,
            "classification": classification,
        })
    return {
        "schemaVersion": 1,
        "suite": SUITE,
        "kind": "comparison",
        "baselineLabel": baseline.get("label", "baseline"),
        "candidateLabel": candidate.get("label", "candidate"),
        "baselineBuild": baseline["build"],
        "candidateBuild": candidate["build"],
        "workloadDigest": baseline["workloadDigest"],
        "machineFingerprint": baseline["machine"]["fingerprint"],
        "policy": baseline["policy"],
        "conditionWarnings": {"baseline": baseline.get("conditionWarnings", []),
                              "candidate": candidate.get("conditionWarnings", [])},
        "bootstrap": {"seed": BOOTSTRAP_SEED, "resamples": BOOTSTRAP_RESAMPLES, "confidence": 0.95, "method": "independent percentile bootstrap of median ratio"},
        "practicalThresholdPercent": PRACTICAL_THRESHOLD_PERCENT,
        "metrics": effects,
        "limitations": [
            "p95 is empirical nearest-rank p95 of the recorded samples; 15 observations cannot establish population p95.",
            "Bootstrap intervals assume independent representative samples; they do not prove machine stability or account for correlated thermal drift.",
            "Each timing is assessed separately at 95% confidence without a multiple-comparison correction; classifications are evidence for investigation, not causal proof.",
            "Bytes and counts are diagnostic: changed workload progress, allocations, or retention is not automatically an improvement.",
            "No aggregate score is calculated; an improvement cannot hide another metric's regression.",
        ],
        "baselineLimitations": baseline.get("limitations", []) + [limitation for component in baseline["components"] for limitation in component.get("limitations", [])],
        "candidateLimitations": candidate.get("limitations", []) + [limitation for component in candidate["components"] for limitation in component.get("limitations", [])],
    }


def _cell(value):
    return str(value).replace("|", "\\|").replace("\n", " ").replace("\r", " ")


def _number(value):
    if value is None:
        return "n/a"
    if type(value) is int:
        return str(value)
    return f"{value:.5g}"


def render_comparison(report):
    """Render every metric, including regressions and diagnostic counters."""
    lines = [
        f"Benchmark comparison: {_cell(report['baselineLabel'])} → {_cell(report['candidateLabel'])}",
        "",
        "| Component / case / metric | Unit | Median before → after | Empirical p95 before → after | Absolute Δ | Median Δ % | Median ratio 95% interval | Effect |",
        "| --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for metric in report["metrics"]:
        interval = metric["medianRatio95"]
        interval_text = "n/a" if interval is None else f"{_number(interval[0])}–{_number(interval[1])}"
        old, new = metric["baseline"], metric["candidate"]
        lines.append("| " + " | ".join([
            _cell(" / ".join([metric["component"], metric["case"], metric["metric"]])),
            _cell(metric["unit"]),
            f"{_number(old['median'])} → {_number(new['median'])}",
            f"{_number(old['p95'])} → {_number(new['p95'])}",
            _number(metric["absoluteChange"]),
            _number(metric["percentChange"]),
            interval_text,
            metric["classification"],
        ]) + " |")
    lines.extend(["", f"Timing classifications require the entire 95% interval beyond a {report['practicalThresholdPercent']:g}% practical threshold. Counters are diagnostic."])
    for position in ("baseline", "candidate"):
        for warning in report.get("conditionWarnings", {}).get(position, []):
            lines.extend(["", f"{position.title()} condition: {_cell(warning)}"])
    for limitation in report["limitations"]:
        lines.extend(["", str(limitation)])
    for label, limitations in (("Baseline", report.get("baselineLimitations", [])), ("Candidate", report.get("candidateLimitations", []))):
        for limitation in limitations:
            lines.extend(["", f"{label}: {limitation}"])
    return "\n".join(lines) + "\n"
