#!/usr/bin/env bash

set -euo pipefail
TESTBED_DIR="$(cd "$(dirname "$0")" && pwd)"
runs="${BINFS_EVAL_RUNS:-5}"
timestamp="$(date +%Y%m%d-%H%M%S)"

for run in $(seq 1 "${runs}"); do
  BINFS_RUN_ID="${timestamp}-run-${run}" "${TESTBED_DIR}/benchmark.sh"
done

echo "Completed ${runs} benchmark runs"
