"""Root CLI acceptance runner; native/core/SDK/client suites are owned separately."""
import argparse
import pathlib
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--ready', action='store_true', required=True,
                        help='Explicitly confirm main has released the Cargo slot')
    parser.add_argument('--standalone', action='store_true', help='Also run real new/custom-native/frontend proof')
    parser.add_argument('--target-dir', default='target/desktop-check')
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1]
    subprocess.run(['cargo', 'check', '-p', 'revenant', '--all-targets', '--locked', '--target-dir', args.target_dir], cwd=root, check=True)
    subprocess.run(['cargo', 'test', '-p', 'revenant', '--locked', '--target-dir', args.target_dir], cwd=root, check=True)
    if args.standalone:
        subprocess.run([sys.executable, str(root / 'tests/standalone_workflows.py'), '--ready', '--target-dir', args.target_dir], cwd=root, check=True)


if __name__ == '__main__':
    main()
