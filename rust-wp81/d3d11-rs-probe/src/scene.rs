//! Stage 3: replay one recorded frame's command stream through D3D11.
//!
//! The port records every frame as a flat array of `PortCmd` (56 bytes each) plus side arenas of
//! vertices and indices, and hands those to the shell instead of a scene graph. This module is the
//! shell side: decode such a stream, check it structurally, flatten it into one shared vertex /
//! index / draw-item triple, and issue the draws.
//!
//! ## Where the contract comes from (read, not remembered)
//!
//! * `PortCmd`, `PortVertex`, the `KIND_*` tags, the flip bits and every `f[]` slot layout are
//!   defined by `encode()` in `shims/macroquad/src/lib.rs` (`pub mod draw`). The mirrors below are
//!   byte-for-byte copies of those records, pinned by the `const _` size asserts - this crate has
//!   zero dependencies on purpose (see its `Cargo.toml`), so it cannot `use macroquad::draw::*`;
//!   the mirrors are the boundary, and the asserts are what stop them drifting.
//! * The fixture file format is the one `dump_fixture()` writes in
//!   `prpr-port/src/bin/host_m1.rs`: a 0x24-byte little-endian prelude (`magic`, `version`,
//!   `frame`, `screen_w`, `screen_h`, `ncmds`, `nverts`, `nidx`, `ntextures`), then the four
//!   arrays, texture *pixels deliberately omitted* (the writer's doc explains why: 24 MiB of RGBA
//!   would make the fixture as big as the chart it exists to test).
//!
//! ## Coordinates are pixels, and this module does no camera math
//!
//! The recording side applies prpr's camera and model matrices *before* writing a command, so
//! every coordinate arriving here is already a screen pixel: origin top-left, y growing down,
//! viewport 768x1229 (the Lumia 925's resolution; the desktop harness matches it). Applying a
//! camera transform here would double-transform the frame - that is the whole point of the
//! design. The only matrix in this file is [`screen_projection`], which maps pixel coordinates to
//! D3D11 clip space; it depends on nothing but the recorded screen size and is the identity in
//! pixel space, i.e. the minimum any GPU renderer must do to see pixels at all. (The C# head had
//! the same kind of viewport mapping - "the C# renderer applies the same projection" per the shim's
//! own `PortVertex` doc.)
//!
//! Consequence for validation: structural checks below never assert "coordinate is on screen".
//! The only fixture recorded so far predates the pixel-space projection and still carries world
//! coordinates; re-recording it is a data refresh, not a code change, and until then a range
//! assertion would fail for reasons that have nothing to do with the renderer.
//!
//! ## Textures: unknown by design
//!
//! The fixture stores only `{id, w, h, bytes}` per texture - no pixels. Replaying therefore can
//! never bind the real image, and this module does not try: every texture id is drawn as a flat
//! quad through the pipeline's 1x1 white stand-in (UVs are still computed from the texture's
//! recorded dimensions so the path is true once a real SRV registry exists on the phone). That is
//! deliberate, and it is enough to check every geometric claim the frame makes.
//!
//! ## Where `unsafe` lives
//!
//! Exactly three places, each documented at the site: [`decode`] and [`build`] touch the fixed
//! arenas (`static mut`, same pattern as `lib.rs`'s `STATE` and the shim's own `PORT_*` buffers -
//! no per-frame allocation, single-threaded by contract), and [`draw`] is raw D3D11 vtable FFI,
//! which cannot be anything else. Everything decode, validate and build compute afterwards is
//! ordinary safe code over slices with bounds checked before every write.

#![allow(non_snake_case, dead_code)]

// `direct` re-exports its types but not this one, and `crate::c_void` is a private import in
// lib.rs - the FFI void pointer is named at every call site below, so it is imported here like
// every other file in this crate does (`direct.rs`, `lib.rs`, `hostcheck.rs`).
use core::ffi::c_void;

use crate::direct::*;
use crate::renderer::{Pipeline, Vertex, MAX_VERTICES, VERTEX_STRIDE};

// ===========================================================================
// wire contract, mirrored from shims/macroquad/src/lib.rs (`pub mod draw`)
// ===========================================================================

/// Command tags. `Text` is deliberately absent from this set: the shim records no text commands
/// (see the `KIND_*` note there), so the tag space is exactly 0..=4.
pub const KIND_TEXTURE: u32 = 0;
pub const KIND_RECT: u32 = 1;
pub const KIND_LINE: u32 = 2;
pub const KIND_CIRCLE: u32 = 3;
pub const KIND_GEOMETRY: u32 = 4;

/// `PortCmd::kind` packs the tag in the low 16 bits and two flags above it - **always mask
/// before comparing**, or the one texture command per frame that carries a flip reads as kind
/// 65536/65537 and falls off every `match`.
pub const KIND_MASK: u32 = 0xFFFF;
/// Bit 16 of `kind`: mirror the quad horizontally (set by `encode()` because the 8-slot payload
/// has no room left for flips).
pub const FLAG_FLIP_X: u32 = 1 << 16;
/// Bit 17 of `kind`: mirror vertically.
pub const FLAG_FLIP_Y: u32 = 1 << 17;

/// Byte-for-byte mirror of the shim's `PortCmd`. The field order and the fixed 56-byte size are
/// asserted below; the `f[]` meanings are per-kind and documented at each decode site in
/// [`build`], transcribed from `encode()`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct PortCmd {
    pub kind: u32,
    pub flags: u32,
    pub vstart: u32,
    pub vcount: u32,
    pub istart: u32,
    pub icount: u32,
    pub f: [f32; 8],
}

impl PortCmd {
    pub const ZERO: PortCmd = PortCmd {
        kind: 0,
        flags: 0,
        vstart: 0,
        vcount: 0,
        istart: 0,
        icount: 0,
        f: [0.0; 8],
    };
}

/// Mirror of the shim's `PortVertex`: position, uv, colour - **no alpha** (`encode()` copies
/// `color[0..2]` only) and `z` is 0 in practice. Replayed geometry is therefore opaque; alpha
/// survives only on the `Rect`/`Circle` payloads, which carry it in `f[]`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct PortVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub u: f32,
    pub v: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl PortVertex {
    pub const ZERO: PortVertex = PortVertex {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        u: 0.0,
        v: 0.0,
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };
}

