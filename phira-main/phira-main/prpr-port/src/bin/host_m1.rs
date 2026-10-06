//! M1 host smoke test: run the real `prpr` game headlessly on the PC and report exactly what it did.
//!
//! # Why this exists
//!
//! `wp81_game_create` has never once been called anywhere - the port built the DLL, shipped it, and
//! stopped one step short of ever asking it for a frame. Everything between "chart on disk" and
//! "draw commands out" is therefore unverified: the RPE parse, `GameScene::new`, the font load, the
//! texture registration, `pollster` blocking on async that must resolve immediately, and the audio
//! decode.
//!
//! On the phone each of those failures surfaces as one of five integer codes. Here they surface as
//! an `anyhow` chain, a panic message, or - the point of this program - **the name of the file
//! that was requested and not found**. Guessing that from a device log costs one deploy per guess.
//!
//! This is also not throwaway: whatever command stream it captures is real `prpr` output, which is
//! what the D3D11 renderer's Stage 3 replay has to consume. Feeding those bytes to the desktop
//! `hostcheck` harness makes the replay testable before any of it goes near the handset.
//!
//! # Where the `unsafe` is, and why so little of it
//!
//! Every callback in `HostCallbacks` is typed `unsafe extern "C" fn`, but a *safe* function coerces
//! into that type, so only the two callbacks that actually read or write memory are written
//! `unsafe` - `load_asset` and `audio_decode`. The rest are plain `extern "C" fn`, and that is a
//! true statement about them: they take no pointer they ever dereference, so there is no
//! precondition a caller could violate.
//!
//! Inside the two that are unsafe, the raw arguments are converted to safe references in a single
//! documented block at the top, and everything after it - the WAV walk, the Ogg page walk, the MP3
//! frame walk - is ordinary safe code over `&[u8]`. `wide_string` is the only other `unsafe fn`,
//! because "NUL-terminated and mapped for its whole length" is a property of the caller.
//!
//! # Usage
//!
//! ```text
//! cargo run -p prpr-port --bin host_m1 -- <charts-dir> [folder] [chart-file] [frames]
//! ```
//!
//! `<charts-dir>` is the directory *containing* the chart folder - the equivalent of the app's
//! `ApplicationData\LocalFolder\charts`, which is what `HostFileSystem`'s prefix is relative to.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use macroquad::draw::{PortCmd, PortVertex};
use prpr_port::audio_info::audio_info;
use prpr_port::{DecodedAudio, HostCallbacks};

// ===========================================================================
// the shell's filesystem, standing in for `PrprPort.OnLoadAsset`
// ===========================================================================

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Every file the engine asks for, in request order. Printed at the end: this list *is* the
/// answer M1 is looking for, since the C# side has to have each of these on disk.
static REQUESTS: std::sync::Mutex<Vec<(String, i32)>> = std::sync::Mutex::new(Vec::new());

/// Read a NUL-terminated UTF-16 string out of a raw pointer.
///
/// The bound of `1 << 20` units is what stops a pointer that is *not* NUL-terminated from walking
/// into unmapped memory forever; it is a ceiling on a bug, not a bound on anything legitimate -
/// no asset path the engine sends comes anywhere near it.
///
/// # Safety
///
/// `p` must either be null (handled) or point at `u16` cells that are readable up to and including
/// the first `0` within `1 << 20` cells.
unsafe fn wide_string(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut v = Vec::new();
    let mut i = 0usize;
    unsafe {
        while *p.add(i) != 0 {
            v.push(*p.add(i));
            i += 1;
            if i >= 1 << 20 {
                break;
            }
        }
    }
    String::from_utf16(&v).ok()
}

