"""Read visibility of native scene windows belonging to the named fixture PID."""
import argparse
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import shutil
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument("pid", type=int)
parser.add_argument("--report-dir", type=Path)
parser.add_argument("--stage")
args = parser.parse_args()
user = ctypes.WinDLL("user32", use_last_error=True)
callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
user.EnumChildWindows.argtypes = [wintypes.HWND, callback_type, wintypes.LPARAM]
user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user.IsWindowVisible.argtypes = [wintypes.HWND]
scenes = []

@callback_type
def child(hwnd, unused):
    pid = wintypes.DWORD()
    user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    if pid.value != args.pid:
        return True
    name = ctypes.create_unicode_buffer(256)
    user.GetClassNameW(hwnd, name, len(name))
    if name.value == "RevenantScene2D":
        rect = wintypes.RECT()
        assert user.GetWindowRect(hwnd, ctypes.byref(rect))
        scenes.append({"hwnd": int(hwnd), "class": name.value,
                       "visible": bool(user.IsWindowVisible(hwnd)),
                       "width": rect.right-rect.left, "height": rect.bottom-rect.top})
    return True

@callback_type
def parent(hwnd, unused):
    pid = wintypes.DWORD()
    user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    if pid.value == args.pid:
        user.EnumChildWindows(hwnd, child, 0)
    return True

assert user.EnumWindows(parent, 0)
evidence = {"appPid": args.pid, "scenes": scenes,
            "visibleScenes": sum(scene["visible"] for scene in scenes)}
if args.report_dir:
    assert args.stage, "a named stage is required for persisted evidence"
    package = json.loads((args.report_dir / "package-acceptance.json").read_text(encoding="utf-8"))
    assert package["appPid"] == args.pid, "the observer must use the owned fixture PID"
    browser = shutil.which("agent-browser.cmd")
    assert browser, "agent-browser is required only for the external observer"
    javascript = ('JSON.stringify({text:document.body.innerText,focus:document.activeElement?.textContent,'
                  'dialogs:document.querySelectorAll("dialog[open], [role=dialog]").length})')
    observed = subprocess.run([browser, "--session", "revenant-native-package", "--cdp",
                               str(package["cdpPort"]), "eval", javascript, "--json"],
                              capture_output=True, text=True, encoding="utf-8", timeout=20, check=True)
    response = json.loads(observed.stdout)
    assert response["success"] and response["data"]["origin"] == "http://tauri.localhost/", response
    evidence.update(stage=args.stage, observedAtUnix=time.time(),
                    document=json.loads(response["data"]["result"]))
    path = args.report_dir / "native-ui-evidence.json"
    report = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {
        "executableSha256": package["executableSha256"], "stages": []}
    assert report["executableSha256"] == package["executableSha256"], "mixed build evidence"
    report["stages"] = [stage for stage in report["stages"] if stage["stage"] != args.stage]
    report["stages"].append(evidence)
    path.write_text(json.dumps(report, indent=2), encoding="utf-8")
print(json.dumps(evidence))
