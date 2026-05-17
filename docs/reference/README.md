# Brain — reference

Look-it-up material. Concise, accurate, complete. No prose,
no learning narrative — those live in
[`../tutorials/`](../tutorials/) and [`../guides/`](../guides/).

| Topic | File |
|---|---|
| Performance targets per phase | [`performance.md`](performance.md) |

Most low-level reference (the wire frame layout, opcode space,
storage records, redb tables, error code taxonomy, etc.) lives
in the authoritative spec at [`../../spec/`](../../spec/).
This directory holds reference material that doesn't belong
in the spec — operator-visible numbers, performance gates,
SLO targets — and rustdoc-style indexes when we add them.

## Other reference surfaces

- **API reference**: `cargo doc --workspace --no-deps --open`.
- **Wire protocol**: [`../../spec/03_wire_protocol/`](../../spec/03_wire_protocol/).
- **Error codes**: [`../../spec/03_wire_protocol/10_errors.md`](../../spec/03_wire_protocol/10_errors.md).
- **Tunables**: [`../guides/configure.md`](../guides/configure.md).
