"""Run the ignored real-created-app root integration test in the released Cargo slot."""
import argparse
import os
import pathlib
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--ready', action='store_true', required=True)
    parser.add_argument('--cli', type=pathlib.Path, help='Optional existing CLI; otherwise Cargo supplies its built binary')
    parser.add_argument('--target-dir', default='target/desktop-check')
    args = parser.parse_args()
    env = os.environ.copy()
    if args.cli:
        env['REVENANT_TEST_CLI'] = str(args.cli.resolve(strict=True))
    root = pathlib.Path(__file__).resolve().parents[1]
    subprocess.run(['cargo', 'test', '-p', 'revenant', '--locked', '--target-dir', args.target_dir,
                    '--test', 'desktop_cli_workflows', '--', '--ignored', '--nocapture', '--test-threads=1'],
                   cwd=root, env=env, check=True)


if __name__ == '__main__':
    main()
