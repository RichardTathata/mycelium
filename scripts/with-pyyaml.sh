#!/usr/bin/env bash
# Run a Python script that needs PyYAML (the test-inventory check and its mutation suite): with the
# system python3 if it has PyYAML, else with a venv made once under target/ (PEP 668 forbids installing
# into a distribution's python, so `pip install` there is not an option on current Ubuntu or Homebrew).
set -euo pipefail
if python3 -c 'import yaml, tomllib' 2>/dev/null; then
  exec python3 "$@"
fi
venv="${CARGO_TARGET_DIR:-target}/check-venv"
if ! "$venv/bin/python" -c 'import yaml' 2>/dev/null; then
  python3 -m venv "$venv"
  "$venv/bin/pip" install --quiet --disable-pip-version-check pyyaml
fi
exec "$venv/bin/python" "$@"
