"""Exercise only the copied fixture, with developer executables absent from its PATH."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import socket
import subprocess
import threading
import time


def mcp_connection(executable, destination, env, kernel, job):
    process = subprocess.Popen([str(executable), "--mcp"], cwd=destination, env=env,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, encoding="utf-8")
    assert kernel.AssignProcessToJobObject(job, wintypes.HANDLE(int(process._handle))), ctypes.get_last_error()
    replies = queue.Queue()
    diagnostics = []
    def read_replies():
        try:
            for line in process.stdout:
                replies.put(json.loads(line))
        finally:
            replies.put(None)
    def read_diagnostics():
        for line in process.stderr:
            diagnostics.append(line.rstrip())
            del diagnostics[:-20]
    threading.Thread(target=read_replies, daemon=True).start()
    stderr_reader = threading.Thread(target=read_diagnostics, daemon=True)
    stderr_reader.start()
    sequence = 0
    def request(method, params):
        nonlocal sequence
        sequence += 1
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": sequence, "method": method, "params": params}) + "\n")
        process.stdin.flush()
        deadline = time.monotonic() + 15
        while True:
            try:
                response = replies.get(timeout=max(0.001, deadline-time.monotonic()))
            except queue.Empty as error:
                raise RuntimeError(f"MCP {method} timeout; exit={process.poll()}; diagnostics={diagnostics}") from error
            if response is None:
                stderr_reader.join(timeout=1)
                raise RuntimeError(f"MCP {method} ended; exit={process.poll()}; diagnostics={diagnostics}")
            if response.get("id") == sequence:
                assert "error" not in response, response
                return response["result"]
    return process, request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("executable", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--hold-seconds", type=int, default=600)
    parser.add_argument("--await-readiness-marker", action="store_true")
    args = parser.parse_args()
    destination = args.destination.resolve()
    checkout = Path(__file__).resolve().parent.parent
    assert not destination.is_relative_to(checkout), "execution must leave the checkout"
    destination.mkdir(parents=True, exist_ok=True)
    executable = destination / "native-workflows.exe"
    shutil.copy2(args.executable, executable)
    env = dict(os.environ)
    system_root = next(value for key, value in env.items() if key.casefold() == "systemroot")
    env["PATH"] = str(Path(system_root) / "System32")
    assert shutil.which("node", path=env["PATH"]) is None
    assert shutil.which("cargo", path=env["PATH"]) is None
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
        f"--remote-debugging-port={port} --remote-debugging-address=127.0.0.1"
    )
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.restype = wintypes.HANDLE
    kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    job = kernel.CreateJobObjectW(None, None)
    assert job, ctypes.get_last_error()
    app = None
    mcp = None
    try:
        app = subprocess.Popen([str(executable)], cwd=destination, env=env)
        assert kernel.AssignProcessToJobObject(job, wintypes.HANDLE(int(app._handle))), ctypes.get_last_error()
        deadline = time.monotonic() + 30
        while True:
            assert app.poll() is None, "packaged app exited during startup"
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                    break
            except OSError:
                assert time.monotonic() < deadline, "owned WebView did not start"
                time.sleep(0.05)
        print(json.dumps({"appPid": app.pid, "cdpPort": port, "outsideCheckout": str(destination),
                          "nodeAvailable": False, "cargoAvailable": False}), flush=True)
        if args.await_readiness_marker:
            deadline = time.monotonic() + 180
            while not (destination / "mcp-ready").exists():
                assert app.poll() is None, "packaged app exited before readiness observation"
                assert time.monotonic() < deadline, "packaged readiness observation expired"
                time.sleep(0.1)
        startup_deadline = time.monotonic() + 60
        while True:
            mcp, request = mcp_connection(executable, destination, env, kernel, job)
            try:
                request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                       "clientInfo": {"name": "fixture-acceptance", "version": "1"}})
                break
            except RuntimeError as error:
                if "instance_missing" not in str(error) or time.monotonic() >= startup_deadline:
                    raise
                # Only discovery/initialization is retried; no operation has run.
                # The debug port can open before the host publishes its bridge.
                mcp.stdin.close()
                mcp.wait(timeout=10)
                assert app.poll() is None, "packaged app exited before bridge readiness"
                time.sleep(0.05)
        def tool(name, arguments):
            response = request("tools/call", {"name": name, "arguments": arguments})
            assert not response.get("isError"), response
            return response["structuredContent"]
        mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        mcp.stdin.flush()
        tools = request("tools/list", {})["tools"]
        exposed = {item["name"] for item in tools}
        assert "fixture.agentSummary" in exposed and "fixture.build" not in exposed
        assert "fixture.patch" not in exposed and "fixture.scene" not in exposed
        receipt = tool("fixture.agentSummary", {})
        deadline = time.monotonic() + 10
        while True:
            snapshot = tool("revenant.tasks.get", {"task": receipt["task"]})
            if snapshot.get("result") is not None:
                break
            assert time.monotonic() < deadline, snapshot
            time.sleep(0.01)
        result = snapshot["result"]
        assert result["checksum"] == "18446744073709551615", result
        assert result["details"]["hostPathsAvailable"] is True, result
        chunks = []
        offset = 0
        while True:
            page = tool("revenant.tasks.result", {"task": receipt["task"], "offset": offset, "limit": 32})
            assert len(page["chunk"].encode()) <= 32
            chunks.append(page["chunk"])
            if page["complete"]:
                break
            assert page["nextOffset"] > offset
            offset = page["nextOffset"]
        assert json.loads("".join(chunks)) == result
        tool("revenant.tasks.dispose", {"task": receipt["task"]})
        mcp.stdin.close()
        assert mcp.wait(timeout=10) == 0
        report = {"executableSha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                  "directory": str(destination), "appPid": app.pid, "cdpPort": port,
                  "developerExecutablesAvailable": {"node": False, "cargo": False},
                  "exposedTools": sorted(exposed), "agentResult": result,
                  "resultPageLimitBytes": 32, "resultPages": len(chunks), "mcpExit": 0}
        (destination / "package-acceptance.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print("Packaged MCP and bounded wide-integer read passed; app retained for UI checks.", flush=True)
        deadline = time.monotonic() + args.hold_seconds
        while time.monotonic() < deadline and not (destination / "stop-acceptance").exists():
            assert app.poll() is None, "packaged app exited during UI checks"
            time.sleep(0.2)
    finally:
        # This job contains only the two fixture processes and their descendants.
        kernel.TerminateJobObject(job, 1)
        if mcp is not None:
            mcp.wait(timeout=10)
        if app is not None:
            app.wait(timeout=10)
        kernel.CloseHandle(job)


if __name__ == "__main__":
    main()
