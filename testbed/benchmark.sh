#!/usr/bin/env bash

set -euo pipefail
source "$(cd "$(dirname "$0")" && pwd)/common.sh"
require_cluster

run_id="${BINFS_RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
result_dir="${RESULTS_ROOT}/${run_id}"
iterations="${BINFS_EVAL_ITERATIONS:-30}"
warmup="${BINFS_EVAL_WARMUP:-5}"
metadata_entries="${BINFS_EVAL_METADATA_ENTRIES:-0,100,1000,5000,10000}"
file_sizes="${BINFS_EVAL_FILE_SIZES:-4096,65536,1048576}"
chunk_sizes="${BINFS_EVAL_CHUNK_SIZES:-65536,1048576}"
clients="${BINFS_EVAL_CLIENTS:-1,2,4,8,16}"

mkdir -p "${result_dir}"
cd "${REPO_ROOT}"
cargo build --release -p binfs --bin binfs-eval

"${REPO_ROOT}/target/release/binfs-eval" service \
  --backs "${BACKS_CSV}" \
  --output "${result_dir}/service.csv" \
  --iterations "${iterations}" \
  --warmup "${warmup}" \
  --metadata-entries "${metadata_entries}" \
  --file-sizes "${file_sizes}" \
  --chunk-sizes "${chunk_sizes}" \
  --clients "${clients}"

native_root="${STATE_DIR}/native"
mkdir -p "${native_root}"
"${REPO_ROOT}/target/release/binfs-eval" filesystem \
  --root "${native_root}" \
  --label native \
  --output "${result_dir}/native.csv" \
  --iterations "${iterations}" \
  --warmup "${warmup}" \
  --file-sizes "${file_sizes}"

if mount_is_active; then
  "${REPO_ROOT}/target/release/binfs-eval" filesystem \
    --root "${MOUNT_POINT}" \
    --label binfs-fuse \
    --output "${result_dir}/fuse.csv" \
    --iterations "${iterations}" \
    --warmup "${warmup}" \
    --file-sizes "${file_sizes}"
else
  echo "Skipping FUSE benchmark because ${MOUNT_POINT} is not mounted" >&2
fi

{
  echo "run_id=${run_id}"
  echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "backs=${BACKS_CSV}"
  echo "iterations=${iterations}"
  echo "warmup=${warmup}"
  echo "metadata_entries=${metadata_entries}"
  echo "file_sizes=${file_sizes}"
  echo "chunk_sizes=${chunk_sizes}"
  echo "clients=${clients}"
  uname -a
} >"${result_dir}/environment.txt"

echo "Benchmark results written to ${result_dir}"
