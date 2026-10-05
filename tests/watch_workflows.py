"""Real dev watcher proof. Owns a disposable app and briefly opens a desktop window."""
import argparse
import json
import os
import pathlib
import queue
import re
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request


def port_open(port):
    try:
        with socket.create_connection(('127.0.0.1', port), timeout=.2):
            return True
    except OSError:
        return False


def stop(child):
    if child.poll() is not None:
        return
    try:
        if os.name == 'nt':
            child.send_signal(signal.CTRL_BREAK_EVENT)
        else:
            os.killpg(child.pid, signal.SIGINT)
        child.wait(timeout=20)
    except (OSError, subprocess.TimeoutExpired):
        if os.name == 'nt':
            subprocess.run(['taskkill.exe', '/PID', str(child.pid), '/T', '/F'], capture_output=True)
        else:
            os.killpg(child.pid, signal.SIGKILL)
        child.kill()
        child.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--cli', required=True, type=pathlib.Path)
    parser.add_argument('--ready', action='store_true', required=True)
    parser.add_argument('--allow-desktop-window', action='store_true', required=True,
                        help='This integration launches a visible disposable native window')
    parser.add_argument('--timeout', type=float, default=600)
    args = parser.parse_args()
    cli = args.cli.resolve(strict=True)
    fixtures = pathlib.Path(__file__).resolve().parent / 'fixtures/custom-native'
    with tempfile.TemporaryDirectory(prefix='revenant-watcher-') as directory:
        parent = pathlib.Path(directory)
        subprocess.run([str(cli), 'new', 'watcher-proof'], cwd=parent, check=True, timeout=args.timeout)
        project = parent / 'watcher-proof'
        for source in fixtures.rglob('*'):
            if source.is_file():
                target = project / 'native' / source.relative_to(fixtures)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(source.read_bytes())
        # Ask the OS for an unused local dev port rather than colliding with an app.
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            port = probe.getsockname()[1]
        config = project / 'revenant.toml'
        with config.open('a') as output:
            output.write(f'\n[desktop]\ndev_port = {port}\n')
        source = project / 'native/src/records.rs'
        original = source.read_text(encoding='utf-8')
        sdk_source = project / '.revenant/sdk/crates/revenant-sdk/src/lib.rs'
        sdk_original = sdk_source.read_bytes()
        entry = project / 'web/src/lib/revenant.ts'
        flags = {'creationflags': subprocess.CREATE_NEW_PROCESS_GROUP} if os.name == 'nt' else {'start_new_session': True}
        child = subprocess.Popen([str(cli), 'dev'], cwd=project, stdout=subprocess.PIPE,
                                 stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace', **flags)
        lines, history = queue.Queue(), []

        def read():
            for line in child.stdout:
                history.append(line)
                lines.put(line)
                if any(marker in line for marker in ['Desktop running.', 'Native rebuild failed;', 'Rebuilding native application', 'Native contract published;', 'All owned desktop']):
                    print(line.rstrip(), flush=True)
        reader = threading.Thread(target=read, daemon=True)
        reader.start()

        def wait(marker):
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise RuntimeError('dev exited: ' + ''.join(history[-30:]))
                try:
                    if marker in lines.get(timeout=.1):
                        return
                except queue.Empty:
                    pass
            raise TimeoutError(marker + ': ' + ''.join(history[-30:]))

        def current_description():
            match = re.search(r'\.revenant/([a-f0-9]{64})/facade', entry.read_text())
            assert match, 'missing generated facade pointer'
            contract = json.loads((entry.parent / '.revenant' / match[1] / 'revenant.contract.json').read_text())
            assert (contract['version'], contract['protocol']) == (3, 2)
            return next(op['description'] for op in contract['operations'] if op['id'] == 'records.normalize')

        def edit(document):
            source.write_text(original.replace('Normalize a title for the portable CLI proof.', document), encoding='utf-8')

        def current_shadow(strict=True):
            shadows = list((project / '.revenant/run').glob('*/watcher-proof.exe' if os.name == 'nt' else '*/watcher-proof'))
            if strict:
                assert len(shadows) == 1, f'expected one owned desktop snapshot, got {shadows}'
            return shadows[0] if len(shadows) == 1 else None

        try:
            wait('Desktop running.')
            first_shadow = current_shadow()
            before = entry.read_bytes()
            source.write_text(original + '\ncompile_error!("intentional watcher failure");\n', encoding='utf-8')
            wait('Native rebuild failed; previous generation retained:')
            assert entry.read_bytes() == before, 'failed compile replaced last successful facade'
            assert child.poll() is None
            assert current_shadow() == first_shadow, 'failed compile replaced the running desktop snapshot'
            with urllib.request.urlopen(f'http://127.0.0.1:{port}', timeout=10) as response:
                assert response.status == 200, 'failed native rebuild stopped the owned frontend'
            for text in ['watch-rapid-A', 'watch-rapid-B', 'watch-rapid-C']:
                edit(text)
                time.sleep(.1)
            wait('Rebuilding native application')
            edit('watch-final-D')  # A trailing edit must survive an already-running build.
            deadline = time.monotonic() + args.timeout
            while current_description().strip() != 'watch-final-D':
                assert child.poll() is None, ''.join(history[-30:])
                assert time.monotonic() < deadline, 'trailing edit was lost: ' + ''.join(history[-30:])
                time.sleep(.1)
            # Publication precedes restart by a few milliseconds; await the
            # completed restart before asserting snapshot ownership or saving again.
            while current_shadow(False) is None or current_shadow(False) == first_shadow:
                assert time.monotonic() < deadline, 'successful rebuild did not replace the loaded shadow'
                time.sleep(.1)
            # Drain old logs before triggering the SDK edit to avoid a false match.
            while not lines.empty():
                lines.get_nowait()
            sdk_source.write_bytes(sdk_original + b'\n// Local SDK watcher proof.\n')
            wait('Rebuilding native application')
            wait('Native contract published; desktop restarted.')
        finally:
            stop(child)
            reader.join(timeout=5)
            child.stdout.close()
        deadline = time.monotonic() + 10
        while port_open(port):
            assert time.monotonic() < deadline, 'owned Vite process survived CLI cleanup'
            time.sleep(.05)
        assert not list((project / '.revenant/run').glob('*')), 'owned desktop run directories survived cleanup'
        print('PASS: failed-build preservation, trailing edits, SDK watching, desktop/Vite process cleanup')


if __name__ == '__main__':
    main()
