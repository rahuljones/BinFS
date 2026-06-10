#!/usr/bin/env bash

set -euo pipefail
TESTBED_DIR="$(cd "$(dirname "$0")" && pwd)"

cleanup() {
  "${TESTBED_DIR}/stop.sh" >/dev/null 2>&1 || true
}
trap cleanup EXIT

"${TESTBED_DIR}/start.sh"
"${TESTBED_DIR}/functional.sh"
"${TESTBED_DIR}/concurrency.sh"
"${TESTBED_DIR}/benchmark.sh"

cleanup
trap - EXIT
echo "Complete BinFS evaluation passed"
