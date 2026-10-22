"""Read-only Windows counters and an explicitly owned persistent GPU observer.

GPU sampling queries CIM once per second in one PowerShell process. Its overhead
is included in external CPU activity. These are utilization estimates, not GPU
timestamps, compositor evidence, thermal control, or complete process memory.
"""
from __future__ import annotations

import ctypes
import copy
import hashlib
import json
import math
import os
import platform
import re
import subprocess
import threading
import time

DWORD = ctypes.c_uint32
SIZE_T = ctypes.c_size_t
HANDLE = ctypes.c_void_p


class FILETIME(ctypes.Structure):
    _fields_ = [("low", DWORD), ("high", DWORD)]

    def ticks(self):
        return self.low | (self.high << 32)


class MEMORYSTATUSEX(ctypes.Structure):
    _fields_ = [("length", DWORD), ("memoryLoad", DWORD)] + [
        (name, ctypes.c_uint64) for name in (
            "totalPhysical", "availablePhysical", "totalPageFile",
            "availablePageFile", "totalVirtual", "availableVirtual",
            "availableExtendedVirtual")]


class PROCESS_MEMORY_COUNTERS_EX(ctypes.Structure):
    _fields_ = [("cb", DWORD), ("pageFaultCount", DWORD)] + [
        (name, SIZE_T) for name in (
            "peakWorkingSet", "workingSet", "quotaPeakPagedPool",
            "quotaPagedPool", "quotaPeakNonPagedPool", "quotaNonPagedPool",
            "pageFile", "peakPageFile", "privateBytes")]


class RECT(ctypes.Structure):
    _fields_ = [(name, ctypes.c_int32) for name in ("left", "top", "right", "bottom")]


class MONITORINFOEXW(ctypes.Structure):
    _fields_ = [("cbSize", DWORD), ("monitor", RECT), ("work", RECT),
                ("flags", DWORD), ("device", ctypes.c_uint16 * 32)]


class DISPLAY_UNION(ctypes.Union):
    _fields_ = [("raw", ctypes.c_uint8 * 16)]


class DEVMODEW(ctypes.Structure):
    _fields_ = [("device", ctypes.c_uint16 * 32), ("specVersion", ctypes.c_uint16),
                ("driverVersion", ctypes.c_uint16), ("size", ctypes.c_uint16),
                ("driverExtra", ctypes.c_uint16), ("fields", DWORD),
                ("display", DISPLAY_UNION)] + [
        (name, ctypes.c_uint16) for name in
        ("color", "duplex", "yResolution", "ttOption", "collate")] + [
        ("formName", ctypes.c_uint16 * 32), ("logPixels", ctypes.c_uint16)] + [
        (name, DWORD) for name in (
            "bitsPerPel", "width", "height", "displayFlags", "frequency",
            "icmMethod", "icmIntent", "mediaType", "ditherType", "reserved1",
            "reserved2", "panningWidth", "panningHeight")]