/// One `{id, w, h, bytes}` row from the fixture's texture table. Pixels are not recorded - see
/// the module doc.
#[derive(Copy, Clone, Debug)]
pub struct TexInfo {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub bytes: u32,
}

impl TexInfo {
    pub const ZERO: TexInfo = TexInfo { id: 0, width: 0, height: 0, bytes: 0 };
}

// Lockstep with the writer, evaluated at compile time: if the shim grows a field these fail to
// build instead of silently mis-parsing every frame (host_m1's `dump_fixture` writes exactly
// `size_of::<PortCmd>()`/`size_of::<PortVertex>()` bytes per record).
const _: () = assert!(core::mem::size_of::<PortCmd>() == 56);
const _: () = assert!(core::mem::size_of::<PortVertex>() == 32);

/// Writer-side caps, mirrored from the shim so the reader can reject a corrupt header *before*
/// copying into the fixed arenas below: `PORT_CMD_CAP`, `PORT_VTX_CAP`, `ICAP`, `TEX_SLOTS`
/// (all in `shims/macroquad/src/lib.rs`).
const MAX_CMDS: usize = 8192;
const MAX_FVERTS: usize = 65536;
const MAX_FIDX: usize = 98304;
const MAX_TEXS: usize = 128;

/// Fixture container magic/version, from `dump_fixture()` in `prpr-port/src/bin/host_m1.rs`.
pub const FIXTURE_MAGIC: u32 = 0x5846_3150; // "P1FX"
pub const FIXTURE_VERSION: u32 = 1;
/// Size of the fixture prelude: magic..ntextures inclusive (9 little-endian words), and where
/// the command array starts.
pub const FIXTURE_HEADER: usize = 0x24;

/// One decoded frame. Slices borrow the arena [`decode`] filled (or, on the future phone path,
/// the FFI pointers C# passes in) - **a `Frame` is invalidated by the next `decode` call**, the
/// same single-threaded frame-drain contract the shim's own arenas have.
pub struct Frame<'a> {
    pub frame_id: u32,
    pub screen_w: f32,
    pub screen_h: f32,
    pub cmds: &'a [PortCmd],
    pub verts: &'a [PortVertex],
    pub idx: &'a [u16],
    pub texs: &'a [TexInfo],
}

// ===========================================================================
// decode: fixture bytes -> Frame
// ===========================================================================

/// Why decode is desktop-only: it exists to replay *recorded* fixtures under `hostcheck`. The
/// phone never parses a file - it reads the live arrays through pointers the shell hands it -
/// so the ~2.6 MiB of copy arenas below would be pure waste inside the ARM DLL's address space.
#[cfg(not(target_arch = "arm"))]
mod arena {
    use super::{PortCmd, PortVertex, TexInfo, MAX_CMDS, MAX_FIDX, MAX_FVERTS, MAX_TEXS};

    pub(super) static mut D_CMDS: [PortCmd; MAX_CMDS] = [PortCmd::ZERO; MAX_CMDS];
    pub(super) static mut D_VERTS: [PortVertex; MAX_FVERTS] = [PortVertex::ZERO; MAX_FVERTS];
    pub(super) static mut D_IDX: [u16; MAX_FIDX] = [0; MAX_FIDX];
    pub(super) static mut D_TEXS: [TexInfo; MAX_TEXS] = [TexInfo::ZERO; MAX_TEXS];
}

/// Why a fixture decode failed. Every variant is a *structural* property of the bytes; nothing
/// here ever inspects coordinate values (see the module doc).
#[cfg(not(target_arch = "arm"))]
#[derive(Debug)]
pub enum Fail {
    Short,
    BadMagic,
    BadVersion(u32),
    BadScreen,
    TooManyCmds,
    TooManyVerts,
    TooManyIdx,
    TooManyTexs,
    SizeMismatch,
}

#[cfg(not(target_arch = "arm"))]
impl Fail {
    pub fn name(&self) -> &'static str {
        match self {
            Fail::Short => "file shorter than the 0x24-byte prelude",
            Fail::BadMagic => "magic is not P1FX",
            Fail::BadVersion(_) => "unsupported fixture version",
            Fail::BadScreen => "screen_w/screen_h not finite and positive",
            Fail::TooManyCmds => "ncmds exceeds the shim's PORT_CMD_CAP",
            Fail::TooManyVerts => "nverts exceeds the shim's PORT_VTX_CAP",
            Fail::TooManyIdx => "nidx exceeds the shim's ICAP",
            Fail::TooManyTexs => "ntextures exceeds the shim's TEX_SLOTS",
            Fail::SizeMismatch => "file size != header counts (truncated or grown data)",
        }
    }
}

#[inline]
fn ru32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[inline]
fn rf32(b: &[u8]) -> f32 {
    f32::from_bits(ru32(b))
}

