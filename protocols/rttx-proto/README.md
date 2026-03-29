# rttx-proto

Shared protobuf wire protocol for the rttx client and daemon.

This crate defines:

- protobuf message types
- length-prefixed frame encoding and decoding
- UUID helpers shared by `rttx` and `rttx-server`

From the repository root:

```bash
cargo build -p rttx-proto
cargo test -p rttx-proto
```

This crate is developed and built as part of the workspace. It is not installed as a standalone
runtime artifact.