def active_displays():
    """Active physical-pixel monitor layout, mode frequency and effective DPI.

    Awareness is changed only on this observing thread and restored immediately;
    no display, power, scaling or application setting is changed.
    """
    if os.name != "nt":
        raise OSError("Active display observations require Windows")
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    shcore = ctypes.WinDLL("shcore", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(ctypes.c_int32, HANDLE, HANDLE,
                                      ctypes.POINTER(RECT), ctypes.c_ssize_t)
    user32.EnumDisplayMonitors.argtypes = [HANDLE, ctypes.c_void_p, callback_type, ctypes.c_ssize_t]
    user32.EnumDisplayMonitors.restype = ctypes.c_int32
    user32.GetMonitorInfoW.argtypes = [HANDLE, ctypes.POINTER(MONITORINFOEXW)]
    user32.GetMonitorInfoW.restype = ctypes.c_int32
    user32.EnumDisplaySettingsExW.argtypes = [ctypes.c_wchar_p, DWORD, ctypes.POINTER(DEVMODEW), DWORD]
    user32.EnumDisplaySettingsExW.restype = ctypes.c_int32
    user32.SetThreadDpiAwarenessContext.argtypes = [HANDLE]
    user32.SetThreadDpiAwarenessContext.restype = HANDLE
    shcore.GetDpiForMonitor.argtypes = [HANDLE, ctypes.c_int32, ctypes.POINTER(DWORD), ctypes.POINTER(DWORD)]
    shcore.GetDpiForMonitor.restype = ctypes.c_int32
    shcore.GetScaleFactorForMonitor.argtypes = [HANDLE, ctypes.POINTER(ctypes.c_int32)]
    shcore.GetScaleFactorForMonitor.restype = ctypes.c_int32
    displays, errors = [], []

    def inspect(monitor, _dc, _rect, _data):
        try:
            information = MONITORINFOEXW()
            information.cbSize = ctypes.sizeof(information)
            _checked(user32.GetMonitorInfoW(monitor, ctypes.byref(information)), "GetMonitorInfoW")
            device = bytes(information.device).decode("utf-16-le").rstrip("\x00")
            mode = DEVMODEW()
            mode.size = ctypes.sizeof(mode)
            _checked(user32.EnumDisplaySettingsExW(device, 0xFFFFFFFF, ctypes.byref(mode), 0),
                     "EnumDisplaySettingsExW")
            dpi_x, dpi_y = DWORD(), DWORD()
            if shcore.GetDpiForMonitor(monitor, 0, ctypes.byref(dpi_x), ctypes.byref(dpi_y)) < 0:
                raise OSError("Effective monitor DPI is unavailable")
            scale = ctypes.c_int32()
            if shcore.GetScaleFactorForMonitor(monitor, ctypes.byref(scale)) < 0:
                raise OSError("Active monitor scale is unavailable")
            if not device or not mode.width or not mode.height or mode.frequency <= 1 or not dpi_x.value or not dpi_y.value or scale.value <= 0:
                raise OSError("Active display geometry, refresh frequency or DPI is unknown")
            displays.append({
                "device": device, "primary": bool(information.flags & 1),
                "position": [information.monitor.left, information.monitor.top],
                "widthPixels": mode.width, "heightPixels": mode.height,
                "refreshHz": mode.frequency, "bitsPerPixel": mode.bitsPerPel,
                "dpiX": dpi_x.value, "dpiY": dpi_y.value,
                "scalePercent": scale.value,
            })
            return 1
        except Exception as error:
            errors.append(error)
            return 0

    callback = callback_type(inspect)
    previous = user32.SetThreadDpiAwarenessContext(HANDLE(-4))
    _checked(previous, "SetThreadDpiAwarenessContext(observer)")
    try:
        success = user32.EnumDisplayMonitors(None, None, callback, 0)
        if errors:
            raise OSError("Active GPU display metadata is unavailable") from errors[0]
        _checked(success, "EnumDisplayMonitors")
        if not displays:
            raise OSError("GPU timing requires a known active display")
        return sorted(displays, key=lambda display: display["device"])
    finally:
        _checked(user32.SetThreadDpiAwarenessContext(previous),
                 "Restore observing thread DPI awareness")


class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("processUserLimit", ctypes.c_int64),
                ("jobUserLimit", ctypes.c_int64), ("flags", DWORD),
                ("minimumWorkingSet", SIZE_T), ("maximumWorkingSet", SIZE_T),
                ("activeProcessLimit", DWORD), ("affinity", SIZE_T),
                ("priorityClass", DWORD), ("schedulingClass", DWORD)]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint64) for name in (
        "readOperations", "writeOperations", "otherOperations", "readBytes",
        "writeBytes", "otherBytes")]


class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("basic", JOBOBJECT_BASIC_LIMIT_INFORMATION),
                ("io", IO_COUNTERS), ("processMemoryLimit", SIZE_T),
                ("jobMemoryLimit", SIZE_T), ("peakProcessMemoryUsed", SIZE_T),
                ("peakJobMemoryUsed", SIZE_T)]