/// The only callback that writes to a buffer the caller owns.
///
/// Returns the byte count on success, `-1` when the shell has no such file and `-2` when the file
/// exists but does not fit the buffer the engine offered - which is how the size query and the real
/// read are distinguished on the C# side too.
unsafe extern "C" fn load_asset(name: *const u16, out: *mut u8, cap: usize) -> i32 {
    // One unsafe block: turn the two raw arguments into a `String` the rest of the function can
    // work with without ever thinking about them again. The write below is a second, and the last.
    let Some(name) = (unsafe { wide_string(name) }) else {
        return -1;
    };
    // The engine sends `<folder>/<file>`; the root we were given plays the part of `charts\`.
    let Some(root) = ROOT.get() else {
        return -1;
    };
    let full = root.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));

    let bytes = match fs::read(&full) {
        Ok(b) => b,
        Err(_) => {
            if let Ok(mut rq) = REQUESTS.lock() {
                rq.push((name.clone(), -1));
            }
            eprintln!("[m1]   load_asset  MISS  {name}");
            return -1;
        }
    };
    if out.is_null() {
        // No buffer to write into. Returning `-1` rather than `-2` is deliberate: `-2` is the
        // "grow and try again" signal for the caller in `host::load_asset`, and answering it to a
        // null buffer would spin that loop until the ceiling.
        return -1;
    }
    if bytes.len() > cap {
        if let Ok(mut rq) = REQUESTS.lock() {
            rq.push((name.clone(), -2));
        }
        eprintln!("[m1]   load_asset  TOO BIG ({} > {})  {name}", bytes.len(), cap);
        return -2;
    }
    if let Ok(mut rq) = REQUESTS.lock() {
        rq.push((name.clone(), bytes.len() as i32));
    }
    eprintln!("[m1]   load_asset  {} bytes  {name}", bytes.len());
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    }
    bytes.len() as i32
}

// ===========================================================================
// the shell's audio, standing in for the sasa shim's forwarders
//
// Position is the one that matters for M1: prpr keeps the judge in step with the music through
// `Music::position()`, so a stub that always answers 0.0 would leave the chart frozen at its first
// frame and look exactly like "the notes never spawn". The bin advances it by the same `dt` it
// hands `wp81_game_frame`, which is what real playback would do.
//
// None of these dereference anything, which is why none of them are `unsafe`: they cannot be made
// to do anything the caller could not already do with the integer they pass in.
// ===========================================================================

static MUSIC_POS_BITS: AtomicU64 = AtomicU64::new(0);
static MUSIC_PLAYING: AtomicBool = AtomicBool::new(false);

fn music_pos() -> f64 {
    f64::from_bits(MUSIC_POS_BITS.load(Ordering::Relaxed))
}

fn set_music_pos(v: f64) {
    MUSIC_POS_BITS.store(v.to_bits(), Ordering::Relaxed);
}

/// The one audio callback that touches memory: read the caller's buffer, write the caller's struct.
///
/// Both conversions happen in a single block at the top, after the null and length checks that make
/// them sound; everything below is safe code over the references that produces a plain `i32`.
unsafe extern "C" fn audio_decode(data: *const u8, len: usize, out: *mut DecodedAudio) -> i32 {
    if data.is_null() || out.is_null() || len < 16 {
        return 1;
    }
    let (bytes, out) = unsafe {
        (
            core::slice::from_raw_parts(data, len),
            &mut *out,
        )
    };
    let Some((rate, frames)) = audio_info(bytes) else {
        return 1;
    };
    out.sample_rate = rate;
    out.frame_count = frames;
    eprintln!(
        "[m1]   audio_decode  {rate} Hz, {frames} frames ({:.2} s)",
        frames as f64 / rate as f64
    );
    0
}

extern "C" fn music_new(
    _clip: u32,
    _name: *const u8,
    _name_len: usize,
    _rate: f64,
    _amp: f32,
    _loop_mix: f64,
    _clip_secs: f64,
) -> u32 {
    1
}
extern "C" fn music_play(_id: u32) {
    MUSIC_PLAYING.store(true, Ordering::Relaxed);
}
extern "C" fn music_pause(_id: u32) {
    MUSIC_PLAYING.store(false, Ordering::Relaxed);
}
extern "C" fn music_seek(_id: u32, position: f64) {
    set_music_pos(position);
}
extern "C" fn music_position(_id: u32) -> f64 {
    music_pos()
}
extern "C" fn music_paused(_id: u32) -> i32 {
    if MUSIC_PLAYING.load(Ordering::Relaxed) {
        0
    } else {
        1
    }
}
extern "C" fn music_amp(_id: u32, _amp: f32) {}
extern "C" fn music_lowpass(_id: u32, _cutoff: f32) {}
extern "C" fn music_fade(_id: u32, _seconds: f64, _out: i32) {}
extern "C" fn sfx_new(_clip: u32, _name: *const u8, _name_len: usize, _secs: f64) -> u32 {
    0
}
extern "C" fn sfx_play(_id: u32, _amp: f32) {}
extern "C" fn pick_file(_title: *const u16, _out: *mut u16, _cap: usize) -> i32 {
    0
}
extern "C" fn save_file(
    _title: *const u16,
    _suggested: *const u16,
    _out: *mut u16,
    _cap: usize,
) -> i32 {
    0
}
extern "C" fn present() {}

