#!/usr/bin/env bash

set -euo pipefail
source "$(cd "$(dirname "$0")" && pwd)/common.sh"

FAULT_BASE_PORT="${BINFS_FAULT_BASE_PORT:-$((BASE_PORT + 100))}"
FAULT_STATE_DIR="${BINFS_FAULT_STATE_DIR:-${STATE_DIR}/fault}"
FAULT_MOUNT_POINT="${BINFS_FAULT_MOUNT_POINT:-${FAULT_STATE_DIR}/mount}"
FAULT_CONFIG_FILE="${FAULT_STATE_DIR}/bins.json"
FAULT_LOG_DIR="${FAULT_STATE_DIR}/logs"
FAULT_REPAIR_WAIT="${BINFS_FAULT_REPAIR_WAIT:-5}"

FAULT_BACKEND_0="127.0.0.1:${FAULT_BASE_PORT}"
FAULT_BACKEND_1="127.0.0.1:$((FAULT_BASE_PORT + 1))"
FAULT_BACKEND_2="127.0.0.1:$((FAULT_BASE_PORT + 2))"
FAULT_KEEPER="127.0.0.1:$((FAULT_BASE_PORT + 3))"
FAULT_BACKS_CSV="${FAULT_BACKEND_0},${FAULT_BACKEND_1},${FAULT_BACKEND_2}"

MOUNT_POINT="${FAULT_MOUNT_POINT}"
local_tmp=""

kill_pid_file() {
  local pid_file="$1"
  if pid_is_running "${pid_file}"; then
    kill "$(cat "${pid_file}")" >/dev/null 2>&1 || true
  fi
}

wait_for_pid_exit() {
  local pid_file="$1"
  if [[ ! -f "${pid_file}" ]]; then
    return
  fi
  local pid
  pid="$(cat "${pid_file}")"
  for _ in $(seq 1 50); do
    if ! kill -0 "${pid}" 2>/dev/null; then
      rm -f "${pid_file}"
      return
    fi
    sleep 0.1
  done
  kill -KILL "${pid}" >/dev/null 2>&1 || true
  rm -f "${pid_file}"
}

unmount_fault_mount() {
  if ! mount_is_active; then
    return
  fi
  if command -v fusermount3 >/dev/null 2>&1; then
    fusermount3 -u "${FAULT_MOUNT_POINT}" >/dev/null 2>&1 || true
  elif command -v fusermount >/dev/null 2>&1; then
    fusermount -u "${FAULT_MOUNT_POINT}" >/dev/null 2>&1 || true
  else
    umount "${FAULT_MOUNT_POINT}" >/dev/null 2>&1 || true
  fi
}

cleanup() {
  set +e
  unmount_fault_mount
  kill_pid_file "${FAULT_STATE_DIR}/mount.pid"
  kill_pid_file "${FAULT_STATE_DIR}/keeper.pid"
  for index in 0 1 2; do
    kill_pid_file "${FAULT_STATE_DIR}/back-${index}.pid"
  done
  wait_for_pid_exit "${FAULT_STATE_DIR}/mount.pid"
  wait_for_pid_exit "${FAULT_STATE_DIR}/keeper.pid"
  for index in 0 1 2; do
    wait_for_pid_exit "${FAULT_STATE_DIR}/back-${index}.pid"
  done
  if [[ -n "${local_tmp}" ]]; then
    rm -rf "${local_tmp}"
  fi
}
trap cleanup EXIT

start_backend() {
  local index="$1"
  local address="$2"
  local pid_file="${FAULT_STATE_DIR}/back-${index}.pid"
  local port="${address##*:}"

  "${REPO_ROOT}/target/release/kv-server" \
    --address "${address}" \
    --log-level ERROR \
    >"${FAULT_LOG_DIR}/back-${index}.log" 2>&1 &
  echo "$!" >"${pid_file}"

  if ! wait_for_port 127.0.0.1 "${port}" "${pid_file}"; then
    echo "Backend ${index} failed to start. See ${FAULT_LOG_DIR}/back-${index}.log" >&2
    exit 1
  fi
}

stop_backend() {
  local index="$1"
  local pid_file="${FAULT_STATE_DIR}/back-${index}.pid"
  kill_pid_file "${pid_file}"
  wait_for_pid_exit "${pid_file}"
}

