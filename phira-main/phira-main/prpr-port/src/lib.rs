//! The `prpr` → C# bridge for Windows Phone 8.1.
//!
//! # Direction of the two halves
//!
//! **C# calls Rust** with `DllImport`, which needs no ceremony.
//!
//! **Rust calls C#** is the awkward direction: a C# app cannot export a native entry point, so
//! there is nothing for `extern "C" { ... }` to bind to. Instead the shell hands over a table of
//! function pointers at startup (`wp81_install_callbacks`), and this crate forwards to it. The
//! shims declare their dependencies as plain `extern "C"` symbols; this file *defines* those
//! symbols, so the shims stay ignorant of how the shell is built.
//!
//! # Status
//!
//! Milestone 1: prove `prpr` links for this target and can parse a chart end to end. The parse
//! entry point is deliberately the smallest thing that exercises the real code path - the PEC
//! parser, the chart model, `nalgebra`, and the chart finalisation - rather than a synthetic
//! smoke test.

#![allow(non_snake_case, non_camel_case_types, unused_variables)]

use core::ffi::c_void;

// ===========================================================================
// the shell callback table
// ===========================================================================

/// Decoded audio metadata, matching the `sasa` shim's expectation.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub frame_count: u64,
}

/// Every native operation the Rust side needs from the shell.
///
/// All fields are optional so a partially-wired shell still links and reports "not implemented"
/// rather than crashing on a null call.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HostCallbacks {
    pub audio_decode:
        Option<unsafe extern "C" fn(data: *const u8, len: usize, out: *mut DecodedAudio) -> i32>,
    pub audio_music_new: Option<unsafe extern "C" fn(clip: u32, rate: f64, amp: f32, loop_mix: f64) -> u32>,
    pub audio_music_play: Option<unsafe extern "C" fn(id: u32)>,
    pub audio_music_pause: Option<unsafe extern "C" fn(id: u32)>,
    pub audio_music_seek: Option<unsafe extern "C" fn(id: u32, position: f64)>,
    pub audio_music_position: Option<unsafe extern "C" fn(id: u32) -> f64>,
    pub audio_music_paused: Option<unsafe extern "C" fn(id: u32) -> i32>,
    pub audio_music_set_amplifier: Option<unsafe extern "C" fn(id: u32, amp: f32)>,
    pub audio_music_set_low_pass: Option<unsafe extern "C" fn(id: u32, cutoff: f32)>,
    pub audio_music_fade: Option<unsafe extern "C" fn(id: u32, seconds: f64, out: i32)>,
    pub audio_sfx_new: Option<unsafe extern "C" fn(clip: u32) -> u32>,
    pub audio_sfx_play: Option<unsafe extern "C" fn(id: u32, amp: f32)>,
    pub pick_file: Option<unsafe extern "C" fn(title: *const u16, out: *mut u16, cap: usize) -> i32>,
    pub save_file:
        Option<unsafe extern "C" fn(title: *const u16, suggested: *const u16, out: *mut u16, cap: usize) -> i32>,
    /// Called by the renderer once per frame with the recorded draw commands.
    pub present: Option<unsafe extern "C" fn()>,
}

static mut CALLBACKS: HostCallbacks = HostCallbacks {
    audio_decode: None,
    audio_music_new: None,
    audio_music_play: None,
    audio_music_pause: None,
    audio_music_seek: None,
    audio_music_position: None,
    audio_music_paused: None,
    audio_music_set_amplifier: None,
    audio_music_set_low_pass: None,
    audio_music_fade: None,
    audio_sfx_new: None,
    audio_sfx_play: None,
    pick_file: None,
    save_file: None,
    present: None,
};

/// Installed once by the shell, before anything else is called.
///
/// # Safety
/// `cb` must point to a `HostCallbacks` that stays alive for the process lifetime - which is what a
/// `static` field holding the delegates achieves on the C# side.
#[no_mangle]
pub unsafe extern "C" fn wp81_install_callbacks(cb: *const HostCallbacks) {
    if cb.is_null() {
        return;
    }
    CALLBACKS = *cb;
}

#[inline]
fn cb() -> &'static HostCallbacks {
    unsafe { &*core::ptr::addr_of!(CALLBACKS) }
}