class THREADENTRY32(ctypes.Structure):
    _fields_ = [("size", DWORD), ("usage", DWORD), ("threadId", DWORD),
                ("ownerProcessId", DWORD), ("basePriority", ctypes.c_int32),
                ("deltaPriority", ctypes.c_int32), ("flags", DWORD)]


def _kernel():
    if os.name != "nt":
        raise OSError("Benchmark host observations require Windows")
    library = ctypes.WinDLL("kernel32", use_last_error=True)
    library.GetSystemTimes.argtypes = [ctypes.POINTER(FILETIME)] * 3
    library.GetSystemTimes.restype = ctypes.c_int32
    library.GetProcessTimes.argtypes = [HANDLE] + [ctypes.POINTER(FILETIME)] * 4
    library.GetProcessTimes.restype = ctypes.c_int32
    library.GlobalMemoryStatusEx.argtypes = [ctypes.POINTER(MEMORYSTATUSEX)]
    library.GlobalMemoryStatusEx.restype = ctypes.c_int32
    library.GetActiveProcessorCount.argtypes = [ctypes.c_uint16]
    library.GetActiveProcessorCount.restype = DWORD
    library.GetActiveProcessorGroupCount.argtypes = []
    library.GetActiveProcessorGroupCount.restype = ctypes.c_uint16
    library.CreateJobObjectW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p]
    library.CreateJobObjectW.restype = HANDLE
    library.SetInformationJobObject.argtypes = [HANDLE, ctypes.c_int32, ctypes.c_void_p, DWORD]
    library.SetInformationJobObject.restype = ctypes.c_int32
    library.AssignProcessToJobObject.argtypes = [HANDLE, HANDLE]
    library.AssignProcessToJobObject.restype = ctypes.c_int32
    library.CreateToolhelp32Snapshot.argtypes = [DWORD, DWORD]
    library.CreateToolhelp32Snapshot.restype = HANDLE
    library.Thread32First.argtypes = [HANDLE, ctypes.POINTER(THREADENTRY32)]
    library.Thread32First.restype = ctypes.c_int32
    library.Thread32Next.argtypes = [HANDLE, ctypes.POINTER(THREADENTRY32)]
    library.Thread32Next.restype = ctypes.c_int32
    library.OpenThread.argtypes = [DWORD, ctypes.c_int32, DWORD]
    library.OpenThread.restype = HANDLE
    library.GetProcessIdOfThread.argtypes = [HANDLE]
    library.GetProcessIdOfThread.restype = DWORD
    library.ResumeThread.argtypes = [HANDLE]
    library.ResumeThread.restype = DWORD
    library.CloseHandle.argtypes = [HANDLE]
    library.CloseHandle.restype = ctypes.c_int32
    return library


def _checked(success, operation):
    if not success:
        raise ctypes.WinError(ctypes.get_last_error(), operation)


def _resume_primary_thread(process, library):
    """Resume only the sole thread of our CREATE_SUSPENDED child.

    Popen closes CreateProcess's primary thread handle. Toolhelp recovers its ID
    before any child code runs; unexpected thread/suspend state fails closed.
    """
    snapshot = library.CreateToolhelp32Snapshot(0x00000004, 0)  # TH32CS_SNAPTHREAD
    _checked(snapshot and snapshot != HANDLE(-1).value, "CreateToolhelp32Snapshot")
    threads = []
    try:
        entry = THREADENTRY32()
        entry.size = ctypes.sizeof(entry)
        _checked(library.Thread32First(snapshot, ctypes.byref(entry)), "Thread32First")
        while True:
            if entry.size < THREADENTRY32.ownerProcessId.offset + ctypes.sizeof(DWORD):
                raise OSError("Thread snapshot omitted process ownership")
            if entry.ownerProcessId == process.pid:
                threads.append(entry.threadId)
            entry.size = ctypes.sizeof(entry)
            if not library.Thread32Next(snapshot, ctypes.byref(entry)):
                if ctypes.get_last_error() != 18:  # ERROR_NO_MORE_FILES
                    _checked(False, "Thread32Next")
                break
    finally:
        _checked(library.CloseHandle(snapshot), "CloseHandle(thread snapshot)")
    if len(threads) != 1:
        raise OSError(f"Suspended owned child requires one primary thread; found {len(threads)}")
    # The snapshot's numeric thread ID can go stale before OpenThread.
    thread = library.OpenThread(0x0002 | 0x0800, False, threads[0])
    # THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION
    _checked(thread, "OpenThread(owned primary thread)")
    try:
        owner = library.GetProcessIdOfThread(thread)
        _checked(owner, "GetProcessIdOfThread(owned primary thread)")
        if owner != process.pid or process.poll() is not None:
            raise OSError("Opened primary thread no longer belongs to the live owned child")
        previous = library.ResumeThread(thread)
        if previous == 0xFFFFFFFF:
            _checked(False, "ResumeThread")
        if previous != 1:
            raise OSError(f"Owned primary thread had unexpected suspend count {previous}")
    finally:
        _checked(library.CloseHandle(thread), "CloseHandle(primary thread)")


