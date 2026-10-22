"""Synthetic Windows-host observer tests; no performance workloads or devices."""
import ctypes
import threading
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import benchmark_host as host


def row(pid, engine, utilization, physical=0, luid="0x123_0x456"):
    return {"Name": f"pid_{pid}_luid_{luid}_phys_{physical}_eng_{engine}_engtype_3D",
            "UtilizationPercentage": utilization}


class HostTests(unittest.TestCase):
    def test_cached_identity_is_an_independent_deep_copy(self):
        observer = object.__new__(host.WindowsHost)
        observer._identity = {"fingerprint": "stable", "gpuDrivers": [{"Name": "GPU"}]}
        first = observer.identity()
        first["gpuDrivers"][0]["Name"] = "changed"
        self.assertEqual(observer.identity()["gpuDrivers"][0]["Name"], "GPU")
        self.assertEqual(observer.identity()["fingerprint"], "stable")

    def test_identity_refresh_detects_configuration_changes_and_unknown_metadata(self):
        observer = object.__new__(host.WindowsHost)
        observer._identity = None
        observer._gpu_requested = False
        metadata = {"powerPlanGuid": "first-plan", "gpuDrivers": []}
        with patch.object(observer, "_read_identity", return_value=metadata) as read:
            first = observer.identity()
            self.assertEqual(first, observer.identity())
            self.assertEqual(read.call_count, 1)
            metadata["powerPlanGuid"] = "second-plan"
            second = observer.identity(refresh=True)
            self.assertNotEqual(first["fingerprint"], second["fingerprint"])
            self.assertEqual(read.call_count, 2)
            metadata["powerPlanGuid"] = None
            with self.assertRaisesRegex(OSError, "power plan"):
                observer.identity(refresh=True)
            metadata["powerPlanGuid"] = "known-plan"
            metadata["gpuDrivers"] = None
            with self.assertRaisesRegex(OSError, "driver metadata"):
                observer.identity(refresh=True)
            metadata["gpuDrivers"] = []
            observer._gpu_requested = True
            with self.assertRaisesRegex(OSError, "adapter"):
                observer.identity(refresh=True)
            metadata["gpuDrivers"] = [{"Name": "GPU"}]
            with self.assertRaisesRegex(OSError, "active display"):
                observer.identity(refresh=True)
            metadata["activeDisplays"] = [{"widthPixels": 2560, "heightPixels": 1440,
                                           "refreshHz": 144, "dpiX": 96, "dpiY": 96}]
            before_display_change = observer.identity(refresh=True)
            metadata["activeDisplays"][0]["refreshHz"] = 60
            after_display_change = observer.identity(refresh=True)
            self.assertNotEqual(before_display_change["fingerprint"],
                                after_display_change["fingerprint"])

    def test_unsupported_platforms_and_cpu_groups_fail_before_observer_creation(self):
        with patch.object(host.os, "name", "unsupported"):
            with self.assertRaisesRegex(OSError, "Windows"):
                host.WindowsHost()
        for count, groups in [(65, 1), (64, 2), (0, 1)]:
            library = SimpleNamespace(GetActiveProcessorCount=lambda _: count,
                                      GetActiveProcessorGroupCount=lambda: groups)
            with patch.object(host, "_kernel", return_value=library):
                with self.assertRaisesRegex(OSError, "processor group"):
                    host.WindowsHost(gpu=True)

    def test_windows_field_sizes_do_not_depend_on_native_c_long(self):
        self.assertEqual(ctypes.sizeof(host.FILETIME), 8)
        self.assertEqual(ctypes.sizeof(host.MEMORYSTATUSEX), 64)
        self.assertEqual(ctypes.sizeof(host.PROCESS_MEMORY_COUNTERS_EX),
                         80 if ctypes.sizeof(ctypes.c_void_p) == 8 else 44)
        self.assertEqual(ctypes.sizeof(host.JOBOBJECT_EXTENDED_LIMIT_INFORMATION),
                         144 if ctypes.sizeof(ctypes.c_void_p) == 8 else 112)
        self.assertEqual(ctypes.sizeof(host.DEVMODEW), 220)
        self.assertEqual(ctypes.sizeof(host.MONITORINFOEXW), 104)

    def test_filetime_combines_unsigned_low_and_high(self):
        value = host.FILETIME(0xFFFFFFFF, 2)
        self.assertEqual(value.ticks(), (2 << 32) | 0xFFFFFFFF)

    def test_gpu_aggregates_pid_shares_per_adapter_engine_without_summing_engines(self):
        rows = [row(11, 0, 10), row(12, 0, 20), row(11, 1, 25),
                row(12, 0, 15, physical=1), row(12, 0, 16, luid="0x777_0x888")]
        self.assertEqual(host.aggregate_gpu(rows), 30)
        self.assertEqual(host.aggregate_gpu(rows, excluded_pid=12), 25)

    def test_gpu_excludes_only_benchmark_pid_and_caps_engine_sum(self):
        self.assertEqual(host.aggregate_gpu([row(1, 0, 100), row(2, 0, 50)], 1), 50)
        self.assertEqual(host.aggregate_gpu([row(1, 0, 80), row(2, 0, 80)]), 100)
        self.assertEqual(host.aggregate_gpu([row(1, 0, 90)], 1), 0)

    def test_gpu_unknown_and_partial_sensor_failures_are_not_zero(self):
        for rows in [None, [], {}, [row(1, 0, float("nan"))],
                     [row(1, 0, -1)], [row(1, 0, True)], [row(1, 0, "2")],
                     [{"Name": "_Total", "UtilizationPercentage": 0}],
                     [row(1, 0, 10), row(1, 0, 10)],
                     [row(1, 0, 10), {"Name": "unparsable"}]]:
            with self.subTest(rows=rows):
                self.assertIsNone(host.aggregate_gpu(rows))

    def test_gpu_stale_or_terminated_observer_is_unknown(self):
        observer = object.__new__(host._GpuObserver)
        observer._lock = threading.Lock()
        observer._sample = (100, [row(1, 0, 10)])
        observer._process = SimpleNamespace(poll=lambda: None)
        with patch.object(host.time, "monotonic", return_value=101):
            self.assertEqual(observer.utilization(None), 10)
        with patch.object(host.time, "monotonic", return_value=106):
            self.assertIsNone(observer.utilization(None))
        observer._process = SimpleNamespace(poll=lambda: 0)
        with patch.object(host.time, "monotonic", return_value=101):
            self.assertIsNone(observer.utilization(None))

    def test_exit_snapshot_keeps_process_cpu_when_process_memory_is_unavailable(self):
        def system_times(idle, kernel, user):
            idle._obj.low, kernel._obj.low, user._obj.low = 100, 200, 300
            return 1

        def process_times(_handle, _created, _exited, kernel, user):
            kernel._obj.low, user._obj.low = 17, 23
            return 1

        def memory_status(memory):
            memory._obj.availablePhysical = 1234
            return 1

        observer = object.__new__(host.WindowsHost)
        observer._kernel = SimpleNamespace(GetSystemTimes=system_times,
                                          GetProcessTimes=process_times,
                                          GlobalMemoryStatusEx=memory_status)
        observer._psapi = SimpleNamespace(GetProcessMemoryInfo=lambda *_: 0)
        observer._gpu = None
        observed = observer.snapshot(SimpleNamespace(_handle=123, pid=55))
        self.assertEqual(observed["processTicks"], 40)
        self.assertEqual(observed["availableMemoryBytes"], 1234)
        self.assertIsNone(observed["workingSetBytes"])
        self.assertIsNone(observed["privateBytes"])

    def test_owned_job_assigns_only_explicit_child_and_closes_only_its_handle(self):
        assigned, closed = [], []

        def limits(job, kind, pointer, size):
            self.assertEqual(job, 99)
            self.assertEqual(kind, 9)
            self.assertEqual(pointer._obj.basic.flags, 0x2000)
            self.assertEqual(size, ctypes.sizeof(host.JOBOBJECT_EXTENDED_LIMIT_INFORMATION))
            return 1

        library = SimpleNamespace(CreateJobObjectW=lambda *_: 99,
                                  SetInformationJobObject=limits,
                                  AssignProcessToJobObject=lambda job, process: assigned.append((job, process.value)) or 1,
                                  CloseHandle=lambda handle: closed.append(handle) or 1)
        with patch.object(host, "_kernel", return_value=library):
            with host.OwnedJob(SimpleNamespace(_handle=42)) as job:
                self.assertEqual(assigned, [(99, 42)])
            job.close()
        self.assertEqual(closed, [99])


if __name__ == "__main__":
    unittest.main()
