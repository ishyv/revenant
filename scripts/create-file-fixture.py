"""Create a real, reusable 500,000-file acceptance fixture in an ignored directory.

Each file is created exclusively. An interrupted run resumes without truncating
existing files; no deletion or filesystem traversal is performed by this helper.
"""
import argparse
import json
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("directory", type=Path)
parser.add_argument("--count", type=int, default=500_000)
args = parser.parse_args()
args.directory.mkdir(parents=True, exist_ok=True)
start = time.perf_counter()
created = 0
for ordinal in range(args.count):
    path = args.directory / f"entry{ordinal:06d}.txt"
    try:
        with path.open("xb") as file:
            if ordinal == 0:
                file.write(b"Hello, Revenant. Native files stay native.\n")
        created += 1
    except FileExistsError:
        pass
    if (ordinal + 1) % 50_000 == 0:
        print(json.dumps({"visited": ordinal + 1, "created": created,
                          "elapsedSeconds": round(time.perf_counter() - start, 3)}), flush=True)
print(json.dumps({"fixture": str(args.directory.resolve()), "files": args.count,
                  "created": created, "elapsedSeconds": round(time.perf_counter() - start, 3)}))