class OwnedJob:
    """Kill-on-close job containing only explicitly assigned owned Popen children.

    Benchmark launchers create children suspended and request resume=True so job
    assignment precedes execution and descendant creation. Explicit assignment of
    an already-running child remains possible, without that startup guarantee.
    """
    def __init__(self, process=None, *, resume=False):
        if resume and process is None:
            raise ValueError("Resuming an owned child requires a process")
        self._kernel = _kernel()
        self._handle = self._kernel.CreateJobObjectW(None, None)
        _checked(self._handle, "CreateJobObjectW")
        limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        try:
            _checked(self._kernel.SetInformationJobObject(
                self._handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)),
                "SetInformationJobObject")
            if process is not None:
                self.assign(process)
                if resume:
                    _resume_primary_thread(process, self._kernel)
        except BaseException:
            self.close()
            raise

    def assign(self, process):
        if not self._handle:
            raise RuntimeError("Owned job is closed")
        _checked(self._kernel.AssignProcessToJobObject(
            self._handle, HANDLE(int(process._handle))), "AssignProcessToJobObject")

    def close(self):
        if self._handle:
            _checked(self._kernel.CloseHandle(self._handle), "CloseHandle(job)")
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


ENGINE = re.compile(
    r"^pid_(\d+)_luid_([^_]+)_([^_]+)_phys_(\d+)_eng_(\d+)_engtype_(.+)$")


def aggregate_gpu(rows, excluded_pid=None):
    """Maximum external engine load, summing PID shares per adapter and engine.

    Multiple engines operate concurrently; adding independent engines would
    invent utilization above 100%. Missing or malformed sensors remain unknown.
    """
    if not isinstance(rows, list) or not rows:
        return None
    engines = {}
    seen = set()
    for row in rows:
        if not isinstance(row, dict):
            return None
        match = ENGINE.fullmatch(str(row.get("Name", "")))
        if not match:
            return None
        value = row.get("UtilizationPercentage")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return None
        value = float(value)
        if not math.isfinite(value) or not 0 <= value <= 100:
            return None
        pid, luid_low, luid_high, physical, engine, _kind = match.groups()
        identity = (int(pid), luid_low, luid_high, physical, engine)
        if identity in seen:
            return None
        seen.add(identity)
        if int(pid) == excluded_pid:
            continue
        key = (luid_low, luid_high, physical, engine)
        engines[key] = min(100., engines.get(key, 0.) + value)
    return max(engines.values(), default=0.)


