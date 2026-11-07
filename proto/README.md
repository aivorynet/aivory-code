# proto

Machine-readable files of the protocol between `aivoryd` and its clients.
The Rust types are in `crates/aivory-protocol`.

- `fixtures/`: one JSON file for each demo RPC result. The Rust and Flutter tests read them.
- `schema/aivory-protocol.schema.json`: the JSON Schema of the whole surface.
- Regenerate both with `AIVORY_BLESS=1 cargo test -p aivory-protocol`.

Status: the wire format is JSON-RPC 2.0. Protobuf frames (ADR 0005) come later.
