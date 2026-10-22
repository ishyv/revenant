"""Real Windows child-lifetime regressions; no timed benchmark workloads."""
import ctypes
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import benchmark_host as host
import benchmark_suite as suite


@unittest.skipUnless(os.name == "nt", "Windows job ownership")
class OwnershipTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        self.processes, self.jobs, self.logs = [], [], []
        self.entry = {"path": sys.executable, "sha256": suite.file_hash(Path(sys.executable))}
        self.request = suite.driver_request(
            "describe", self.directory / "report.json", self.directory / "work", [])

    def tearDown(self):
        try:
            for job in self.jobs:
                job.close()
            for process in self.processes:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=10)
            for log in self.logs:
                log.close()
        finally:
            self.temporary.cleanup()

    def launch(self, code):
        log = (self.directory / f"child-{len(self.logs)}.log").open("wb")
        self.logs.append(log)
        with patch.object(suite, "TEST_ARGS", ["-c", code]):
            process = suite.launch_driver(self.entry, self.request, log)
        self.processes.append(process)
        return process

    def own(self, process):
        job = suite.own_process(process)
        self.jobs.append(job)
        return job

    @staticmethod
    def report_code(extra=""):
        return (
            "import json,os,pathlib; "
            "request=json.loads(os.environ['REVENANT_BENCHMARK_REQUEST']); "
            + extra
            + "pathlib.Path(request['output']).write_text(json.dumps({'finished':True}))"
        )

    def test_child_cannot_publish_or_exit_before_job_assignment(self):
        process = self.launch(self.report_code())
        with self.assertRaises(subprocess.TimeoutExpired):
            process.wait(timeout=0.3)
        self.assertFalse(Path(self.request["output"]).exists())
        self.own(process)
        self.assertEqual(process.wait(timeout=10), 0)
        self.assertEqual(suite.read_json(Path(self.request["output"])), {"finished": True})

    def test_delayed_ownership_preserves_an_immediate_exit_report(self):
        process = self.launch(self.report_code())
        try:
            process.wait(timeout=0.3)
        except subprocess.TimeoutExpired:
            pass
        self.own(process)
        self.assertEqual(process.wait(timeout=10), 0)
        self.assertEqual(suite.read_json(Path(self.request["output"])), {"finished": True})

    def test_job_closure_terminates_a_slow_child(self):
        process = self.launch("import time; time.sleep(60)")
        job = self.own(process)
        self.assertIsNone(process.poll())
        job.close()
        self.assertIsNotNone(process.wait(timeout=10))

    def test_job_closure_terminates_a_descendant_after_root_exit(self):
        pid_path = self.directory / "descendant.pid"
        process = self.launch(
            "import pathlib,subprocess,sys; "
            "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); "
            f"pathlib.Path({str(pid_path)!r}).write_text(str(child.pid))"
        )
        job = self.own(process)
        self.assertEqual(process.wait(timeout=10), 0)
        library = ctypes.WinDLL("kernel32", use_last_error=True)
        library.OpenProcess.argtypes = [host.DWORD, ctypes.c_int32, host.DWORD]
        library.OpenProcess.restype = host.HANDLE
        library.WaitForSingleObject.argtypes = [host.HANDLE, host.DWORD]
        library.WaitForSingleObject.restype = host.DWORD
        library.TerminateProcess.argtypes = [host.HANDLE, ctypes.c_uint32]
        library.TerminateProcess.restype = ctypes.c_int32
        library.CloseHandle.argtypes = [host.HANDLE]
        library.CloseHandle.restype = ctypes.c_int32
        handle = library.OpenProcess(0x100000 | 0x0001, False, int(pid_path.read_text()))
        self.assertTrue(handle, "Owned descendant must still be running before job closure")
        try:
            self.assertEqual(library.WaitForSingleObject(handle, 0), 0x102)
            job.close()
            self.assertEqual(library.WaitForSingleObject(handle, 10000), 0)
        finally:
            # A failed containment assertion must not leave the synthetic sleeper behind.
            library.TerminateProcess(handle, 1)
            library.WaitForSingleObject(handle, 10000)
            library.CloseHandle(handle)

    def check_startup_failure(self, target, error):
        original_launch = suite.launch_driver

        def capture(*args, **kwargs):
            process = original_launch(*args, **kwargs)
            self.processes.append(process)
            return process

        log_path = self.directory / "failed.log"
        with patch.object(suite, "TEST_ARGS", ["-c", self.report_code()]), \
             patch.object(suite, "launch_driver", side_effect=capture), \
             patch.object(target[0], target[1], side_effect=error, create=True), \
             self.assertRaisesRegex(OSError, str(error)):
            suite.protocol_call(self.entry, self.request, log_path, timeout=5)
        self.assertEqual(len(self.processes), 1)
        self.assertIsNotNone(self.processes[0].poll())
        self.assertFalse(Path(self.request["output"]).exists())
        self.assertTrue(log_path.exists(), "Preserve the failed attempt's log")

    def test_assignment_failure_reaps_child_without_executing_it(self):
        self.check_startup_failure((host.OwnedJob, "assign"), OSError("assignment denied"))

    def test_resume_failure_reaps_child_without_executing_it(self):
        self.check_startup_failure((host, "_resume_primary_thread"), OSError("resume denied"))

    def check_gpu_startup_failure(self, target, error):
        original_popen, original_job = subprocess.Popen, host.OwnedJob
        created_processes, created_jobs = [], []

        def capture_process(*args, **kwargs):
            process = original_popen(*args, **kwargs)
            created_processes.append(process)
            self.processes.append(process)
            self.logs.append(process.stdout)
            return process

        def capture_job(*args, **kwargs):
            job = original_job(*args, **kwargs)
            created_jobs.append(job)
            self.jobs.append(job)
            return job

        # Exercise ownership and pipe cleanup without depending on GPU counters.
        command = [sys.executable, "-c", "import time; time.sleep(60)"]
        with patch.object(host, "_powershell_command", return_value=command), \
             patch.object(host.subprocess, "Popen", side_effect=capture_process), \
             patch.object(host, "OwnedJob", side_effect=capture_job), \
             patch.object(target[0], target[1], side_effect=error), \
             self.assertRaisesRegex(type(error), str(error)):
            host._GpuObserver()
        self.assertEqual(len(created_processes), 1)
        process = created_processes[0]
        self.assertIsNotNone(process.poll(), "Failed observer must reap its child")
        self.assertTrue(process.stdout.closed, "Failed observer must close its pipe")
        for job in created_jobs:
            self.assertIsNone(job._handle, "Failed observer must close its owned job")

    def test_gpu_ownership_failure_closes_reaped_child_pipe(self):
        self.check_gpu_startup_failure(
            (host.OwnedJob, "assign"), OSError("observer assignment denied"))

    def test_gpu_reader_setup_failure_reaps_child_and_closes_job_and_pipe(self):
        for target in [(host.threading, "Thread"), (host.threading.Thread, "start")]:
            with self.subTest(target=target[1]):
                self.check_gpu_startup_failure(target, RuntimeError("reader unavailable"))