def _powershell_command(script):
    system_root = os.environ.get("SystemRoot", r"C:\Windows")
    executable = os.path.join(system_root, "System32", "WindowsPowerShell", "v1.0", "powershell.exe")
    return [executable, "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script]


GPU_OBSERVER = r"""
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
while ($true) {
  try {
    $rows = @(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine |
      Select-Object Name, UtilizationPercentage)
    @{ ok=$true; rows=$rows } | ConvertTo-Json -Compress -Depth 4
  } catch {
    @{ ok=$false; rows=@() } | ConvertTo-Json -Compress
  }
  Start-Sleep -Milliseconds 1000
}
"""


class _GpuObserver:
    def __init__(self):
        self._lock = threading.Lock()
        self._sample = None
        self._job = None
        self._thread = None
        self._process = subprocess.Popen(
            _powershell_command(GPU_OBSERVER), stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, encoding="utf-8", errors="replace",
            creationflags=subprocess.CREATE_NO_WINDOW | 0x00000004)  # CREATE_SUSPENDED
        try:
            self._job = OwnedJob(self._process, resume=True)
            self._thread = threading.Thread(target=self._read, daemon=True,
                                            name="benchmark-gpu-observer")
            self._thread.start()
        except BaseException:
            self.close()
            raise

    def _read(self):
        try:
            for line in self._process.stdout:
                try:
                    parsed = json.loads(line)
                    rows = parsed.get("rows") if parsed.get("ok") is True else None
                except (ValueError, AttributeError):
                    rows = None
                with self._lock:
                    self._sample = (time.monotonic(), rows)
        finally:
            with self._lock:
                self._sample = None

    def utilization(self, excluded_pid):
        with self._lock:
            sample = self._sample
        if sample is None or time.monotonic() - sample[0] > 5 or self._process.poll() is not None:
            return None
        return aggregate_gpu(sample[1], excluded_pid)

    def close(self):
        try:
            if self._job is not None:
                self._job.close()
        finally:
            try:
                if self._process.poll() is None:
                    self._process.kill()
                self._process.wait(timeout=10)
            finally:
                try:
                    # Construction may fail before the reader thread is started.
                    if self._thread is not None and self._thread.ident is not None:
                        self._thread.join(timeout=10)
                finally:
                    self._process.stdout.close()


class WindowsHost:
    """Read-only host identity and cumulative system/process counters."""
    def __init__(self, gpu=False):
        self._kernel = _kernel()
        self.logical_cpus = int(self._kernel.GetActiveProcessorCount(0xFFFF))
        groups = int(self._kernel.GetActiveProcessorGroupCount())
        if not 1 <= self.logical_cpus <= 64 or groups != 1:
            raise OSError("Benchmark CPU counters support one processor group and at most 64 logical CPUs")
        self._psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self._psapi.GetProcessMemoryInfo.argtypes = [
            HANDLE, ctypes.POINTER(PROCESS_MEMORY_COUNTERS_EX), DWORD]
        self._psapi.GetProcessMemoryInfo.restype = ctypes.c_int32
        self._gpu = _GpuObserver() if gpu else None
        self._gpu_requested = bool(gpu)
        self._identity = None

    def _memory(self):
        memory = MEMORYSTATUSEX()
        memory.length = ctypes.sizeof(memory)
        _checked(self._kernel.GlobalMemoryStatusEx(ctypes.byref(memory)), "GlobalMemoryStatusEx")
        return memory

    def _read_identity(self):
        import winreg
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as key:
            cpu_model = winreg.QueryValueEx(key, "ProcessorNameString")[0].strip()
        command = _powershell_command(
            "$ErrorActionPreference='Stop'; "
            "[Console]::OutputEncoding=New-Object System.Text.UTF8Encoding($false); "
            "$controllers = @(Get-CimInstance Win32_VideoController | "
            "Select-Object Name,DriverVersion,PNPDeviceID); "
            "ConvertTo-Json -Compress -InputObject $controllers")
        try:
            output = subprocess.run(command, check=True, capture_output=True,
                                    text=True, encoding="utf-8", errors="replace",
                                    timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
            controllers = json.loads(output.stdout)
            if not isinstance(controllers, list):
                controllers = None
            elif not all(isinstance(row, dict) and all(
                    isinstance(row.get(field), str) and row[field].strip()
                    for field in ("Name", "DriverVersion", "PNPDeviceID"))
                    for row in controllers):
                controllers = None
            else:
                controllers.sort(key=lambda row: row["PNPDeviceID"])
        except (subprocess.SubprocessError, ValueError):
            controllers = None
        try:
            powercfg = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "powercfg.exe")
            output = subprocess.run([powercfg, "/getactivescheme"], check=True,
                                    capture_output=True, timeout=10,
                                    creationflags=subprocess.CREATE_NO_WINDOW)
            match = re.search(rb"[0-9a-fA-F]{8}-(?:[0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}", output.stdout)
            power_plan = match.group().decode("ascii").lower() if match else None
        except subprocess.SubprocessError:
            power_plan = None
        return {
            "platform": "Windows", "osVersion": platform.version(),
            "architecture": platform.machine(), "cpuLogicalCount": self.logical_cpus,
            "cpuModel": cpu_model, "physicalMemoryBytes": self._memory().totalPhysical,
            "powerPlanGuid": power_plan, "gpuDrivers": controllers,
            "activeDisplays": active_displays() if self._gpu_requested else None,
        }

    def identity(self, refresh=False):
        """Refreshable identity; missing comparison-critical metadata fails closed."""
        if refresh:
            self._identity = None
        if self._identity is None:
            metadata = self._read_identity()
            if not metadata.get("powerPlanGuid"):
                raise OSError("Active Windows power plan is unavailable; host identity cannot be compared")
            if metadata.get("gpuDrivers") is None:
                raise OSError("GPU driver metadata is unavailable; host identity cannot be compared")
            if self._gpu_requested and not metadata["gpuDrivers"]:
                raise OSError("GPU profile requires a known graphics adapter")
            if self._gpu_requested and not metadata.get("activeDisplays"):
                raise OSError("GPU profile requires known active display geometry, refresh and DPI")
            fingerprint = hashlib.sha256(json.dumps(
                metadata, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()
            self._identity = {**metadata, "fingerprint": fingerprint}
        return copy.deepcopy(self._identity)

    def own(self, process):
        """Assign and resume this caller-owned CREATE_SUSPENDED benchmark child."""
        return OwnedJob(process, resume=True)

    def process_times(self, process):
        """Final owned-process CPU durations, including all its threads and setup."""
        created, exited, kernel, user = FILETIME(), FILETIME(), FILETIME(), FILETIME()
        _checked(self._kernel.GetProcessTimes(
            HANDLE(int(process._handle)), ctypes.byref(created), ctypes.byref(exited),
            ctypes.byref(kernel), ctypes.byref(user)), "GetProcessTimes")
        kernel_ns, user_ns = kernel.ticks() * 100, user.ticks() * 100
        return {"kernelNs": kernel_ns, "userNs": user_ns, "totalNs": kernel_ns + user_ns}

    def snapshot(self, process=None):
        idle, kernel, user = FILETIME(), FILETIME(), FILETIME()
        _checked(self._kernel.GetSystemTimes(
            ctypes.byref(idle), ctypes.byref(kernel), ctypes.byref(user)), "GetSystemTimes")
        process_ticks, working_set, private_bytes = 0, None, None
        if process is not None:
            created, exited, process_kernel, process_user = FILETIME(), FILETIME(), FILETIME(), FILETIME()
            handle = HANDLE(int(process._handle))
            _checked(self._kernel.GetProcessTimes(
                handle, ctypes.byref(created), ctypes.byref(exited),
                ctypes.byref(process_kernel), ctypes.byref(process_user)), "GetProcessTimes")
            process_ticks = process_kernel.ticks() + process_user.ticks()
            counters = PROCESS_MEMORY_COUNTERS_EX()
            counters.cb = ctypes.sizeof(counters)
            if self._psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                working_set, private_bytes = counters.workingSet, counters.privateBytes
        return {
            "idleTicks": idle.ticks(), "kernelTicks": kernel.ticks(), "userTicks": user.ticks(),
            "processTicks": process_ticks, "availableMemoryBytes": self._memory().availablePhysical,
            "workingSetBytes": working_set, "privateBytes": private_bytes,
            "externalGpuPercent": self._gpu.utilization(process.pid if process else None) if self._gpu else None,
        }

    def close(self):
        if self._gpu:
            gpu, self._gpu = self._gpu, None
            gpu.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
