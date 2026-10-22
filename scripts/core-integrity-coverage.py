"""Maintain hash-bound semantic coverage; opening a file never marks it reviewed."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import subprocess
from pathlib import Path


def workflow(path: str) -> str:
    if path.startswith("crates/revenant-graphics") or "native_canvas" in path or "native-canvas" in path:
        return "graphics"
    if path.startswith(("crates/revenant-macros/", "crates/revenant-sdk/src/operations/")):
        return "authoring"
    if path.startswith("crates/revenant-work/") or "/compute" in path:
        return "cpu-memory"
    if path.startswith(("crates/revenant-core/src/tasks/", "crates/revenant-core/src/resources/", "crates/revenant-desktop/src/runtime/")):
        return "execution"
    if path.startswith("packages/client/") or path.startswith(("src/compiled", "templates/")):
        return "authoring"
    return "support"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--record-current", action="store_true", help="bind explicitly authored review records to the current bytes")
    args = parser.parse_args()
    root = args.root.resolve()
    directory = root / "docs/research/core-integrity"
    records_path = directory / "reviews.json"
    reviews = json.loads(records_path.read_text(encoding="utf-8"))
    baseline = json.loads(Path(reviews["baseline"]).read_text(encoding="utf-8"))
    original = {entry["path"]: entry for entry in baseline["files"] if entry["project"] == "framework"}
    reviewed = {entry["path"]: entry for entry in reviews["records"]}
    if len(reviewed) != len(reviews["records"]):
        raise ValueError("duplicate semantic review record")
    for path, entry in reviewed.items():
        if entry["status"] not in ("interpreted", "partial") or not entry["evidence"]:
            raise ValueError(f"invalid review record: {path}")
        if args.record_current:
            entry["reviewed_sha256"] = hashlib.sha256((root / path).read_bytes()).hexdigest()
    if args.record_current:
        records_path.write_text(json.dumps(reviews, indent=2) + "\n", encoding="utf-8")
    paths = set(original) | set(reviewed)
    paths.update(subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard"], cwd=root, text=True).splitlines())
    paths = {path for path in paths if not path.startswith((".beads/", ".tooling/", ".agents/", "docs/research/core-integrity/"))}
    rows = []
    for path in sorted(paths):
        file = root / path
        digest = hashlib.sha256(file.read_bytes()).hexdigest() if file.is_file() else None
        review = reviewed.get(path)
        status = review["status"] if review else "unreviewed"
        if review and digest != review.get("reviewed_sha256"):
            status = "stale-review"
        if path.startswith(("vendor/", "imgs/")) or path in ("Cargo.lock", "LICENSE") or path.endswith("package-lock.json") or path.endswith("/Cargo.lock"):
            status = "dependency-or-asset"
        rows.append({"path": path, "workflow": review.get("workflow", workflow(path)) if review else workflow(path),
                     "component": review["component"] if review else None, "status": status,
                     "baseline_sha256": original.get(path, {}).get("sha256"), "current_sha256": digest,
                     "evidence": review["evidence"] if review else "", "tests": review["tests"] if review else []})
    counts = {status: sum(row["status"] == status for row in rows) for status in sorted({row["status"] for row in rows})}
    output = {"schema": 1, "baseline_inventory_sha256": baseline["source_inventory_sha256"], "counts": counts, "files": rows}
    (directory / "coverage.json").write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    with (directory / "coverage.tsv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.writer(file, delimiter="\t")
        writer.writerow(["path", "workflow", "status", "evidence"])
        writer.writerows([row["path"], row["workflow"], row["status"], row["evidence"]] for row in rows)
    print(json.dumps({"files": len(rows), "counts": counts}))


if __name__ == "__main__":
    main()