/// Parse a fixture into the shared arenas and hand back a view over them.
///
/// # Why this never panics
///
/// Every length is checked before a single slice is taken: prelude present, header counts within
/// the writer's caps (and *those first*, so the size arithmetic below cannot overflow a 32-bit
/// `usize`), then exact total size. `dump_fixture` writes `HEADER + the four arrays` and nothing
/// else, so the exact-size check **is** the "counts match the header" guarantee - a record count
/// that disagreed with the file length would show up here, not as a mid-stream overrun.
///
/// # Safety-adjacent contract
///
/// Single-threaded: the returned `Frame` shares storage with the next `decode`'s writes. That is
/// the same rule the shim's drain has (`drain_reset` before reuse), and the phone path is a UI
/// thread by construction.
#[cfg(not(target_arch = "arm"))]
pub fn decode(bytes: &[u8]) -> Result<Frame<'static>, Fail> {
    if bytes.len() < FIXTURE_HEADER {
        return Err(Fail::Short);
    }
    if ru32(&bytes[0..4]) != FIXTURE_MAGIC {
        return Err(Fail::BadMagic);
    }
    let version = ru32(&bytes[4..8]);
    if version != FIXTURE_VERSION {
        return Err(Fail::BadVersion(version));
    }
    let frame_id = ru32(&bytes[8..12]);
    let screen_w = rf32(&bytes[12..16]);
    let screen_h = rf32(&bytes[16..20]);
    if !screen_w.is_finite() || screen_w <= 0.0 || !screen_h.is_finite() || screen_h <= 0.0 {
        return Err(Fail::BadScreen);
    }
    let ncmds = ru32(&bytes[20..24]) as usize;
    let nverts = ru32(&bytes[24..28]) as usize;
    let nidx = ru32(&bytes[28..32]) as usize;
    let ntextures = ru32(&bytes[32..36]) as usize;

    // Caps before arithmetic: with each count bounded by its arena size the products below are a
    // few MiB at most and cannot wrap even on the 32-bit phone target.
    if ncmds > MAX_CMDS {
        return Err(Fail::TooManyCmds);
    }
    if nverts > MAX_FVERTS {
        return Err(Fail::TooManyVerts);
    }
    if nidx > MAX_FIDX {
        return Err(Fail::TooManyIdx);
    }
    if ntextures > MAX_TEXS {
        return Err(Fail::TooManyTexs);
    }
    let need = FIXTURE_HEADER + ncmds * 56 + nverts * 32 + nidx * 2 + ntextures * 16;
    if bytes.len() != need {
        return Err(Fail::SizeMismatch);
    }

    // One unsafe block for the arena references, same pattern as `lib.rs`'s `st()`: fixed
    // `static mut` buffers, no per-frame allocation, single-threaded. Every index below is
    // provably in range because the counts were capped and the total size matched above.
    unsafe {
        let dc = &mut *core::ptr::addr_of_mut!(arena::D_CMDS);
        let dv = &mut *core::ptr::addr_of_mut!(arena::D_VERTS);
        let di = &mut *core::ptr::addr_of_mut!(arena::D_IDX);
        let dt = &mut *core::ptr::addr_of_mut!(arena::D_TEXS);
        let mut o = FIXTURE_HEADER;
        for i in 0..ncmds {
            let s = &bytes[o..o + 56];
            dc[i] = PortCmd {
                kind: ru32(&s[0..4]),
                flags: ru32(&s[4..8]),
                vstart: ru32(&s[8..12]),
                vcount: ru32(&s[12..16]),
                istart: ru32(&s[16..20]),
                icount: ru32(&s[20..24]),
                f: [
                    rf32(&s[24..28]),
                    rf32(&s[28..32]),
                    rf32(&s[32..36]),
                    rf32(&s[36..40]),
                    rf32(&s[40..44]),
                    rf32(&s[44..48]),
                    rf32(&s[48..52]),
                    rf32(&s[52..56]),
                ],
            };
            o += 56;
        }
        for i in 0..nverts {
            let s = &bytes[o..o + 32];
            dv[i] = PortVertex {
                x: rf32(&s[0..4]),
                y: rf32(&s[4..8]),
                z: rf32(&s[8..12]),
                u: rf32(&s[12..16]),
                v: rf32(&s[16..20]),
                r: rf32(&s[20..24]),
                g: rf32(&s[24..28]),
                b: rf32(&s[28..32]),
            };
            o += 32;
        }
        for i in 0..nidx {
            di[i] = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
            o += 2;
        }
        for i in 0..ntextures {
            let s = &bytes[o..o + 16];
            dt[i] = TexInfo {
                id: ru32(&s[0..4]),
                width: ru32(&s[4..8]),
                height: ru32(&s[8..12]),
                bytes: ru32(&s[12..16]),
            };
            o += 16;
        }
    }

    // Second block: the mutable views above are out of scope, so making shared views of the same
    // memory here is sequenced after every write (single-thread contract, per `decode`'s doc).
    unsafe {
        Ok(Frame {
            frame_id,
            screen_w,
            screen_h,
            cmds: core::slice::from_raw_parts(
                core::ptr::addr_of!(arena::D_CMDS) as *const PortCmd,
                ncmds,
            ),
            verts: core::slice::from_raw_parts(
                core::ptr::addr_of!(arena::D_VERTS) as *const PortVertex,
                nverts,
            ),
            idx: core::slice::from_raw_parts(core::ptr::addr_of!(arena::D_IDX) as *const u16, nidx),
            texs: core::slice::from_raw_parts(
                core::ptr::addr_of!(arena::D_TEXS) as *const TexInfo,
                ntextures,
            ),
        })
    }
}

// ===========================================================================
// validate: structural checks only
// ===========================================================================

/// Outcome of [`validate`]. The `*_oob`/`nan`/... counters are errors (they mean the stream
/// would draw wrong or read out of bounds); `flipped` and `stray_bits` are informational - flip
/// bits on a texture are legal, anything else in the high bits is a writer deviation the reader
/// simply ignores, because no observed payload encodes meaning there.
#[derive(Default)]
pub struct Report {
    pub cmds: u32,
    pub verts: u32,
    pub idx: u32,
    pub texs: u32,
    /// Histogram over `kind & KIND_MASK`, indexed by the `KIND_*` constants.
    pub kinds: [u32; 5],
    pub flipped: u32,
    pub stray_bits: u32,
    pub unknown_kind: u32,
    pub nan: u32,
    pub verts_nan: u32,
    pub vtx_oob: u32,
    pub idx_oob: u32,
    pub local_idx_oob: u32,
    pub mode_bad: u32,
    pub empty: bool,
}

impl Report {
    /// True when every *error* counter is clean. `empty` counts as an error: `dump_fixture` only
    /// fires on frames with `ncmds > 0`, so a zero-command fixture is a failed capture rather
    /// than a valid frame.
    pub fn ok(&self) -> bool {
        self.unknown_kind == 0
            && self.nan == 0
            && self.verts_nan == 0
            && self.vtx_oob == 0
            && self.idx_oob == 0
            && self.local_idx_oob == 0
            && self.mode_bad == 0
            && !self.empty
    }
}

/// Map a geometry `DrawMode` payload (`f[0]`: 0/1/2 as written by `encode()`) to a D3D11
/// topology. `None` for anything else, including NaN - [`validate`] counts that as `mode_bad`
/// and [`build`] refuses the batch, so a future fourth mode cannot silently draw as triangles.
fn mode_topo(mode: f32) -> Option<u32> {
    if mode == 0.0 {
        Some(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST)
    } else if mode == 1.0 {
        Some(D3D11_PRIMITIVE_TOPOLOGY_LINELIST)
    } else if mode == 2.0 {
        Some(D3D11_PRIMITIVE_TOPOLOGY_LINESTRIP)
    } else {
        None
    }
}

