# krilla 0.8.2, patched for DesignCraft

Upstream: https://github.com/LaurenzV/krilla (MIT OR Apache-2.0; LICENSE_MIT, LICENSE_APACHE, from upstream main).
The crate as published on crates.io (0.8.2), used through `[patch.crates-io]` in the workspace
Cargo.toml.

## Patch

`src/text/cid.rs` and `src/text/type3.rs`: the ToUnicode CMap stream's dictionary gets
`/CMapName` and `/CIDSystemInfo` (`cmap.name(CMAP_NAME); cmap.system_info(SYSTEM_INFO);`).

krilla typed the stream `/Type /CMap` (pdf-writer's `Chunk::cmap`) and wrote both values only
inside the CMap program. A CMap stream's dictionary must carry them (PDF 32000-1, 9.7.5.3;
pdf-writer documents `Cmap::name` and `Cmap::system_info` as required). Strict readers refuse the
file: HexaPDF raises "Required field CMapName is not set" when it writes a DesignCraft PDF, so
book_write and the design studio untyped those streams before editing a PDF.

Still missing upstream as of 2026-10-15 (krilla main). Drop this copy once a release writes them.

`Cargo.toml`: `[lints.rust] deprecated = "allow"`, for upstream's deprecated `std::f32` constants,
which warn in a path dependency (crates.io builds hide them).
