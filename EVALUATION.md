# BinFS Evaluation

The evaluation has four layers:

1. Pure metadata, chunk, and service tests.
2. Concurrent service tests using synchronized Tokio tasks.
3. An integration test through three real Lab 3 gRPC backends.
4. A local testbed with three backends, one keeper, a FUSE mount, shell tests,
   and CSV-producing performance experiments.

## Automated correctness tests

Run:

```sh
cargo test -p binfs
```

The tests cover:

- Parent existence and nonempty-directory invariants.
- First-writer-wins behavior for concurrent same-name creates.
- Last-valid-commit behavior without torn file contents.
- Deterministic metadata replay.
- Empty, boundary-sized, missing, truncated, corrupt, and reordered chunks.
- Concurrent parent removal versus child creation.
- Concurrent readers and writers.
- End-to-end operation through replicated RPC storage.

The concurrency tests use barriers so operations begin together and repeat
race-sensitive cases rather than relying on one scheduling outcome.

## Local FUSE testbed

On macOS, install macFUSE before running the mounted tests. Then run:

```sh
testbed/run-all.sh
```

The testbed starts:

- Three Lab 3 storage backends on ports 39000-39002.
- One keeper configured on port 39003.
- One `binfs-mount` process.
- A mount at `/tmp/binfs-eval-$UID/mount`.

Override the locations or ports with:

```sh
BINFS_BASE_PORT=40000 \
BINFS_STATE_DIR=/tmp/my-binfs-eval \
BINFS_MOUNT_POINT=/tmp/my-binfs-mount \
testbed/run-all.sh
```

Individual stages are also available:

```sh
testbed/start.sh
testbed/functional.sh
testbed/concurrency.sh
testbed/benchmark.sh
testbed/repeat-benchmark.sh
testbed/stop.sh
```

To run only the RPC service benchmarks without macFUSE:

```sh
BINFS_SKIP_MOUNT=1 testbed/start.sh
testbed/benchmark.sh
testbed/stop.sh
```

## Performance experiments

`binfs-eval` writes one aggregate CSV row per experimental condition. Each row
contains sample counts, failures, p50/p95/p99/mean latency, operations per
second, and MiB per second.

The service experiments measure:

| Experiment | Independent variable | Operations |
| --- | --- | --- |
| Metadata scaling | Metadata log entries | Snapshot, root listing, read, mkdir |
| File scaling | File size and chunk size | Commit/write and read |
| Concurrency | Simultaneous clients | Create-and-write distinct files |

The filesystem experiment runs the same read/write and directory-listing
measurements against both the FUSE mount and a native local directory.

Defaults are 5 warmups and 30 measured iterations. For a quick smoke run:

The default file sizes are 4 KiB, 64 KiB, and 1 MiB. The default chunk sizes
are 64 KiB and 1 MiB. Larger files and 4 KiB chunks remain available through
the environment variables below, but are omitted from routine runs because
they make the sequential RPC data path disproportionately slow.

```sh
BINFS_EVAL_ITERATIONS=2 \
BINFS_EVAL_WARMUP=1 \
BINFS_EVAL_METADATA_ENTRIES=0,10 \
BINFS_EVAL_FILE_SIZES=4096,65536 \
BINFS_EVAL_CHUNK_SIZES=65536,1048576 \
BINFS_EVAL_CLIENTS=1,4 \
testbed/benchmark.sh
```

Results are stored under `testbed/results/<timestamp>/`.

Run the default five repetitions with:

```sh
testbed/repeat-benchmark.sh
```

## Reporting

Use the generated CSV files for these primary figures:

1. Operation latency versus metadata-log length.
2. Read/write throughput versus file size, grouped by chunk size.
3. Throughput and p95 latency versus concurrent client count.
4. BinFS FUSE latency and throughput relative to the native filesystem.

Report all failures instead of dropping failed samples. Run the full benchmark
at least five times and show variation across runs.

Fault injection is intentionally outside the main project claim. BinFS uses the
Lab 3 replicated storage layer, but this evaluation only verifies the
filesystem layer over that storage. Do not claim new BinFS-specific backend
failure guarantees without adding process-isolated backend fault tests.