/// Run every structural check over a decoded frame.
///
/// Deliberately *not* checked: whether coordinates fall inside the viewport (see the module doc
/// - the only fixture so far predates the pixel-space projection), and whether `vstart` agrees
/// with the flat vertex array's layout. The latter holds by construction on any frame the shim
/// can produce - `encode()` re-emits every geometry batch in arena order, so the input-arena
/// offset and the flat offset advance in lockstep - and [`build`] only ever trusts the bounds.
pub fn validate(f: &Frame) -> Report {
    let mut r = Report {
        cmds: f.cmds.len() as u32,
        verts: f.verts.len() as u32,
        idx: f.idx.len() as u32,
        texs: f.texs.len() as u32,
        ..Report::default()
    };
    r.empty = f.cmds.is_empty();

    for c in f.cmds {
        let kind = c.kind & KIND_MASK;
        if kind as usize >= r.kinds.len() {
            r.unknown_kind += 1;
            continue;
        }
        r.kinds[kind as usize] += 1;

        let high = c.kind >> 16;
        if high != 0 {
            if kind == KIND_TEXTURE && (high & !0b11) == 0 {
                r.flipped += 1;
            } else {
                // Either a flip on something that has no flip, or bits above 17 that `encode()`
                // never writes - both are tolerated, counted, and ignored by the reader.
                r.stray_bits += 1;
            }
        }

        for v in c.f.iter() {
            if !v.is_finite() {
                r.nan += 1;
            }
        }

        if kind != KIND_GEOMETRY {
            continue;
        }

        if mode_topo(c.f[0]).is_none() {
            r.mode_bad += 1;
        }

        let vtx_ok = match c.vstart.checked_add(c.vcount) {
            Some(e) => e as usize <= f.verts.len(),
            None => false,
        };
        if !vtx_ok {
            r.vtx_oob += 1;
        }
        let idx_ok = match c.istart.checked_add(c.icount) {
            Some(e) => e as usize <= f.idx.len(),
            None => false,
        };
        if !idx_ok {
            r.idx_oob += 1;
        }

        if idx_ok && c.icount > 0 {
            // Index base: values are **local to the batch**, i.e. every index must address
            // `vstart..vstart+vcount`. Confirmed against the writer, not assumed: every prpr call
            // site passes 0-based literals into `gl.geometry()` (`prpr/src/ext.rs` lines 301,
            // 328, 349, `prpr/src/ui/text.rs:400`, `prpr/src/core/video.rs:166`), and
            // `push_geometry` copies indices into the arena without rebasing them.
            let base = c.istart as usize;
            for j in 0..c.icount as usize {
                if f.idx[base + j] >= c.vcount as u16 {
                    r.local_idx_oob += 1;
                }
            }
        }
    }

    for v in f.verts {
        if !(v.x.is_finite()
            && v.y.is_finite()
            && v.z.is_finite()
            && v.u.is_finite()
            && v.v.is_finite()
            && v.r.is_finite()
            && v.g.is_finite()
            && v.b.is_finite())
        {
            r.verts_nan += 1;
        }
    }

    r
}

// ===========================================================================
// build: Frame -> shared vertex/index/draw-item arenas
// ===========================================================================

/// One `DrawIndexed` call: a disjoint index range inside the shared index arena plus the
/// topology it must enter the input assembler with. Base vertex is baked into the index values
/// at build time (see [`build`]'s geometry arm), so the draw side needs no `BaseVertexLocation`.
#[derive(Copy, Clone, Debug)]
pub struct DrawItem {
    pub idx_start: u32,
    pub idx_count: u32,
    pub topo: u32,
}

impl DrawItem {
    const ZERO: DrawItem = DrawItem { idx_start: 0, idx_count: 0, topo: 0 };
}

/// What [`build`] produced. `skipped` must be 0 on a valid frame; `empty` counts geometry
/// batches with no indices (they legitimately draw nothing) so `draws + empty + skipped ==
/// cmds` always holds - hostcheck asserts exactly that identity.
#[derive(Default, Debug)]
pub struct Build {
    pub nverts: u32,
    pub nidx: u32,
    pub draws: u32,
    pub empty: u32,
    pub skipped: u32,
}

/// How many segments a `Circle` command is fanned into. 48 keeps chord error under a pixel for
/// the radii prpr draws (judge/progress discs at phone DPI) while costing 49 vertices - a
/// const rather than a heuristic because the count must be identical everywhere it is read.
const CIRCLE_SEG: usize = 48;

// The output arenas. Sizes: `OUT_VERTS` mirrors the pipeline's vertex buffer capacity exactly
// (`MAX_VERTICES`, renderer.rs - copying more would overrun the Map), the index/draw arenas
// mirror the shim's `ICAP`/`PORT_CMD_CAP`.
static mut OUT_VERTS: [Vertex; MAX_VERTICES] = [Vertex {
    x: 0.0,
    y: 0.0,
    z: 0.0,
    u: 0.0,
    v: 0.0,
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
}; MAX_VERTICES];
static mut OUT_IDX: [u16; MAX_FIDX] = [0; MAX_FIDX];
static mut OUT_DRAWS: [DrawItem; MAX_CMDS] = [DrawItem::ZERO; MAX_CMDS];

/// Bounds-checked `u32`-range slice. `checked_add` first, so a wrapped `start + count` on a
/// 32-bit target becomes `None` instead of a backwards range - this is what lets [`build`] be a
/// *safe* function over untrusted `Frame` data.
fn slice32<T>(s: &[T], start: u32, count: u32) -> Option<&[T]> {
    let end = start.checked_add(count)?;
    s.get(start as usize..end as usize)
}

/// `PortVertex` -> the pipeline's 9-float `Vertex`. `a: 1.0` because the record has no alpha
/// (see [`PortVertex`]); `z` passes through unchanged (0 in practice, depth test off).
fn convert(p: &PortVertex) -> Vertex {
    Vertex {
        x: p.x,
        y: p.y,
        z: p.z,
        u: p.u,
        v: p.v,
        r: p.r,
        g: p.g,
        b: p.b,
        a: 1.0,
    }
}

fn vquad(x: f32, y: f32, u: f32, v: f32, col: [f32; 4]) -> Vertex {
    Vertex { x, y, z: 0.0, u, v, r: col[0], g: col[1], b: col[2], a: col[3] }
}