// ===========================================================================
// the run
// ===========================================================================

/// Break down a drained frame: how many records of each kind, and the vertex/index watermark.
///
/// The watermark is the number the Stage 3 replay actually sizes its dynamic buffer from - it is
/// the largest `vstart + vcount` any `Geometry` record reaches, i.e. how much of the 65536-vertex
/// arena this frame touched.
fn drain_stats(
    ncmds: usize,
    cmds: *const PortCmd,
    _verts: *const PortVertex,
    _idx: *const u16,
) -> (u32, u32, [u32; 5]) {
    let (mut vhi, mut ihi) = (0u32, 0u32);
    // Kinds 0..4 are TEXTURE, RECT, LINE, CIRCLE, GEOMETRY (`shims/macroquad/src/lib.rs`).
    let mut kinds = [0u32; 5];
    if cmds.is_null() || ncmds == 0 {
        return (vhi, ihi, kinds);
    }
    // One conversion for the whole frame instead of one per record: the engine handed over an
    // array of exactly `ncmds` live entries, which is the precondition `from_raw_parts` needs and
    // the whole of what this function assumes.
    let cmds = unsafe { core::slice::from_raw_parts(cmds, ncmds) };
    for c in cmds {
        if (c.kind as usize) < kinds.len() {
            kinds[c.kind as usize] += 1;
        }
        if c.kind == macroquad::draw::KIND_GEOMETRY {
            vhi = vhi.max(c.vstart.saturating_add(c.vcount));
            ihi = ihi.max(c.istart.saturating_add(c.icount));
        }
    }
    (vhi, ihi, kinds)
}

/// Enumerate the registered textures: id, dimensions and byte length.
///
/// The ids are allocated where the shim sees pixels, not filenames, so the only way to learn what
/// the engine actually asked for is to ask the registry. `out == null` is the size query.
fn dump_textures() {
    let n = prpr_port::wp81_texture_count();
    let dropped = prpr_port::wp81_texture_overflow();
    eprintln!("[m1] {n} textures registered, {dropped} dropped for room");
    // Always printed, not only on a drop: how much headroom the budget has left is the question
    // every future change to the registry gets judged against, and by the time something is
    // dropped the number that mattered has already been spent.
    eprintln!(
        "       registry holds {} bytes of {}",
        macroquad::draw::texture_used_bytes(),
        macroquad::draw::texture_capacity()
    );
    let (demand, aliased) = macroquad::draw::texture_demand();
    eprintln!(
        "       unique demand = {demand} bytes; {aliased} registrations aliased instead of copied"
    );
    if dropped > 0 {
        // A count on its own cannot say whether the budget, the slot count or the chart is at
        // fault - the refused texture's size is what separates those.
        let (id, w, h, bytes) = macroquad::draw::texture_drop_last();
        eprintln!(
            "       last drop: id={id} {w}x{h} {bytes} bytes, reason={}",
            macroquad::draw::texture_drop_reason()
        );
    }
    // Sub-image writes are the only writes that happen *after* registration, and on this port they
    // belong to the glyph atlas alone. Both numbers are needed: a refusal count of zero is also
    // what an engine that never wrote anything would report, so the accepted count is the liveness
    // signal and the refusal count says whether what did run was refused.
    let (writes, last_write) = macroquad::draw::texture_write_stats();
    eprintln!(
        "       sub-image writes = {writes} (last id={last_write}), refused = {}",
        prpr_port::wp81_texture_write_misses()
    );

    for i in 0..n {
        let id = prpr_port::wp81_texture_id_at(i);
        let (mut w, mut h) = (0u32, 0u32);
        let bytes = unsafe {
            prpr_port::wp81_texture_data(id, &mut w, &mut h, core::ptr::null_mut(), 0)
        };
        eprintln!("       id={id} {w}x{h} {bytes} bytes");
    }

    // The texture that last received a sub-image write is the glyph atlas - nothing else in this
    // port is filled after it is registered - and counting its non-zero bytes is the measurement
    // that separates "an entry exists" from "glyphs were rasterised into it". An empty atlas
    // registers perfectly well and renders nothing, so registration alone proves nothing.
    if last_write != 0 {
        let (mut w, mut h) = (0u32, 0u32);
        let len = unsafe {
            prpr_port::wp81_texture_data(last_write, &mut w, &mut h, core::ptr::null_mut(), 0)
        };
        if len > 0 {
            let mut buf = vec![0u8; len as usize];
            let got = unsafe {
                prpr_port::wp81_texture_data(last_write, &mut w, &mut h, buf.as_mut_ptr(), buf.len())
            };
            if got == len {
                let filled = buf.iter().filter(|b| **b != 0).count();
                eprintln!(
                    "       last-written: id={last_write} {w}x{h}, {filled} of {} bytes non-zero",
                    buf.len()
                );
            }
        }
    }
}

