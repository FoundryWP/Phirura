//! A `zstd` replacement for Windows Phone 8.1.
//!
//! # Why
//!
//! `zstd` is a C library: the real crate drives `zstd-sys`, whose build script compiles
//! `libzstd` with `cc` and fails on this target exactly like `aws-lc-sys` and `ring` did - there
//! is no `thumbv7a-pc-windows-msvc` C toolchain in this build. Replacing the crate removes
//! `zstd-sys`/`zstd-safe` from the graph outright.
//!
//! # Why "store" instead of a pure-Rust zstd
//!
//! The only caller is `phira::censor`, which caches a serialized Aho-Corasick automaton next to
//! its data directory: `encode_all` on write, `decode_all` on read. The cache is an optimisation -
//! `load_cache` failing simply falls back to rebuilding the automaton - so compression is not a
//! correctness requirement, only a disk-space one. The pure-Rust `ruzstd` decodes but does not
//! encode, so it would not satisfy both call sites anyway.
//!
//! # The invariant this relies on
//!
//! Both ends of the format are this code: on the phone the cache is written by this shim and read
//! by this shim, so a byte-identical store passes through unchanged. That matters because
//! `censor::load_cache` validates only its own magic/version header before handing the payload to
//! `CharwiseDoubleArrayAhoCorasick::deserialize_unchecked`, under a `SAFETY` comment that assumes
//! "written by this program under the current format". Nothing else ever writes that path: the
//! phone's local storage is never shared with a desktop build, where a real zstd frame would live.
//!
//! (If the shim were ever swapped for real compression, old store-only caches would fail
//! `decode_all` with an error and rebuild - the graceful direction. The reverse direction is the
//! dangerous one, and it cannot happen here.)

use std::io::{self, Read, Write};

/// Reads `source` and returns it unchanged in `W`.
///
/// `W: Default` rather than a way to build `W` from the bytes, because that is the only bound a
/// generic `Write` does not already give: the sole call site infers `W = Vec<u8>` (it calls
/// `.len()` and `extend_from_slice(&..)` on the result), and `Vec<u8>` implements both.
pub fn encode_all<R: Read, W: Write + Default>(mut source: R, _level: i32) -> io::Result<W> {
    let mut raw = Vec::new();
    source.read_to_end(&mut raw)?;
    let mut out = W::default();
    out.write_all(&raw)?;
    Ok(out)
}

/// Returns `source` unchanged - see the module docs for why that is sound here.
pub fn decode_all<R: Read>(mut source: R) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    source.read_to_end(&mut out)?;
    Ok(out)
}