/// Capacity gate shared by every emitter: room for `add_v` vertices, `add_i` indices and one
/// more draw item. `Err` means the caller counts a `skipped` - never a partial write.
fn room(b: &Build, add_v: u32, add_i: u32) -> bool {
    match (b.nverts.checked_add(add_v), b.nidx.checked_add(add_i)) {
        (Some(v), Some(i)) => {
            v as usize <= MAX_VERTICES && i as usize <= MAX_FIDX && (b.draws as usize) < MAX_CMDS
        }
        _ => false,
    }
}

/// Append two triangles covering corners `p0..p3` (in order) as six independent vertices.
///
/// Six rather than four-plus-an-index-trick because every command goes through
/// `DrawIndexed` anyway (one uniform path for generated shapes and engine geometry alike), and
/// with `D3D11_CULL_NONE` (renderer.rs's raster state) the winding is irrelevant.
fn push_corners(
    ov: &mut [Vertex],
    oi: &mut [u16],
    od: &mut [DrawItem],
    b: &mut Build,
    p: [(f32, f32); 4],
    uv: (f32, f32, f32, f32),
    col: [f32; 4],
    topo: u32,
) -> Result<(), ()> {
    if !room(b, 6, 6) {
        return Err(());
    }
    let (ua, va, ub, vb) = uv;
    let v0 = b.nverts as usize;
    let i0 = b.nidx as usize;
    let corners = [
        vquad(p[0].0, p[0].1, ua, va, col),
        vquad(p[1].0, p[1].1, ub, va, col),
        vquad(p[2].0, p[2].1, ub, vb, col),
        vquad(p[3].0, p[3].1, ua, vb, col),
    ];
    ov[v0..v0 + 6].copy_from_slice(&[
        corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
    ]);
    for k in 0..6 {
        oi[i0 + k] = (v0 + k) as u16;
    }
    od[b.draws as usize] = DrawItem { idx_start: b.nidx, idx_count: 6, topo };
    b.nverts += 6;
    b.nidx += 6;
    b.draws += 1;
    Ok(())
}

/// Axis-aligned textured quad - the shape behind the `Texture` and `Rect` kinds.
fn push_quad(
    ov: &mut [Vertex],
    oi: &mut [u16],
    od: &mut [DrawItem],
    b: &mut Build,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv: (f32, f32, f32, f32),
    col: [f32; 4],
    topo: u32,
) -> Result<(), ()> {
    push_corners(ov, oi, od, b, [(x, y), (x + w, y), (x + w, y + h), (x, y + h)], uv, col, topo)
}

/// A `Line` as a quad down the segment: D3D11 has no line-width state (the fixed-function
/// `glLineWidth` does not exist on any target this renderer cares about), so thickness is
/// geometry - offset `thickness / 2` along the segment normal. A zero-length segment collapses
/// to four coincident vertices, which rasterises to nothing; no special case needed.
fn push_line(
    ov: &mut [Vertex],
    oi: &mut [u16],
    od: &mut [DrawItem],
    b: &mut Build,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    thickness: f32,
    rgb: [f32; 3],
) -> Result<(), ()> {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len = (dx * dx + dy * dy).sqrt();
    let half = thickness * 0.5;
    let (nx, ny) = if len > 0.0 {
        (-dy / len * half, dx / len * half)
    } else {
        (0.0, 0.0)
    };
    // uv is arbitrary: lines carry no texture (they sample the white stand-in), see module doc.
    push_corners(
        ov,
        oi,
        od,
        b,
        [
            (x1 + nx, y1 + ny),
            (x2 + nx, y2 + ny),
            (x2 - nx, y2 - ny),
            (x1 - nx, y1 - ny),
        ],
        (0.0, 0.0, 1.0, 1.0),
        [rgb[0], rgb[1], rgb[2], 1.0],
        D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
    )
}

/// A `Circle` as a triangle fan. `encode()` records exactly what macroquad's `draw_circle`
/// means - a *filled* disc (`shims/macroquad/src/lib.rs`, `pub fn draw_circle`) - with rgba in
/// `f[3..7]`. Colour alpha survives here (unlike vertex alpha), so fading discs blend.
fn push_fan(
    ov: &mut [Vertex],
    oi: &mut [u16],
    od: &mut [DrawItem],
    b: &mut Build,
    cx: f32,
    cy: f32,
    r: f32,
    col: [f32; 4],
) -> Result<(), ()> {
    let vcount = (CIRCLE_SEG + 1) as u32;
    let icount = (CIRCLE_SEG * 3) as u32;
    if !room(b, vcount, icount) {
        return Err(());
    }
    let v0 = b.nverts as usize;
    let i0 = b.nidx as usize;
    ov[v0] = vquad(cx, cy, 0.0, 0.0, col);
    for k in 0..=CIRCLE_SEG {
        let ang = k as f32 / CIRCLE_SEG as f32 * core::f32::consts::TAU;
        ov[v0 + 1 + k] = vquad(cx + r * ang.cos(), cy + r * ang.sin(), 0.0, 0.0, col);
    }
    for k in 0..CIRCLE_SEG {
        oi[i0 + 3 * k] = v0 as u16;
        oi[i0 + 3 * k + 1] = (v0 + 1 + k) as u16;
        oi[i0 + 3 * k + 2] = (v0 + 2 + k) as u16;
    }
    od[b.draws as usize] = DrawItem { idx_start: b.nidx, idx_count: icount, topo: D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST };
    b.nverts += vcount;
    b.nidx += icount;
    b.draws += 1;
    Ok(())
}

/// Texture dimensions for UV normalisation, from the fixture's texture table. `(0, 0)` when the
/// id is unknown - see the fallback at the `KIND_TEXTURE` arm.
fn tex_dims(f: &Frame, id: u32) -> (f32, f32) {
    for t in f.texs {
        if t.id == id && t.width > 0 && t.height > 0 {
            return (t.width as f32, t.height as f32);
        }
    }
    (0.0, 0.0)
}