/// Serialise one drained frame, so the Stage 3 replay has real numbers to test against.
///
/// `scene.rs` sizes its vertex and index buffers from whatever the engine happened to emit, and a
/// wrong number does not surface as an error: it surfaces as clipped notes, or as a buffer range
/// the GPU is handed and never reports on. Nothing in the decode path can catch it. A fixture from
/// the real engine pins those numbers down off-device, which matters because a device round needs
/// the phone unlocked and in hand, while this needs a dozen iterations to get right.
///
/// Texture *pixels* are deliberately left out. The registry is measured on its own (75 registered,
/// 0 dropped on the desktop), 24 MiB of RGBA would make the file as big as the chart it exists to
/// test, and the reader painting an unknown id flat white is enough to check every geometric claim
/// and enough to see a chart.
///
/// Everything is little-endian. Layout:
///
/// ```text
///   0x00 u32 magic   0x04 u32 version   0x08 u32 frame
///   0x0C f32 screen_w        0x10 f32 screen_h
///   0x14 u32 ncmds     0x18 u32 nverts     0x1C u32 nidx     0x20 u32 ntextures
///   then ncmds * 56 (PortCmd), nverts * 32 (PortVertex), nidx * 2 (u16),
///   then ntextures * 16 ({ id, w, h, bytes } as u32 each)
/// ```
///
/// `vhi`/`ihi` are the watermarks `drain_stats` measured - the *live* extent of the arenas, which
/// can be far smaller than their capacity and is what the replay must size itself from.
/// Exercises the touch queue on the desktop, where no finger will ever arrive.
///
/// The phone is the only place a real touch can be produced, which makes this the one part of the
/// port that cannot be verified without a device round - and a device round is where a plumbing bug
/// is most expensive to find, because "the chart does not respond to my finger" has about six
/// candidate causes. So the three halves of the path are checked here instead:
///
///   1. `push_touch` -> `touches()`: the *state* `Judge::update` reads.
///   2. `push_touch` -> `repeat_all_miniquad_input`: the *event stream* that fills `prpr::judge`'s
///      `TOUCHES` store, and with it everything `get_touches()` reaches.
///   3. `clear_touches`: the frame boundary. A queue that is not cleared leaks one frame of events
///      into the next, which reads as a note hit one frame early, every frame.
///
/// The handler is a real `EventHandler` impl rather than a hand-rolled loop, because the point is
/// that `repeat_all_miniquad_input` calls the *trait method* `prpr` implements - a loop pushing
/// straight into a `Vec` would pass even if that dispatch were broken.
struct TouchRecorder {
    events: Vec<(u64, u32, f32, f32, f64)>,
}

impl macroquad::miniquad::EventHandler for TouchRecorder {
    fn touch_event(
        &mut self,
        _ctx: &mut macroquad::miniquad::Context,
        phase: macroquad::miniquad::TouchPhase,
        id: u64,
        x: f32,
        y: f32,
        time: f64,
    ) {
        // A fieldless enum casts to its declaration order - the same encoding `wp81_game_touch`
        // documents for the shell, so the two sides cannot drift apart.
        self.events.push((id, phase as u32, x, y, time));
    }
}

