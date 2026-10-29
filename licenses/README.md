# Runtime notices

`musl-COPYRIGHT` is the unchanged musl 1.2.5 copyright notice, obtained from
the [source mirror](https://github.com/ifduyue/musl/blob/v1.2.5/COPYRIGHT).
The upstream source repository is <https://git.musl-libc.org/cgit/musl/>.

The release notice generator also includes the pinned Rust toolchain's standard
library copyright/license document and license files from the locked Cargo graph.
These notices accompany the executable in archives and Docker images.

`linux-video-core-0.1.1-LICENSE` is the unchanged MIT license shipped in
`linux-video` 0.1.1. The `linux-video-core` 0.1.1 tarball omits that text but
declares MIT. Both crates' `.cargo_vcs_info.json` identify upstream commit
[`5802bee38487328dc579042deaf71ee44a589aff`](https://github.com/katyo/linux-video-rs/tree/5802bee38487328dc579042deaf71ee44a589aff),
with the core crate at `core/` and the shared license at the repository root.
The notice generator uses this fallback only for that exact package/version.

The `parse-display` / `parse-display-derive` 0.8.2 and `structmeta` /
`structmeta-derive` 0.2.0 tarballs also omit their workspace license files.
Their manifests offer MIT OR Apache-2.0; we include the unchanged MIT option:

- `parse-display-0.8.2-LICENSE-MIT`: [upstream text](https://github.com/frozenlib/parse-display/blob/5513dad782890b748217446eb66257c4b8bced79/LICENSE-MIT).
- `structmeta-0.2.0-LICENSE-MIT`: [upstream text](https://github.com/frozenlib/structmeta/blob/6247378b5d56d02887461d62f46f59873d097a92/LICENSE-MIT).

These commits match the respective crates' `.cargo_vcs_info.json`; fallbacks
are restricted to the exact package versions above.
