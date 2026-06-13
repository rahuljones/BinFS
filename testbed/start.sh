#!/usr/bin/env bash

set -euo pipefail
source "$(cd "$(dirname "$0")" && pwd)/common.sh"

mkdir -p "${STATE_DIR}" "${LOG_DIR}" "${MOUNT_POINT}" "${RESULTS_ROOT}"

if pid_is_running "${STATE_DIR}/back.pid"; then
  echo "BinFS testbed is already running in ${STATE_DIR}"
  exit 0
fi

cleanup_on_error() {
  local status="$?"
  trap - EXIT
  if [[ "${status}" != "0" ]]; then
    "${TESTBED_DIR}/stop.sh" >/dev/null 2>&1 || true
  fi
  exit "${status}"
}
trap cleanup_on_error EXIT

cd "${REPO_ROOT}"
cargo build --release -p cmd --bins
cargo build --release -p binfs --bin binfs-eval
if [[ "${BINFS_SKIP_MOUNT:-0}" != "1" ]]; then
  cargo build --release -p binfs --features mount --bin binfs-mount
fi

cat >"${CONFIG_FILE}" <<EOF
{
  "backs": [
    "${BACKEND_0}",
    "${BACKEND_1}",
    "${BACKEND_2}"
  ],
  "keepers": [
    "${KEEPER}"
  ]
}
EOF

"${REPO_ROOT}/target/release/bins-back" \
  --cfg "${CONFIG_FILE}" \
  >"${LOG_DIR}/back.log" 2>&1 &
echo "$!" >"${STATE_DIR}/back.pid"

if ! wait_for_port 127.0.0.1 "${BASE_PORT}" "${STATE_DIR}/back.pid"; then
  echo "Backends failed to start. See ${LOG_DIR}/back.log" >&2
  exit 1
fi
if ! wait_for_port 127.0.0.1 "$((BASE_PORT + 1))" "${STATE_DIR}/back.pid"; then
  echo "Backends failed to start. See ${LOG_DIR}/back.log" >&2
  exit 1
fi
if ! wait_for_port 127.0.0.1 "$((BASE_PORT + 2))" "${STATE_DIR}/back.pid"; then
  echo "Backends failed to start. See ${LOG_DIR}/back.log" >&2
  exit 1
fi

"${REPO_ROOT}/target/release/bins-keep" \
  --config "${CONFIG_FILE}" \
  >"${LOG_DIR}/keeper.log" 2>&1 &
echo "$!" >"${STATE_DIR}/keeper.pid"
sleep 1
if ! pid_is_running "${STATE_DIR}/keeper.pid"; then
  echo "Keeper failed to start. See ${LOG_DIR}/keeper.log" >&2
  exit 1
fi

if [[ "${BINFS_SKIP_MOUNT:-0}" == "1" ]]; then
  trap - EXIT
  echo "RPC testbed ready: ${BACKS_CSV}"
  exit 0
fi

metadata_bin="__fs_eval_$(date +%Y%m%d_%H%M%S)_$$__"
"${REPO_ROOT}/target/release/binfs-mount" \
  --backs "${BACKS_CSV}" \
  --mount "${MOUNT_POINT}" \
  --metadata-bin "${metadata_bin}" \
  >"${LOG_DIR}/mount.log" 2>&1 &
echo "$!" >"${STATE_DIR}/mount.pid"

for _ in $(seq 1 100); do
  if mount_is_active; then
    trap - EXIT
    echo "BinFS mounted at ${MOUNT_POINT}"
    exit 0
  fi
  if ! pid_is_running "${STATE_DIR}/mount.pid"; then
    break
  fi
  sleep 0.1
done

echo "BinFS failed to mount. See ${LOG_DIR}/mount.log" >&2
exit 1
