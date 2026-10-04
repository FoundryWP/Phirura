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

// ===========================================================================
// container headers
//
// The ABI only carries `sample_rate` and `frame_count` - the shell keeps the bytes and does the
// actual decoding itself when it comes time to play - so what is needed here is a *header* read,
// never samples. `AudioClip::length()` and `Music::length()` are the two consumers, and both are
// `frames / rate`.
//
// Three containers appear in practice: the chart's music (WAVE or MP3 depending on who exported
// it) and every sound effect in the resource pack (Ogg Vorbis). All three are pure `&[u8]`
// walks - no unsafe, and no allocation beyond what the slices already are.
// ===========================================================================

/// RIFF/WAVE: `fmt ` for rate and frame size, `data` for the count.
fn wav_info(b: &[u8]) -> Option<(u32, u64)> {
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data_len = 0u64;
    let mut i = 12usize;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        if body + size > b.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes([b[body + 2], b[body + 3]]);
            rate = u32::from_le_bytes([b[body + 4], b[body + 5], b[body + 6], b[body + 7]]);
            bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
        } else if id == b"data" {
            data_len = size as u64;
        }
        i = body + size + (size & 1);
    }
    if rate == 0 || channels == 0 || bits == 0 {
        return None;
    }
    let frame = (channels as u64) * ((bits as u64) / 8).max(1);
    Some((rate, data_len / frame.max(1)))
}

/// Ogg Vorbis (and Opus) page walk.
///
/// Rate comes from the identification packet on the first page; length from the granule position
/// of the last page that finished a packet. A granule of `u64::MAX` is the "no packet completed on
/// this page" sentinel and is skipped rather than taken as the length.
fn ogg_info(b: &[u8]) -> Option<(u32, u64)> {
    if b.len() < 27 || &b[0..4] != b"OggS" {
        return None;
    }

    // Identification packet: the first page's payload.
    let nseg = b[26] as usize;
    if b.len() < 27 + nseg {
        return None;
    }
    let head = b.get(27 + nseg..)?;

    let (rate, pre_skip) = if head.len() >= 16 && head[0] == 1 && &head[1..7] == b"vorbis" {
        // type(1) "vorbis"(6) version(u32) channels(u8) sample_rate(u32)
        let r = u32::from_le_bytes([head[12], head[13], head[14], head[15]]);
        if r == 0 {
            return None;
        }
        (r, 0u64)
    } else if head.len() >= 12 && &head[0..8] == b"OpusHead" {
        // Opus always decodes at 48 kHz regardless of the input rate in the header, and granule
        // positions are counted in that 48 kHz domain, minus the pre-skip.
        let skip = u16::from_le_bytes([head[10], head[11]]) as u64;
        (48000, skip)
    } else {
        return None;
    };

    // Walk every page forward, remembering the last usable granule.
    let mut granule = 0u64;
    let mut i = 0usize;
    while i + 27 <= b.len() {
        if &b[i..i + 4] != b"OggS" {
            break;
        }
        let nseg = b[i + 26] as usize;
        if i + 27 + nseg > b.len() {
            break;
        }
        let seglen: usize = b[i + 27..i + 27 + nseg].iter().map(|&x| x as usize).sum();
        let body = i + 27 + nseg;
        if body.saturating_add(seglen) > b.len() {
            break;
        }
        let g = u64::from_le_bytes([
            b[i + 6], b[i + 7], b[i + 8], b[i + 9], b[i + 10], b[i + 11], b[i + 12], b[i + 13],
        ]);
        if g != u64::MAX {
            granule = g;
        }
        i = body + seglen;
    }

    let frames = granule.saturating_sub(pre_skip);
    if frames == 0 {
        return None;
    }
    Some((rate, frames))
}