/// Flatten every command of `f` into the shared output arenas.
///
/// # Why everything is rebased instead of drawn in place
///
/// Engine geometry carries `(vstart, vcount, istart, icount)` into the *recording* arenas, and
/// its indices are local to the batch (see [`validate`]). The generated shapes (Texture / Rect /
/// Line / Circle) need vertices too, so all four kinds live in one arena with one index stream
/// and one `DrawIndexed` per command: each batch's vertices are **copied** to the current output
/// offset and its local indices rewritten as `out_base + local`. That way the draw side never
/// depends on how the source arenas were laid out - a `vstart` that disagreed with the flat
/// array (the one theoretical quirk of `encode()`, which copies to an output offset but records
/// the input offset; they agree on every frame the shim can actually produce) could not misplace
/// a single vertex here.
///
/// Safe by construction: capacity is checked before any write, source ranges go through
/// [`slice32`], and unknown or malformed commands are *counted* (`skipped`), never drawn.
pub fn build(f: &Frame) -> Build {
    let mut b = Build::default();
    // The one unsafe block: take references to the three `static mut` output arenas (same
    // pattern as `lib.rs::st()` and the shim's own `push_geometry`). Single-threaded by
    // contract; every write below is bounds-checked first, so the references themselves are the
    // whole of what unsafe is needed for.
    unsafe {
        let ov = &mut *core::ptr::addr_of_mut!(OUT_VERTS);
        let oi = &mut *core::ptr::addr_of_mut!(OUT_IDX);
        let od = &mut *core::ptr::addr_of_mut!(OUT_DRAWS);

        for c in f.cmds {
            match c.kind & KIND_MASK {
                // Texture: dest rect in f[0..4], source rect in f[4..8] (pixels of the source
                // texture), texture id in `flags`; flips ride in the kind's high bits - all as
                // written by `encode()`.
                KIND_TEXTURE => {
                    let (tw, th) = tex_dims(f, c.flags);
                    // Normalise the source rect to 0..1. Unknown texture - the fixture records
                    // no pixels by design - falls back to whole-surface uv; either way the quad
                    // samples the white stand-in, so this is about keeping the UV path honest
                    // for the phone, where real SRVs will exist.
                    let (mut u0, mut v0, mut u1, mut v1) = if tw > 0.0 && th > 0.0 {
                        (c.f[4] / tw, c.f[5] / th, (c.f[4] + c.f[6]) / tw, (c.f[5] + c.f[7]) / th)
                    } else {
                        (0.0, 0.0, 1.0, 1.0)
                    };
                    if c.kind & FLAG_FLIP_X != 0 {
                        core::mem::swap(&mut u0, &mut u1);
                    }
                    if c.kind & FLAG_FLIP_Y != 0 {
                        core::mem::swap(&mut v0, &mut v1);
                    }
                    // Colour is white because `encode()` drops the texture command's colour ("the
                    // Texture path is always full-white in practice") - contract value, not a
                    // placeholder.
                    if push_quad(
                        ov, oi, od, &mut b,
                        c.f[0], c.f[1], c.f[2], c.f[3],
                        (u0, v0, u1, v1),
                        [1.0, 1.0, 1.0, 1.0],
                        D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                    )
                    .is_err()
                    {
                        b.skipped += 1;
                    }
                }
                // Rect: dest in f[0..4], rgba in f[4..8].
                KIND_RECT => {
                    if push_quad(
                        ov, oi, od, &mut b,
                        c.f[0], c.f[1], c.f[2], c.f[3],
                        (0.0, 0.0, 1.0, 1.0),
                        [c.f[4], c.f[5], c.f[6], c.f[7]],
                        D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                    )
                    .is_err()
                    {
                        b.skipped += 1;
                    }
                }
                // Line: x1/y1/x2/y2 in f[0..4], thickness in f[4], rgb in f[5..8]. No alpha
                // slot exists for this kind (`encode()` writes three floats), so lines are
                // opaque by contract - not an oversight here.
                KIND_LINE => {
                    if push_line(
                        ov, oi, od, &mut b,
                        c.f[0], c.f[1], c.f[2], c.f[3], c.f[4],
                        [c.f[5], c.f[6], c.f[7]],
                    )
                    .is_err()
                    {
                        b.skipped += 1;
                    }
                }
                // Circle: centre/radius in f[0..3], rgba in f[3..7].
                KIND_CIRCLE => {
                    if push_fan(
                        ov, oi, od, &mut b,
                        c.f[0], c.f[1], c.f[2],
                        [c.f[3], c.f[4], c.f[5], c.f[6]],
                    )
                    .is_err()
                    {
                        b.skipped += 1;
                    }
                }
                // Geometry: texture id in `flags`, DrawMode in f[0], vertices/indices through
                // the side arrays. (Note the shim's *doc comment* on `flags` claims the mode
                // lives there too; `encode()` writes `flags = tex` and `f[0] = mode` - the code
                // is the contract, and it is what the fixture contains.)
                KIND_GEOMETRY => {
                    let Some(topo) = mode_topo(c.f[0]) else {
                        b.skipped += 1;
                        continue;
                    };
                    let Some(sv) = slice32(f.verts, c.vstart, c.vcount) else {
                        b.skipped += 1;
                        continue;
                    };
                    let Some(si) = slice32(f.idx, c.istart, c.icount) else {
                        b.skipped += 1;
                        continue;
                    };
                    if si.is_empty() {
                        b.empty += 1;
                        continue;
                    }
                    // Validate the local indices *before* writing anything, so a bad batch
                    // cannot leave half a copy behind.
                    if si.iter().any(|&ix| ix as u32 >= c.vcount) {
                        b.skipped += 1;
                        continue;
                    }
                    if !room(&b, sv.len() as u32, si.len() as u32) {
                        b.skipped += 1;
                        continue;
                    }
                    let v0 = b.nverts as usize;
                    let i0 = b.nidx as usize;
                    for (j, p) in sv.iter().enumerate() {
                        ov[v0 + j] = convert(p);
                    }
                    for (j, &ix) in si.iter().enumerate() {
                        oi[i0 + j] = (v0 as u16) + ix;
                    }
                    od[b.draws as usize] = DrawItem {
                        idx_start: b.nidx,
                        idx_count: si.len() as u32,
                        topo,
                    };
                    b.nverts += sv.len() as u32;
                    b.nidx += si.len() as u32;
                    b.draws += 1;
                }
                _ => {
                    // Unknown tag: `validate` flags it as an error; build refuses to guess.
                    b.skipped += 1;
                }
            }
        }
    }
    b
}

// ===========================================================================
// the one matrix: pixels -> clip space
// ===========================================================================