@unittest.skipUnless(os.name == "nt", "Windows thread resume")
class ThreadResumeTests(unittest.TestCase):
    def setUp(self):
        self.exit_code = None
        self.process = SimpleNamespace(pid=42, poll=lambda: self.exit_code)

    def library(self, rows, *, snapshot=99, open_handle=77, resume_count=1,
                next_error=18, entry_size=28, opened_owner=42):
        self.closed, self.opened, self.resumed = [], [], []
        entries = iter(rows)

        def advance(_snapshot, pointer):
            try:
                owner, thread = next(entries)
            except StopIteration:
                ctypes.set_last_error(next_error)
                return 0
            pointer._obj.size = entry_size
            pointer._obj.ownerProcessId, pointer._obj.threadId = owner, thread
            return 1

        def open_thread(access, inherit, thread):
            self.opened.append((access, inherit, thread))
            ctypes.set_last_error(5)
            return open_handle

        def resume(thread):
            self.resumed.append(thread)
            ctypes.set_last_error(5)
            return resume_count

        def process_id(_thread):
            ctypes.set_last_error(5)
            return opened_owner

        return SimpleNamespace(
            CreateToolhelp32Snapshot=lambda *_: snapshot,
            Thread32First=advance, Thread32Next=advance,
            OpenThread=open_thread, GetProcessIdOfThread=process_id, ResumeThread=resume,
            CloseHandle=lambda handle: self.closed.append(handle) or 1)

    def test_only_owned_primary_thread_is_resumed_and_handles_close(self):
        library = self.library([(13, 21), (42, 22), (14, 23)])
        host._resume_primary_thread(self.process, library)
        self.assertEqual(self.opened, [(0x0802, False, 22)])
        self.assertEqual(self.resumed, [77])
        self.assertEqual(self.closed, [99, 77])

    def test_missing_or_ambiguous_thread_does_not_resume_and_closes_snapshot(self):
        for rows in [[], [(13, 21)], [(42, 22), (42, 23)]]:
            with self.subTest(rows=rows):
                library = self.library(rows)
                with self.assertRaises(OSError):
                    host._resume_primary_thread(self.process, library)
                self.assertEqual(self.closed, [99])
                self.assertEqual(self.opened, [])
                self.assertEqual(self.resumed, [])

    def test_enumeration_error_cannot_be_treated_as_a_complete_snapshot(self):
        library = self.library([(42, 22)], next_error=5)
        with self.assertRaisesRegex(OSError, "Thread32Next"):
            host._resume_primary_thread(self.process, library)
        self.assertEqual(self.closed, [99])
        self.assertEqual(self.opened, [])

    def test_incomplete_thread_record_does_not_open_an_unverified_owner(self):
        library = self.library([(42, 22)], entry_size=8)
        with self.assertRaisesRegex(OSError, "ownership"):
            host._resume_primary_thread(self.process, library)
        self.assertEqual(self.closed, [99])
        self.assertEqual(self.opened, [])

    def test_open_thread_failure_closes_only_the_valid_snapshot(self):
        library = self.library([(42, 22)], open_handle=None)
        with self.assertRaisesRegex(OSError, "OpenThread"):
            host._resume_primary_thread(self.process, library)
        self.assertEqual(self.closed, [99])
        self.assertEqual(self.resumed, [])

    def test_resume_error_and_unexpected_suspend_counts_close_both_handles(self):
        for previous in [0xFFFFFFFF, 0, 2]:
            with self.subTest(previous=previous):
                library = self.library([(42, 22)], resume_count=previous)
                with self.assertRaises(OSError):
                    host._resume_primary_thread(self.process, library)
                self.assertEqual(self.closed, [99, 77])

    def test_invalid_snapshot_is_never_closed_as_a_valid_handle(self):
        library = self.library([(42, 22)], snapshot=host.HANDLE(-1).value)
        with self.assertRaisesRegex(OSError, "CreateToolhelp32Snapshot"):
            host._resume_primary_thread(self.process, library)
        self.assertEqual(self.closed, [])
        self.assertEqual(self.opened, [])

    def test_reopened_thread_requires_verified_handle_ownership(self):
        for owner in [0, 99]:
            with self.subTest(owner=owner):
                library = self.library([(42, 22)], opened_owner=owner)
                with self.assertRaises(OSError):
                    host._resume_primary_thread(self.process, library)
                self.assertEqual(self.resumed, [])
                self.assertEqual(self.closed, [99, 77])

    def test_exited_child_cannot_resume_a_reused_process_identity(self):
        self.exit_code = 0
        library = self.library([(42, 22)])
        with self.assertRaises(OSError):
            host._resume_primary_thread(self.process, library)
        self.assertEqual(self.resumed, [])
        self.assertEqual(self.closed, [99, 77])


if __name__ == "__main__":
    unittest.main()