fn dump_touches() {
    use macroquad::input::TouchPhase;

    // One finger pressed and held, plus a second that just went down: the two shapes a real frame
    // produces, and the pair that separates "state" from "event" - the held finger has no event this
    // frame, only state, so a reader that only looked at events would lose it.
    macroquad::input::push_touch(1001, TouchPhase::Started, 100.0, 200.0, 1.5);
    macroquad::input::push_touch(1001, TouchPhase::Stationary, 100.0, 200.0, 1.516);
    macroquad::input::push_touch(2002, TouchPhase::Started, 300.0, 400.0, 1.52);

    let state = macroquad::input::touches();
    eprintln!("[m1] touches() -> {} events", state.len());
    for t in &state {
        eprintln!(
            "       id={} phase={:?} pos=({},{}) time={}",
            t.id, t.phase, t.position.x, t.position.y, t.time
        );
    }

    let mut rec = TouchRecorder { events: Vec::new() };
    macroquad::utils::repeat_all_miniquad_input(&mut rec, 0);
    eprintln!("[m1] repeat_all_miniquad_input -> {} events", rec.events.len());
    for (id, phase, x, y, time) in &rec.events {
        eprintln!("       id={} phase={} pos=({},{}) time={}", id, phase, x, y, time);
    }

    macroquad::input::clear_touches();
    eprintln!(
        "[m1] after clear_touches -> {} events",
        macroquad::input::touches().len()
    );
}

