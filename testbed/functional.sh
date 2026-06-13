#!/usr/bin/env bash

set -euo pipefail
source "$(cd "$(dirname "$0")" && pwd)/common.sh"
require_mount

local_tmp="$(mktemp -d "${TMPDIR:-/tmp}/binfs-functional.XXXXXX")"
case_dir="${MOUNT_POINT}/functional-$$"

cleanup() {
  rm -rf "${case_dir}" "${local_tmp}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir "${case_dir}"
mkdir "${case_dir}/dir"
{
  printf 'binfs-functional-test\n'
  dd if=/dev/zero bs=1024 count=192 2>/dev/null
} >"${local_tmp}/input.bin"

cp "${local_tmp}/input.bin" "${case_dir}/dir/file.bin"
cmp "${local_tmp}/input.bin" "${case_dir}/dir/file.bin"
cat "${case_dir}/dir/file.bin" >"${local_tmp}/cat.bin"
cmp "${local_tmp}/input.bin" "${local_tmp}/cat.bin"

printf 'file.bin\n' >"${local_tmp}/expected-ls.txt"
ls -1 "${case_dir}/dir" >"${local_tmp}/actual-ls.txt"
cmp "${local_tmp}/expected-ls.txt" "${local_tmp}/actual-ls.txt"

if mkdir "${case_dir}/dir" >/dev/null 2>&1; then
  echo "duplicate mkdir unexpectedly succeeded" >&2
  exit 1
fi
if rmdir "${case_dir}/dir" >/dev/null 2>&1; then
  echo "rmdir unexpectedly removed a nonempty directory" >&2
  exit 1
fi
if cat "${case_dir}/dir" >/dev/null 2>&1; then
  echo "cat unexpectedly read a directory" >&2
  exit 1
fi
if ls "${case_dir}/missing" >/dev/null 2>&1; then
  echo "ls unexpectedly found a missing path" >&2
  exit 1
fi

printf 'replacement\n' >"${local_tmp}/replacement.txt"
cp "${local_tmp}/replacement.txt" "${case_dir}/dir/file.bin"
cmp "${local_tmp}/replacement.txt" "${case_dir}/dir/file.bin"

rm "${case_dir}/dir/file.bin"
rmdir "${case_dir}/dir"
rmdir "${case_dir}"
trap - EXIT
rm -rf "${local_tmp}"
echo "FUSE functional tests passed"
