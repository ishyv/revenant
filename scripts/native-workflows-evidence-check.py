"""Check collected package/UI observations against the frozen fixture invariants."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("directory", type=Path)
args = parser.parse_args()
package = json.loads((args.directory / "package-acceptance.json").read_text(encoding="utf-8"))
ui = json.loads((args.directory / "native-ui-evidence.json").read_text(encoding="utf-8"))
assert package["executableSha256"] == ui["executableSha256"]
assert package["developerExecutablesAvailable"] == {"node": False, "cargo": False}
assert package["agentResult"]["checksum"] == str(2**64-1)
assert package["mcpExit"] == 0 and package["resultPageLimitBytes"] == 32
stages = {stage["stage"]: stage for stage in ui["stages"]}
expected = {"ready-500k": (1, 0), "ready-1m-optional-failure": (1, 0),
            "first-cover-open": (0, 1), "overlapping-covers-open": (0, 2),
            "one-cover-remains": (0, 1), "covers-closed-focus-restored": (1, 0)}
handles = set()
for name, (visible, dialogs) in expected.items():
    observed = stages[name]
    assert observed["appPid"] == package["appPid"]
    assert observed["visibleScenes"] == visible, (name, observed)
    assert observed["document"]["dialogs"] == dialogs, (name, observed)
    assert len(observed["scenes"]) == 1
    scene = observed["scenes"][0]
    assert scene["width"] > 1 and scene["height"] > 1
    handles.add(scene["hwnd"])
assert len(handles) == 1, "coverage must retain the same independent native window"

# Compose the thirty-two LCG steps independently, then sum exact native u32 values.
mask = 2**32-1
multiplier, increment = 1, 0
for _ in range(32):
    multiplier = multiplier * 1_664_525 & mask
    increment = (increment * 1_664_525 + 1_013_904_223) & mask
for count, name in [(500_000, "ready-500k"), (1_000_000, "ready-1m-optional-failure")]:
    checksum = sum(((index+378)*multiplier+increment) & mask for index in range(count))
    text = stages[name]["document"]["text"]
    assert f"Request entries-{count} · data entries-{count} · ready" in text
    assert f"{count} entries · checksum {checksum}" in text
assert "Optional read: Requested optional-slot failure" in stages["ready-1m-optional-failure"]["document"]["text"]
assert stages["covers-closed-focus-restored"]["document"]["focus"] == "Open dialog"
print("Package identity, exact checksums, keyed reads, native overlapping covers and final focus passed.")