/// Map pixel coordinates (origin top-left, y down, `screen_w` x `screen_h`) to D3D11 clip space.
///
/// Returned in the same layout as [`crate::renderer::projection`], whose doc pins the convention:
/// read the array as a row-major matrix `A` and the shader computes `clip = A * (x, y, z, 1)`.
/// So row 0 scales x to -1..1, row 1 flips and scales y (pixel y grows downward, clip y grows
/// upward), row 2 passes z through (always 0 here; depth test is off), row 3 is the homogeneous
/// 1.
///
/// This is **not** a camera or model transform - prpr's camera was applied on the recording
/// side (module doc), and re-applying anything of the sort here would double-transform the
/// frame. The matrix depends only on the viewport the frame was recorded at, which is exactly
/// why [`draw`] sets the viewport from the same numbers.
pub fn screen_projection(screen_w: f32, screen_h: f32) -> [f32; 16] {
    [
        2.0 / screen_w, 0.0, 0.0, -1.0,
        0.0, -2.0 / screen_h, 0.0, 1.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ]
}

// ===========================================================================
// draw: the FFI half
// ===========================================================================

/// The shared u16 index buffer, created on first draw and cached for the life of the process -
/// one `ComPtr` static next to the arenas it indexes, rather than a field on `Pipeline` that
/// Stage 1's proven path would have to grow and tear down for a test it never uses.
static mut IB: ComPtr = ComPtr::NULL;

/// Last HRESULT observed inside [`draw`] (Map / CreateBuffer both return one; `DrawIndexed`
/// itself returns void). 0 when the last call succeeded - printed by hostcheck on failure so
/// the D3D11 error is part of the message instead of a bare step code.
pub static mut LAST_HR: i32 = 0;

/// Release the cached index buffer. Idempotent, and the next [`draw`] recreates it - called from
/// hostcheck's teardown so the process exits with nothing held; the phone's `release_all` can
/// call it too when Stage 3 gets wired to the shell.
pub unsafe fn release() {
    IB.release();
    IB = ComPtr::NULL;
}