// ===========================================================================
// implementations of the shims' extern "C" dependencies
//
// These names must match the `extern "C"` blocks in rust-wp81/shims/{sasa,rfd}. They are defined
// here rather than in the shims so that the shims stay independent of the shell.
// ===========================================================================

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_decode(data: *const u8, len: usize, out: *mut DecodedAudio) -> i32 {
    match cb().audio_decode {
        Some(f) => f(data, len, out),
        None => 1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_new(clip: u32, rate: f64, amp: f32, loop_mix: f64) -> u32 {
    match cb().audio_music_new {
        Some(f) => f(clip, rate, amp, loop_mix),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_play(id: u32) {
    if let Some(f) = cb().audio_music_play {
        f(id)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_pause(id: u32) {
    if let Some(f) = cb().audio_music_pause {
        f(id)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_seek(id: u32, position: f64) {
    if let Some(f) = cb().audio_music_seek {
        f(id, position)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_position(id: u32) -> f64 {
    match cb().audio_music_position {
        Some(f) => f(id),
        None => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_paused(id: u32) -> i32 {
    match cb().audio_music_paused {
        Some(f) => f(id),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_set_amplifier(id: u32, amp: f32) {
    if let Some(f) = cb().audio_music_set_amplifier {
        f(id, amp)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_set_low_pass(id: u32, cutoff: f32) {
    if let Some(f) = cb().audio_music_set_low_pass {
        f(id, cutoff)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_fade(id: u32, seconds: f64, out: i32) {
    if let Some(f) = cb().audio_music_fade {
        f(id, seconds, out)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_sfx_new(clip: u32) -> u32 {
    match cb().audio_sfx_new {
        Some(f) => f(clip),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_audio_sfx_play(id: u32, amp: f32) {
    if let Some(f) = cb().audio_sfx_play {
        f(id, amp)
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_pick_file(title: *const u16, out: *mut u16, cap: usize) -> i32 {
    match cb().pick_file {
        Some(f) => f(title, out, cap),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn wp81_save_file(
    title: *const u16,
    suggested: *const u16,
    out: *mut u16,
    cap: usize,
) -> i32 {
    match cb().save_file {
        Some(f) => f(title, suggested, out, cap),
        None => 0,
    }
}

// ===========================================================================
// the C#-facing surface
// ===========================================================================

/// Parse a PEC chart and return its note count, or a negative error code.
///
/// The first real thing the port does, and deliberately the smallest one that exercises the actual
/// code path: the PEC parser, the chart model, `nalgebra`, and `Chart::finalize`.
///
/// Returns:
///   >= 0  total notes across all judge lines
///   -1    the bytes were not valid UTF-8
///   -2    the parser rejected the chart
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn wp81_parse_pec(data: *const u8, len: usize) -> i64 {
    if data.is_null() || len == 0 {
        return -2;
    }
    let bytes = core::slice::from_raw_parts(data, len);
    let text = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    match prpr::parse::parse_pec(text, prpr::core::ChartExtra::default()) {
        Ok(chart) => {
            let mut total = 0i64;
            for line in &chart.lines {
                total += line.notes.len() as i64;
            }
            unsafe { PARSED = Some(chart) };
            total
        }
        Err(_) => -2,
    }
}

/// Chart metadata, filled in by `wp81_chart_info`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ChartInfo {
    pub lines: u32,
    pub notes: u32,
    /// Duration in milliseconds.
    pub duration_ms: i64,
}

/// Report the shape of the chart most recently parsed by `wp81_parse_pec`.
///
/// Kept as a separate call so the parse entry point stays a pure "did it work" probe. The chart is
/// stored rather than returned across the boundary, because the model is large and C# has no use
/// for the Rust structs - it wants the numbers.
#[no_mangle]
pub unsafe extern "C" fn wp81_chart_info(out: *mut ChartInfo) -> i32 {
    if out.is_null() {
        return -1;
    }
    match PARSED.as_ref() {
        Some(chart) => {
            let mut notes = 0u32;
            // `Chart` has no duration field, so it is derived: the last note time across all lines.
            // Holds also carry an `end_time`, and taking it into account matters because a chart
            // can end on a hold that outlasts its final note.
            // `note.time` and `NoteKind::Hold::end_time` are both f64, not f32.
            let mut end = 0.0f64;
            for line in &chart.lines {
                for note in &line.notes {
                    if note.time > end {
                        end = note.time;
                    }
                    // Matched by reference: `NoteKind` is not guaranteed `Copy`, and moving out of
                    // a borrowed note would not compile.
                    if let prpr::core::NoteKind::Hold { end_time, .. } = &note.kind {
                        if *end_time > end {
                            end = *end_time;
                        }
                    }
                }
            }
            for line in &chart.lines {
                notes += line.notes.len() as u32;
            }
            *out = ChartInfo {
                lines: chart.lines.len() as u32,
                notes,
                duration_ms: (end * 1000.0) as i64,
            };
            0
        }
        None => -2,
    }
}

/// The chart from the last successful parse.
///
/// A single slot rather than a handle table: `prpr`'s chart model is not `Send`, C# drives one
/// chart at a time, and a registry would add lifetime questions with no benefit at this stage.
static mut PARSED: Option<prpr::core::Chart> = None;

/// Version of the port, so a stale deployment is identifiable from the log.
#[no_mangle]
pub extern "C" fn wp81_port_version() -> u32 {
    // Bump when the ABI changes; the shell logs it next to the DLL size.
    1
}

// ===========================================================================
// libm
//
// `pxfm` (reached through prpr's image/blur path) calls the C float-maths entry points, and a
// /NODEFAULTLIB link has no libm to resolve them against. These forward to the `libm` crate, a
// pure-Rust implementation, so the symbols come from Rust rather than from a CRT that cannot be
// linked on WP8.1.
//
// The list is exactly what the linker asked for, plus the obvious siblings - a missing name shows up
// as an unresolved external rather than as anything subtle, so the cost of being wrong is one more
// build cycle and not a silent misbehaviour.
// ===========================================================================

macro_rules! libm_shim {
    ($name:ident, $imp:path, ($($a:ident : $t:ty),*) -> $ret:ty) => {
        #[no_mangle]
        pub extern "C" fn $name($($a: $t),*) -> $ret {
            $imp($($a),*)
        }
    };
}

// f32
libm_shim!(roundf, libm::roundf, (x: f32) -> f32);
libm_shim!(sinf, libm::sinf, (x: f32) -> f32);
libm_shim!(cosf, libm::cosf, (x: f32) -> f32);
libm_shim!(tanf, libm::tanf, (x: f32) -> f32);
libm_shim!(asinf, libm::asinf, (x: f32) -> f32);
libm_shim!(floorf, libm::floorf, (x: f32) -> f32);
libm_shim!(ceilf, libm::ceilf, (x: f32) -> f32);
libm_shim!(truncf, libm::truncf, (x: f32) -> f32);
libm_shim!(expf, libm::expf, (x: f32) -> f32);
libm_shim!(exp2f, libm::exp2f, (x: f32) -> f32);
libm_shim!(log2f, libm::log2f, (x: f32) -> f32);
libm_shim!(log10f, libm::log10f, (x: f32) -> f32);
libm_shim!(powf, libm::powf, (x: f32, y: f32) -> f32);
libm_shim!(fmodf, libm::fmodf, (x: f32, y: f32) -> f32);
libm_shim!(atan2f, libm::atan2f, (y: f32, x: f32) -> f32);
libm_shim!(_hypotf, libm::hypotf, (x: f32, y: f32) -> f32);

// f64
libm_shim!(round, libm::round, (x: f64) -> f64);
libm_shim!(sin, libm::sin, (x: f64) -> f64);
libm_shim!(cos, libm::cos, (x: f64) -> f64);
libm_shim!(floor, libm::floor, (x: f64) -> f64);
libm_shim!(ceil, libm::ceil, (x: f64) -> f64);
libm_shim!(trunc, libm::trunc, (x: f64) -> f64);
libm_shim!(exp, libm::exp, (x: f64) -> f64);
libm_shim!(pow, libm::pow, (x: f64, y: f64) -> f64);
libm_shim!(atan2, libm::atan2, (y: f64, x: f64) -> f64);
libm_shim!(fma, libm::fma, (x: f64, y: f64, z: f64) -> f64);

// ===========================================================================
// CRT-free DLL entry point
//
// Same requirement as wp81probe.dll: WP8.1 native DLLs must not import KERNEL32.dll, so the link
// uses /NODEFAULTLIB and the entry point comes from here.
// ===========================================================================

#[no_mangle]
pub static _fltused: i32 = 0;

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    _module: *mut c_void,
    _reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    // Deliberately NOT calling DisableThreadLibraryCalls: Rust std's thread-locals want the thread
    // attach/detach notifications.
    1
}

#[no_mangle]
pub unsafe extern "system" fn _DllMainCRTStartup(
    module: *mut c_void,
    reason: u32,
    reserved: *mut c_void,
) -> i32 {
    DllMain(module, reason, reserved)
}