start_keeper() {
  "${REPO_ROOT}/target/release/bins-keep" \
    --config "${FAULT_CONFIG_FILE}" \
    --log-level ERROR \
    >"${FAULT_LOG_DIR}/keeper.log" 2>&1 &
  echo "$!" >"${FAULT_STATE_DIR}/keeper.pid"
  sleep 1
  if ! pid_is_running "${FAULT_STATE_DIR}/keeper.pid"; then
    echo "Keeper failed to start. See ${FAULT_LOG_DIR}/keeper.log" >&2
    exit 1
  fi
}

start_mount() {
  local metadata_bin="__fs_fault_$(date +%Y%m%d_%H%M%S)_$$__"
  "${REPO_ROOT}/target/release/binfs-mount" \
    --backs "${FAULT_BACKS_CSV}" \
    --mount "${FAULT_MOUNT_POINT}" \
    --metadata-bin "${metadata_bin}" \
    --chunk-size 65536 \
    --data-bins 128 \
    >"${FAULT_LOG_DIR}/mount.log" 2>&1 &
  echo "$!" >"${FAULT_STATE_DIR}/mount.pid"

  for _ in $(seq 1 100); do
    if mount_is_active; then
      return
    fi
    if ! pid_is_running "${FAULT_STATE_DIR}/mount.pid"; then
      break
    fi
    sleep 0.1
  done

  echo "BinFS fault-tolerance mount failed. See ${FAULT_LOG_DIR}/mount.log" >&2
  exit 1
}

make_payload() {
  local label="$1"
  local kib="$2"
  local output="$3"
  {
    printf '%s\n' "${label}"
    dd if=/dev/zero bs=1024 count="${kib}" 2>/dev/null
  } >"${output}"
}

assert_matches() {
  local expected="$1"
  local actual="$2"
  if ! cmp -s "${expected}" "${actual}"; then
    echo "Expected ${actual} to match ${expected}" >&2
    exit 1
  fi
}

mkdir -p "${FAULT_STATE_DIR}" "${FAULT_LOG_DIR}" "${FAULT_MOUNT_POINT}"
local_tmp="$(mktemp -d "${TMPDIR:-/tmp}/binfs-fault.XXXXXX")"

cd "${REPO_ROOT}"
cargo build --release -p cmd --bin kv-server --bin bins-keep
cargo build --release -p binfs --features mount --bin binfs-mount

cat >"${FAULT_CONFIG_FILE}" <<EOF
{
  "backs": [
    "${FAULT_BACKEND_0}",
    "${FAULT_BACKEND_1}",
    "${FAULT_BACKEND_2}"
  ],
  "keepers": [
    "${FAULT_KEEPER}"
  ]
}
EOF

start_backend 0 "${FAULT_BACKEND_0}"
start_backend 1 "${FAULT_BACKEND_1}"
start_backend 2 "${FAULT_BACKEND_2}"
start_keeper
start_mount

case_dir="${FAULT_MOUNT_POINT}/fault-$$"
mkdir "${case_dir}"

make_payload "initial file before failure" 192 "${local_tmp}/initial.bin"
cp "${local_tmp}/initial.bin" "${case_dir}/initial.bin"
assert_matches "${local_tmp}/initial.bin" "${case_dir}/initial.bin"

stop_backend 0
sleep 2
assert_matches "${local_tmp}/initial.bin" "${case_dir}/initial.bin"

make_payload "file written while backend 0 is down" 128 "${local_tmp}/during-failure.bin"
cp "${local_tmp}/during-failure.bin" "${case_dir}/during-failure.bin"
assert_matches "${local_tmp}/during-failure.bin" "${case_dir}/during-failure.bin"

start_backend 0 "${FAULT_BACKEND_0}"
sleep "${FAULT_REPAIR_WAIT}"

stop_backend 1
sleep 2
assert_matches "${local_tmp}/initial.bin" "${case_dir}/initial.bin"
assert_matches "${local_tmp}/during-failure.bin" "${case_dir}/during-failure.bin"

make_payload "file written after backend 0 repair with backend 1 down" 64 "${local_tmp}/after-repair.bin"
cp "${local_tmp}/after-repair.bin" "${case_dir}/after-repair.bin"
assert_matches "${local_tmp}/after-repair.bin" "${case_dir}/after-repair.bin"

start_backend 1 "${FAULT_BACKEND_1}"
sleep "${FAULT_REPAIR_WAIT}"

stop_backend 2
sleep 2
assert_matches "${local_tmp}/initial.bin" "${case_dir}/initial.bin"
assert_matches "${local_tmp}/during-failure.bin" "${case_dir}/during-failure.bin"
assert_matches "${local_tmp}/after-repair.bin" "${case_dir}/after-repair.bin"

trap - EXIT
cleanup
echo "BinFS fault-tolerance evaluation passed"