fn dump_fixture(
    path: &str,
    frame: i32,
    ncmds: usize,
    cmds: *const PortCmd,
    verts: *const PortVertex,
    idx: *const u16,
    vhi: u32,
    ihi: u32,
) {
    const MAGIC: u32 = 0x5846_3150; // "P1FX"
    const VERSION: u32 = 1;
    /// The fixed 36-byte prelude documented above; also where the command array starts.
    const HEADER: usize = 0x24;
    let ntextures = prpr_port::wp81_texture_count();

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&(frame as u32).to_le_bytes());
    buf.extend_from_slice(&macroquad::window::screen_width().to_le_bytes());
    buf.extend_from_slice(&macroquad::window::screen_height().to_le_bytes());
    buf.extend_from_slice(&(ncmds as u32).to_le_bytes());
    buf.extend_from_slice(&vhi.to_le_bytes());
    buf.extend_from_slice(&ihi.to_le_bytes());
    buf.extend_from_slice(&ntextures.to_le_bytes());

    // The three arrays are `#[repr(C)]` with no padding - 6 `u32` then 8 `f32`, and 8 `f32` - so
    // their bytes are the file's bytes. `from_raw_parts` needs `ncmds` live entries at `cmds`,
    // which is exactly what the engine returned from `wp81_draw_encode`; the arenas are read to the
    // watermark rather than to capacity, because capacity is a constant of the shim and would write
    // the same 65536 vertices every time regardless of what the frame drew.
    unsafe {
        let nbytes = ncmds * core::mem::size_of::<PortCmd>();
        buf.extend_from_slice(core::slice::from_raw_parts(cmds.cast::<u8>(), nbytes));
        let nbytes = vhi as usize * core::mem::size_of::<PortVertex>();
        if !verts.is_null() {
            buf.extend_from_slice(core::slice::from_raw_parts(verts.cast::<u8>(), nbytes));
        }
        let nbytes = ihi as usize * core::mem::size_of::<u16>();
        if !idx.is_null() {
            buf.extend_from_slice(core::slice::from_raw_parts(idx.cast::<u8>(), nbytes));
        }
    }

    for i in 0..ntextures {
        let id = prpr_port::wp81_texture_id_at(i);
        let (mut w, mut h) = (0u32, 0u32);
        let bytes = unsafe {
            prpr_port::wp81_texture_data(id, &mut w, &mut h, core::ptr::null_mut(), 0)
        };
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&w.to_le_bytes());
        buf.extend_from_slice(&h.to_le_bytes());
        buf.extend_from_slice(&(bytes.max(0) as u32).to_le_bytes());
    }

    if let Err(e) = fs::write(path, &buf) {
        eprintln!("[m1] fixture NOT written to {path}: {e}");
        return;
    }
    // A checksum over the *commands* alone: two frames with the same counts can still be completely
    // different, and "the stream never changed over ten seconds" is a bug that would otherwise only
    // show up as a frozen picture on the phone, long after the renderer was believed to be wrong.
    let cmd_end = HEADER + ncmds * core::mem::size_of::<PortCmd>();
    let mut sum = 0u32;
    for b in &buf[HEADER..cmd_end] {
        sum = sum.wrapping_mul(31).wrapping_add(*b as u32);
    }
    eprintln!(
        "[m1] fixture -> {path} ({} bytes: frame {frame}, {ncmds} cmds, {vhi} verts, {ihi} idx, {ntextures} textures, cmdhash={sum:08x})",
        buf.len()
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: host_m1 <charts-dir> [folder] [chart] [frames] [fixture-out]");
        std::process::exit(2);
    }
    let charts_dir = PathBuf::from(&args[1]);
    let folder = args.get(2).cloned().unwrap_or_else(|| "demo".into());
    let chart = args.get(3).cloned().unwrap_or_else(|| "chart.json".into());
    let frames: i32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(120);
    let fixture_out = args.get(5).cloned();

    ROOT.set(charts_dir.clone()).ok();
    eprintln!("[m1] charts dir = {}", charts_dir.display());

    // `host.rs` reports construction failures with `tracing::error!`, and with no subscriber
    // installed `tracing` drops the event - leaving `GameCreate -> -2` as the whole diagnosis.
    // Installed before anything else so the very first error is captured.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::stderr)
        .init();
    eprintln!("[m1] port version = {}", prpr_port::wp81_port_version());

    // ---- install the shell ------------------------------------------------
    let cbs = HostCallbacks {
        audio_decode: Some(audio_decode),
        audio_music_new: Some(music_new),
        audio_music_play: Some(music_play),
        audio_music_pause: Some(music_pause),
        audio_music_seek: Some(music_seek),
        audio_music_position: Some(music_position),
        audio_music_paused: Some(music_paused),
        audio_music_set_amplifier: Some(music_amp),
        audio_music_set_low_pass: Some(music_lowpass),
        audio_music_fade: Some(music_fade),
        audio_sfx_new: Some(sfx_new),
        audio_sfx_play: Some(sfx_play),
        pick_file: Some(pick_file),
        save_file: Some(save_file),
        present: Some(present),
        load_asset: Some(load_asset),
    };
    unsafe { prpr_port::wp81_install_callbacks(&cbs) };
    eprintln!("[m1] callbacks installed");

    // The phone's viewport, so a projection bug cannot hide behind a different aspect ratio.
    prpr_port::wp81_game_set_screen(768.0, 1229.0);

    // ---- build the game ---------------------------------------------------
    let mut folder16: Vec<u16> = folder.encode_utf16().collect();
    folder16.push(0);
    let mut chart16: Vec<u16> = chart.encode_utf16().collect();
    chart16.push(0);
    let rc = unsafe { prpr_port::wp81_game_create(folder16.as_ptr(), chart16.as_ptr()) };
    eprintln!("[m1] GameCreate(\"{folder}\", \"{chart}\") -> {rc}");
    if rc != 0 {
        // The list of files the engine asked for before it gave up is usually the whole answer.
        if let Ok(rq) = REQUESTS.lock() {
            eprintln!("[m1] {} asset requests before the failure:", rq.len());
            for (n, c) in rq.iter() {
                eprintln!("       {c:>8}  {n}");
            }
        }
        std::process::exit(1);
    }
    eprintln!("[m1] running = {}", prpr_port::wp81_game_running() != 0);

    // ---- tick -------------------------------------------------------------
    let dt = 1.0 / 60.0;
    let loop_start = std::time::Instant::now();
    let mut best_cmds = 0i32;
    let mut first = true;
    let mut nonzero_frames = 0i32;
    let mut dumped_fixture = false;
    for f in 0..frames {
        // Music advances exactly as the frame does; see the note on `music_position`.
        set_music_pos(music_pos() + dt);
        let n = prpr_port::wp81_game_frame(dt);
        if n > 0 {
            nonzero_frames += 1;
        }
        best_cmds = best_cmds.max(n);
        if first || f == frames - 1 {
            first = false;
            let ncmds = prpr_port::wp81_draw_encode() as usize;
            let cmds = prpr_port::wp81_draw_commands();
            let verts = prpr_port::wp81_draw_vertices();
            let idx = prpr_port::wp81_draw_indices();
            let (vhi, ihi, kinds) = drain_stats(ncmds, cmds, verts, idx);
            eprintln!(
                "[m1] frame#{f}: game_frame -> {n} | encode -> {ncmds} cmds | verts_hi={vhi} idx_hi={ihi}"
            );
            eprintln!(
                "       kinds tex/rect/line/circ/geo = {}/{}/{}/{}/{}",
                kinds[0], kinds[1], kinds[2], kinds[3], kinds[4]
            );
            if f == 0 {
                dump_textures();
                // The touch path has no other desktop coverage: no finger reaches this harness, so
                // without this the first evidence of a broken queue would be a phone round.
                dump_touches();
            }
            // One fixture, taken from the first frame that actually produced something - frame 0
            // for an autoplay chart, but a chart whose music has not started yet draws nothing, so
            // the last frame is the fallback rather than silently capturing an empty stream.
            if let Some(path) = &fixture_out {
                if !dumped_fixture && ncmds > 0 {
                    dump_fixture(path, f, ncmds, cmds, verts, idx, vhi, ihi);
                    dumped_fixture = true;
                } else if dumped_fixture && f == frames - 1 && ncmds > 0 {
                    // The last frame too, side by side with the first. Same command *counts* over
                    // ten seconds of autoplay would mean the clock is not reaching the chart, and
                    // the two `cmdhash` lines are all it takes to see that.
                    dump_fixture(&(path.clone() + ".last"), f, ncmds, cmds, verts, idx, vhi, ihi);
                }
            }
            prpr_port::wp81_draw_reset();
        } else {
            prpr_port::wp81_draw_reset();
        }
    }
    // How long those frames actually took. `prpr`'s `TimeManager` runs off `std::time::Instant`,
    // i.e. *wall* time, while `wp81_game_frame` is handed a fixed 1/60 whether or not the loop
    // kept up - so a headless harness that renders faster than real time plays the scene at a
    // fraction of the speed it names. Six hundred "frames" here can be well under a second of
    // scene, which looks exactly like a frozen chart and is not one. The phone paces itself
    // against a real display, so this number is the difference between the two.
    let loop_secs = loop_start.elapsed().as_secs_f64();
    eprintln!(
        "[m1] {frames} frames in {loop_secs:.3} s wall ({:.0} fps) - scene advanced by wall clock, not by frames",
        frames as f64 / loop_secs.max(1e-9)
    );
    eprintln!(
        "[m1] {frames} frames, {} produced commands, best = {best_cmds}",
        nonzero_frames
    );
    eprintln!("[m1] game frames = {}", prpr_port::wp81_game_frames());

    // The shell's result page reads it through `wp81_game_result`, which walks `Main::scenes` ->
    // `Scene::as_any_mut` -> `downcast_mut::<GameScene>` -> `judge.result()`. Every step of that
    // chain answers -1 rather than failing loudly, so a hook that silently returns `None` would
    // show up only on the phone as a result page reading "0 notes, 0 perfect". Calling it here is
    // what turns that into a build-time failure.
    //
    // The counts are all expected to be 0 after two seconds of lead-in (no note judged yet);
    // `rc == 0` and `num_of_notes > 0` are the measurements, because together they mean the walk
    // both reached `GameScene::judge` and read the whole chart out of it.
    let mut res = prpr_port::host::PlayResultAbi {
        accuracy: 0.0,
        score: 0,
        max_combo: 0,
        num_of_notes: 0,
        counts: [0; 4],
        early: 0,
        late: 0,
        early_kind: [0; 4],
        late_kind: [0; 4],
    };
    let rc = unsafe { prpr_port::wp81_game_result(&mut res) };
    eprintln!(
        "[m1] game_result -> {rc} score={} acc={:.4} notes={} counts={:?} early={} late={}",
        res.score, res.accuracy, res.num_of_notes, res.counts, res.early, res.late
    );
    if rc != 0 {
        eprintln!("[m1] FAILED: as_any_mut/downcast never reached GameScene::judge");
        std::process::exit(1);
    }
    if res.num_of_notes == 0 {
        eprintln!("[m1] FAILED: judge reports 0 notes, so nothing was read out of it");
        std::process::exit(1);
    }

    prpr_port::wp81_game_destroy();
    eprintln!("[m1] destroyed, running = {}", prpr_port::wp81_game_running() != 0);

    if let Ok(rq) = REQUESTS.lock() {
        let miss = rq.iter().filter(|(_, c)| *c < 0).count();
        eprintln!("[m1] {} asset requests, {} missing:", rq.len(), miss);
        for (n, c) in rq.iter() {
            let mark = if *c < 0 { "MISS" } else { "ok  " };
            eprintln!("       {mark} {c:>8}  {n}");
        }
    }
}
