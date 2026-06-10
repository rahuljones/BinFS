#!/usr/bin/env bash

set -euo pipefail
source "$(cd "$(dirname "$0")" && pwd)/common.sh"
require_mount

local_tmp="$(mktemp -d "${TMPDIR:-/tmp}/binfs-concurrency.XXXXXX")"
case_dir="${MOUNT_POINT}/concurrency-$$"

cleanup() {
  rm -rf "${case_dir}" "${local_tmp}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir "${case_dir}"

for client in $(seq 0 7); do
  (
    set +e
    mkdir "${case_dir}/same-directory" >/dev/null 2>&1
    echo "$?" >"${local_tmp}/mkdir-${client}.status"
  ) &
done
wait

mkdir_successes="$(awk '$1 == 0 { count++ } END { print count + 0 }' "${local_tmp}"/mkdir-*.status)"
if [[ "${mkdir_successes}" != "1" ]]; then
  echo "expected one concurrent mkdir winner, found ${mkdir_successes}" >&2
  exit 1
fi
rmdir "${case_dir}/same-directory"

for client in $(seq 0 7); do
  {
    printf 'writer-%s\n' "${client}"
    dd if=/dev/zero bs=1024 count=64 2>/dev/null
  } >"${local_tmp}/payload-${client}.bin"
  cat "${local_tmp}/payload-${client}.bin" >"${case_dir}/shared.bin" &
done
wait

matched=0
for client in $(seq 0 7); do
  if cmp -s "${local_tmp}/payload-${client}.bin" "${case_dir}/shared.bin"; then
    matched=1
  fi
done
if [[ "${matched}" != "1" ]]; then
  echo "concurrent writers produced torn or unexpected contents" >&2
  exit 1
fi

for client in $(seq 0 15); do
  cat "${local_tmp}/payload-0.bin" >"${case_dir}/file-${client}.bin" &
done
wait

entry_count="$(
  find "${case_dir}" -type f ! -name '._*' | wc -l | tr -d ' '
)"
if [[ "${entry_count}" != "17" ]]; then
  echo "expected 17 files after concurrent distinct creates, found ${entry_count}" >&2
  exit 1
fi

rm "${case_dir}"/*.bin
rm -f "${case_dir}"/._*.bin
rmdir "${case_dir}"
trap - EXIT
rm -rf "${local_tmp}"
echo "FUSE concurrency tests passed"
