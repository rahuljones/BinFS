#!/usr/bin/env bash

set -euo pipefail

TESTBED_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${TESTBED_DIR}/.." && pwd)"
STATE_DIR="${BINFS_STATE_DIR:-/tmp/binfs-eval-${UID}}"
MOUNT_POINT="${BINFS_MOUNT_POINT:-${STATE_DIR}/mount}"
RESULTS_ROOT="${BINFS_RESULTS_DIR:-${TESTBED_DIR}/results}"
BASE_PORT="${BINFS_BASE_PORT:-39000}"
BACKEND_0="127.0.0.1:${BASE_PORT}"
BACKEND_1="127.0.0.1:$((BASE_PORT + 1))"
BACKEND_2="127.0.0.1:$((BASE_PORT + 2))"
KEEPER="127.0.0.1:$((BASE_PORT + 3))"
BACKS_CSV="${BACKEND_0},${BACKEND_1},${BACKEND_2}"
CONFIG_FILE="${STATE_DIR}/bins.json"
LOG_DIR="${STATE_DIR}/logs"

export REPO_ROOT STATE_DIR MOUNT_POINT RESULTS_ROOT CONFIG_FILE LOG_DIR BACKS_CSV

pid_is_running() {
  local pid_file="$1"
  [[ -f "${pid_file}" ]] && kill -0 "$(cat "${pid_file}")" 2>/dev/null
}

wait_for_port() {
  local host="$1"
  local port="$2"
  local pid_file="$3"
  local attempts="${4:-100}"
  local attempt

  if ! command -v nc >/dev/null 2>&1; then
    sleep 1
    pid_is_running "${pid_file}"
    return
  fi

  for attempt in $(seq 1 "${attempts}"); do
    if nc -z "${host}" "${port}" >/dev/null 2>&1; then
      return
    fi
    if ! pid_is_running "${pid_file}"; then
      return 1
    fi
    sleep 0.1
  done
  return 1
}

mount_is_active() {
  local canonical_mount="${MOUNT_POINT}"
  if [[ -d "${MOUNT_POINT}" ]]; then
    canonical_mount="$(cd "${MOUNT_POINT}" && pwd -P)"
  fi
  mount | grep -F "${canonical_mount}" >/dev/null 2>&1
}

require_cluster() {
  if ! pid_is_running "${STATE_DIR}/back.pid"; then
    echo "BinFS backends are not running. Run testbed/start.sh first." >&2
    exit 1
  fi
}

require_mount() {
  require_cluster
  if ! mount_is_active; then
    echo "BinFS is not mounted at ${MOUNT_POINT}. Run testbed/start.sh without BINFS_SKIP_MOUNT." >&2
    exit 1
  fi
}
