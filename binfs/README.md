# binfs

`binfs` exposes the lab3 replicated bin storage as a small FUSE filesystem.
The core service can be tested without mounting. The `binfs-mount` binary uses
the `fuser` crate and therefore requires a working FUSE installation.

On macOS, install macFUSE before building or running the mount binary.

```sh
cargo run -p binfs --features mount --bin binfs-mount -- \
  --backs 127.0.0.1:9000,127.0.0.1:9001 \
  --mount /tmp/binfs
```

The first version is scoped to `ls`, `cat`, `cp`, `rm`, `mkdir`, and `rmdir`.
It stores filesystem metadata in an append-only operation log and stores file
contents as fixed-size chunks distributed across data bins.