/// MPEG Layer III frame walk.
///
/// Layer III only - every `.mp3` ever exported is, and Layer I and II have a different frame-length
/// formula that nothing here will ever see. Rate comes from the first frame header; length comes
/// from the `Xing`/`Info` tag when the encoder wrote one (a single number, exact), and otherwise
/// from walking every frame, which is how a VBR file without a tag is measured.
fn mp3_info(b: &[u8]) -> Option<(u32, u64)> {
    // Bitrates in kbit/s, index 0 = "free format" and 15 = "bad", both unusable.
    const BR_V1: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const BR_V2: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    // Sample rates by MPEG version: 0 = 2.5, 1 = reserved, 2 = 2, 3 = 1.
    const SR: [[u32; 3]; 4] = [
        [11025, 12000, 8000],
        [0, 0, 0],
        [22050, 24000, 16000],
        [44100, 48000, 32000],
    ];

    let frame_len = |pos: usize| -> Option<(u32, u64, usize)> {
        if pos + 4 > b.len() || b[pos] != 0xff || b[pos + 1] & 0xe0 != 0xe0 {
            return None;
        }
        let version = (b[pos + 1] >> 3) & 0b11;
        let layer = (b[pos + 1] >> 1) & 0b11;
        let bi = (b[pos + 2] >> 4) as usize;
        let si = ((b[pos + 2] >> 2) & 0b11) as usize;
        let padding = ((b[pos + 2] >> 1) & 1) as u32;
        if version == 1 || layer != 1 || si == 3 || bi == 0 || bi == 15 {
            return None;
        }
        let rate = SR[version as usize][si];
        let bitrate = (if version == 3 { BR_V1 } else { BR_V2 })[bi] * 1000;
        // Bytes per frame = (samples_per_frame / 8) * bitrate / sample_rate, plus the padding bit.
        // 144 for MPEG-1 (1152 samples), 72 for MPEG-2/2.5 (576 samples) - both fall out of this.
        let samples: u64 = if version == 3 { 1152 } else { 576 };
        let len = (samples / 8) * bitrate as u64 / rate as u64;
        Some((rate, samples, len as usize + padding as usize))
    };

    // Skip an ID3v2 tag when the file opens with one: "ID3" + version(2) + flags(1) + four
    // synchsafe size bytes, plus a 10-byte footer when the footer flag says there is one.
    let mut i = 0usize;
    if b.len() >= 10 && &b[0..3] == b"ID3" {
        let size = ((b[6] as usize & 0x7f) << 21)
            | ((b[7] as usize & 0x7f) << 14)
            | ((b[8] as usize & 0x7f) << 7)
            | (b[9] as usize & 0x7f);
        i = 10usize.saturating_add(size);
        if b[5] & 0x10 != 0 {
            i = i.saturating_add(10);
        }
    }
    // Then the first frame, however far the tag or any leading junk put it.
    while i + 4 <= b.len() && frame_len(i).is_none() {
        i += 1;
    }
    let (rate, samples, _) = frame_len(i)?;

    // A Xing/Info header, when present, states the frame count outright. It sits after the side
    // information in the first frame: 17/32 bytes for MPEG-1 (mono/other), 9/17 for MPEG-2 and 2.5.
    let mono = b[i + 3] >> 6 == 0b11;
    let side = match ((b[i + 1] >> 3) & 0b11, mono) {
        (3, true) => 17,
        (3, false) => 32,
        (_, true) => 9,
        (_, false) => 17,
    };
    let xing = i + 4 + side;
    let mut frames = if xing + 12 <= b.len()
        && (&b[xing..xing + 4] == b"Xing" || &b[xing..xing + 4] == b"Info")
        && u32::from_be_bytes([b[xing + 4], b[xing + 5], b[xing + 6], b[xing + 7]]) & 1 != 0
    {
        Some(
            u32::from_be_bytes([b[xing + 8], b[xing + 9], b[xing + 10], b[xing + 11]]) as u64,
        )
    } else {
        None
    };

    // Otherwise walk. Bitrate may change from frame to frame in VBR, so the header is re-read each
    // time rather than the first frame's length being applied to the whole file.
    if frames.is_none() {
        let mut count = 0u64;
        let mut pos = i;
        while let Some((_, _, len)) = frame_len(pos) {
            if len == 0 {
                break;
            }
            count += 1;
            pos = pos.saturating_add(len);
        }
        frames = Some(count);
    }

    let frames = frames?;
    if frames == 0 {
        return None;
    }
    Some((rate, frames * samples))
}

/// Route a byte buffer to the right container parser.
///
/// Dispatches on the leading four bytes rather than the file name: the shell hands the bytes over
/// and the extension is whatever the chart author happened to write.
fn audio_info(b: &[u8]) -> Option<(u32, u64)> {
    if b.len() < 16 {
        return None;
    }
    match &b[0..4] {
        b"RIFF" => wav_info(b),
        b"OggS" => ogg_info(b),
        // MPEG frames open with the 11-bit sync `11111111 111`, but an MP3 usually opens with an
        // ID3v2 tag instead, which `mp3_info` skips on its own. `ID3` cannot collide with either
        // of the other two signatures, so the fourth arm is unambiguous.
        _ if b[0] == 0xff || &b[0..3] == b"ID3" => mp3_info(b),
        _ => None,
    }
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

extern "C" fn music_new(_clip: u32, _rate: f64, _amp: f32, _loop_mix: f64) -> u32 {
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
extern "C" fn sfx_new(_clip: u32) -> u32 {
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
    eprintln!(
        "[m1] {n} textures registered, {} dropped for room",
        prpr_port::wp81_texture_overflow()
    );
    for i in 0..n {
        let id = prpr_port::wp81_texture_id_at(i);
        let (mut w, mut h) = (0u32, 0u32);
        let bytes = unsafe {
            prpr_port::wp81_texture_data(id, &mut w, &mut h, core::ptr::null_mut(), 0)
        };
        eprintln!("       id={id} {w}x{h} {bytes} bytes");
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: host_m1 <charts-dir> [folder] [chart] [frames]");
        std::process::exit(2);
    }
    let charts_dir = PathBuf::from(&args[1]);
    let folder = args.get(2).cloned().unwrap_or_else(|| "demo".into());
    let chart = args.get(3).cloned().unwrap_or_else(|| "chart.json".into());
    let frames: i32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(120);

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
    let mut best_cmds = 0i32;
    let mut first = true;
    let mut nonzero_frames = 0i32;
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
            }
            prpr_port::wp81_draw_reset();
        } else {
            prpr_port::wp81_draw_reset();
        }
    }
    eprintln!(
        "[m1] {frames} frames, {} produced commands, best = {best_cmds}",
        nonzero_frames
    );
    eprintln!("[m1] game frames = {}", prpr_port::wp81_game_frames());
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