/// Replay a built frame: upload the arenas, bind the full pipeline state, one `DrawIndexed` per
/// draw item.
///
/// # Safety
///
/// * Every call in the body is a raw D3D11 vtable invocation - that is the irreducible part.
/// * `b` must describe the data currently in the `OUT_*` arenas (i.e. the `Build` returned by
///   [`build`] for the frame being replayed, with no intervening `build`). Sizes read out of
///   `b` are clamped to the arena caps regardless, so even a wrong `Build` cannot walk the
///   copies out of bounds - but it would draw the wrong frame.
/// * `rtv` and `white` must be live objects of `pipeline`'s device, and `screen_w`/`screen_h`
///   must be finite and positive ( [`decode` ] guarantees this for fixture data).
/// * Single-threaded, like everything else in this crate.
///
/// Returns 0 on success, otherwise a negative step code with [`LAST_HR`] holding the HRESULT
/// of the failing call where the API has one.
///
/// The bind set mirrors hostcheck step 7 - the sequence already proven to rasterize on both
/// desktop and Lumia - with one addition (`IASetIndexBuffer`, which Stage 1's non-indexed draw
/// never needed).
pub unsafe fn draw(
    pipeline: &Pipeline,
    rtv: ComPtr,
    white: ComPtr,
    screen_w: f32,
    screen_h: f32,
    b: &Build,
) -> i32 {
    LAST_HR = 0;
    let context = &pipeline.context;

    // ---- index buffer, once ----
    if IB.is_null() {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: (MAX_FIDX * core::mem::size_of::<u16>()) as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_INDEX_BUFFER,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let create: extern "system" fn(
            *mut c_void,
            *const D3D11_BUFFER_DESC,
            *const D3D11_SUBRESOURCE_DATA,
            *mut *mut c_void,
        ) -> HRESULT = pipeline.device.method(D3D11DEVICE_CREATEBUFFER);
        let hr = create(
            pipeline.device.vtbl as *mut c_void,
            &desc,
            core::ptr::null(),
            &mut IB.vtbl as *mut _ as *mut *mut c_void,
        );
        LAST_HR = hr;
        if hr < 0 || IB.is_null() {
            return -1;
        }
    }

    let map: extern "system" fn(
        *mut c_void,
        *mut c_void,
        UINT,
        D3D11_MAP,
        UINT,
        *mut D3D11_MAPPED_SUBRESOURCE,
    ) -> HRESULT = context.method(D3D11CONTEXT_MAP);
    let unmap: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        context.method(D3D11CONTEXT_UNMAP);

    // ---- upload the vertex arena ----
    // Clamp to the cap, then copy: a `Build` larger than the arenas is nonsense, but clamping
    // makes it *bounded* nonsense instead of an out-of-bounds read. WRITE_DISCARD per frame is
    // the dynamic-buffer pattern Stage 1 already uses (and the shim's philosophy: fixed
    // capacity, no per-frame allocation).
    if b.nverts > 0 {
        let n = (b.nverts as usize).min(MAX_VERTICES);
        let mut m = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        let hr = map(
            context.vtbl as *mut c_void,
            pipeline.vb.vtbl as *mut c_void,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            &mut m,
        );
        LAST_HR = hr;
        if hr < 0 {
            return -2;
        }
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(OUT_VERTS) as *const u8,
            m.pData as *mut u8,
            n * core::mem::size_of::<Vertex>(),
        );
        unmap(context.vtbl as *mut c_void, pipeline.vb.vtbl as *mut c_void, 0);
    }

    // ---- upload the index arena ----
    if b.nidx > 0 {
        let n = (b.nidx as usize).min(MAX_FIDX);
        let mut m = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        let hr = map(
            context.vtbl as *mut c_void,
            IB.vtbl as *mut c_void,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            &mut m,
        );
        LAST_HR = hr;
        if hr < 0 {
            return -3;
        }
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(OUT_IDX) as *const u8,
            m.pData as *mut u8,
            n * core::mem::size_of::<u16>(),
        );
        unmap(context.vtbl as *mut c_void, IB.vtbl as *mut c_void, 0);
    }

    // ---- constant buffer: the viewport mapping (see `screen_projection`) ----
    {
        let mut m = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        let hr = map(
            context.vtbl as *mut c_void,
            pipeline.cb.vtbl as *mut c_void,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            &mut m,
        );
        LAST_HR = hr;
        if hr < 0 {
            return -4;
        }
        let proj = screen_projection(screen_w, screen_h);
        core::ptr::copy_nonoverlapping(proj.as_ptr() as *const u8, m.pData as *mut u8, 64);
        unmap(context.vtbl as *mut c_void, pipeline.cb.vtbl as *mut c_void, 0);
    }

    // ---- bind the full state (hostcheck step 7's proven order) ----
    // Viewport from the frame's own recorded screen size: the coordinates *are* pixels of that
    // screen, so the projection above and the viewport must agree even when the caller's render
    // target was set up for something else (they match exactly in hostcheck - 768x1229 both).
    let vp = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: screen_w,
        Height: screen_h,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let set_viewports: extern "system" fn(*mut c_void, UINT, *const D3D11_VIEWPORT) =
        context.method(D3D11CONTEXT_RSSETVIEWPORTS);
    set_viewports(context.vtbl as *mut c_void, 1, &vp);

    let set_rt: extern "system" fn(*mut c_void, UINT, *const *mut c_void, *mut c_void) =
        context.method(D3D11CONTEXT_OMSETRENDERTARGETS);
    let rt = rtv.vtbl as *mut c_void;
    set_rt(context.vtbl as *mut c_void, 1, &rt, core::ptr::null_mut());

    let set_layout: extern "system" fn(*mut c_void, *mut c_void) =
        context.method(D3D11CONTEXT_IASETINPUTLAYOUT);
    set_layout(context.vtbl as *mut c_void, pipeline.input_layout.vtbl as *mut c_void);

    let set_vb: extern "system" fn(
        *mut c_void,
        UINT,
        UINT,
        *const *mut c_void,
        *const UINT,
        *const UINT,
    ) = context.method(D3D11CONTEXT_IASETVERTEXBUFFERS);
    let vbv = pipeline.vb.vtbl as *mut c_void;
    let stride = VERTEX_STRIDE;
    let offset = 0u32;
    set_vb(context.vtbl as *mut c_void, 0, 1, &vbv, &stride, &offset);

    // The one call Stage 1 never made: u16 indices, format from dxgiformat.h (direct.rs).
    let set_ib: extern "system" fn(*mut c_void, *mut c_void, UINT, UINT) =
        context.method(D3D11CONTEXT_IASETINDEXBUFFER);
    set_ib(
        context.vtbl as *mut c_void,
        IB.vtbl as *mut c_void,
        DXGI_FORMAT_R16_UINT,
        0,
    );

    let set_vs: extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, UINT) =
        context.method(D3D11CONTEXT_VSSETSHADER);
    set_vs(
        context.vtbl as *mut c_void,
        pipeline.vs.vtbl as *mut c_void,
        core::ptr::null(),
        0,
    );
    let set_ps: extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, UINT) =
        context.method(D3D11CONTEXT_PSSETSHADER);
    set_ps(
        context.vtbl as *mut c_void,
        pipeline.ps.vtbl as *mut c_void,
        core::ptr::null(),
        0,
    );
    // Buffer-array parameters are `*const *mut c_void` and are passed as `&obj`: these three
    // APIs take an *array of object pointers* (`ID3D11Buffer**`, `ID3D11ShaderResourceView**`,
    // `ID3D11SamplerState**`), not one object. Declaring the slot as `*mut c_void` and passing
    // the object itself compiles just as happily and then hands D3D11 a vtable address where it
    // expects an array - it reads the first vtable slot as a COM pointer and faults on it. The
    // working declarations are hostcheck step 7's (src/bin/hostcheck.rs set_vscb/set_srv/set_samp)
    // and lib.rs's own bind path; both spell it `*const *mut c_void`.
    let set_vscb: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
        context.method(D3D11CONTEXT_VSSETCONSTANTBUFFERS);
    let cbv = pipeline.cb.vtbl as *mut c_void;
    set_vscb(context.vtbl as *mut c_void, 0, 1, &cbv);
    // One SRV for the whole frame: every texture is the white stand-in (module doc).
    let set_srv: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
        context.method(D3D11CONTEXT_PSSETSHADERRESOURCES);
    let srvs = white.vtbl as *mut c_void;
    set_srv(context.vtbl as *mut c_void, 0, 1, &srvs);
    let set_samp: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
        context.method(D3D11CONTEXT_PSSETSAMPLERS);
    let smp = pipeline.sampler.vtbl as *mut c_void;
    set_samp(context.vtbl as *mut c_void, 0, 1, &smp);
    let set_blend: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT, UINT) =
        context.method(D3D11CONTEXT_OMSETBLENDSTATE);
    set_blend(
        context.vtbl as *mut c_void,
        pipeline.blend.vtbl as *mut c_void,
        core::ptr::null(),
        0xffff_ffff,
    );
    let set_raster: extern "system" fn(*mut c_void, *mut c_void) =
        context.method(D3D11CONTEXT_RSSETSTATE);
    set_raster(context.vtbl as *mut c_void, pipeline.raster.vtbl as *mut c_void);
    let set_depth: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        context.method(D3D11CONTEXT_OMSETDEPTHSTENCILSTATE);
    set_depth(
        context.vtbl as *mut c_void,
        pipeline.depth.vtbl as *mut c_void,
        0,
    );

    // ---- one indexed draw per command ----
    // DrawIndexed(IndexCount, StartIndexLocation, BaseVertexLocation): base stays 0 because
    // `build` baked it into the index values, so a draw item is fully described by its index
    // range plus topology.
    let draw_indexed: extern "system" fn(*mut c_void, UINT, UINT, i32) =
        context.method(D3D11CONTEXT_DRAWINDEXED);
    let set_topo: extern "system" fn(*mut c_void, UINT) =
        context.method(D3D11CONTEXT_IASETPRIMITIVETOPOLOGY);
    // Clamp the item count too, for the same reason as the copies: a lying `Build` draws at
    // most what the arena holds.
    let nitems = (b.draws as usize).min(MAX_CMDS);
    let items = core::slice::from_raw_parts(core::ptr::addr_of!(OUT_DRAWS) as *const DrawItem, nitems);
    for it in items {
        set_topo(context.vtbl as *mut c_void, it.topo);
        draw_indexed(context.vtbl as *mut c_void, it.idx_count, it.idx_start, 0);
    }

    0
}


