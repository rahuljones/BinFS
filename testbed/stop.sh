#!/usr/bin/env bash

set -euo pipefail
source "$(cd "$(dirname "$0")" && pwd)/common.sh"

if mount_is_active; then
  if command -v fusermount3 >/dev/null 2>&1; then
    fusermount3 -u "${MOUNT_POINT}" >/dev/null 2>&1 || true
  elif command -v fusermount >/dev/null 2>&1; then
    fusermount -u "${MOUNT_POINT}" >/dev/null 2>&1 || true
  else
    umount "${MOUNT_POINT}" >/dev/null 2>&1 || true
  fi
fi

for name in mount keeper back; do
  pid_file="${STATE_DIR}/${name}.pid"
  if pid_is_running "${pid_file}"; then
    kill "$(cat "${pid_file}")" >/dev/null 2>&1 || true
  fi
done

for name in mount keeper back; do
  pid_file="${STATE_DIR}/${name}.pid"
  if [[ -f "${pid_file}" ]]; then
    pid="$(cat "${pid_file}")"
    for _ in $(seq 1 50); do
      if ! kill -0 "${pid}" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    if kill -0 "${pid}" 2>/dev/null; then
      kill -KILL "${pid}" >/dev/null 2>&1 || true
    fi
    rm -f "${pid_file}"
  fi
done

echo "BinFS testbed stopped"
