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

/// The game host: builds a real `prpr` `GameScene` and ticks it. Kept out of this file so the ABI
/// surface stays readable.
pub mod host;

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
    /// Read a file the game needs - the chart, its textures, a font.
    ///
    /// `name` is a NUL-terminated UTF-16 path relative to the app's local `charts\` folder;
    /// `out`/`cap` receive the bytes. Returns the byte count, or a negative value when the file
    /// does not exist. This is how the Rust game reaches the shell's storage without owning any
    /// file IO itself.
    pub load_asset: Option<unsafe extern "C" fn(name: *const u16, out: *mut u8, cap: usize) -> i32>,
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
    load_asset: None,
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
    // The macroquad shim's `load_file` cannot see `CALLBACKS` - it is a different crate with no
    // dependency on this one, which is deliberate. So the address of this plain function is
    // handed over instead, and `prpr`'s resource pack (`respack/info.yml`, the note textures, the
    // hit sounds) resolves through the same shell callback as the chart itself. Without this the
    // scene cannot be constructed at all: `ResourcePack` falls back to
    // `fs_from_assets("respack/")`, which has no way to ask anyone for bytes.
    macroquad::set_asset_hook(asset_hook);
}

/// Forwarder for the macroquad shim's asset hook.
///
/// A plain `fn`, not a closure, because that is what the shim stores in a `static`. The path
/// arrives unchanged - `respack/info.yml` from the resource pack, `click.ogg` from the sound
/// fallback - and the shell resolves both against its asset root.
fn asset_hook(path: &str) -> Option<Vec<u8>> {
    host::load_asset(path)
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
    // 2: added the renderer drain (wp81_draw_* below).
    // 3: added the game host (wp81_game_*), which runs the real prpr scene.
    3
}

// ===========================================================================
// the renderer drain
//
// `prpr`'s drawing all goes through the macroquad shim's `draw_*` functions and its `QuadGl`
// batcher, both of which *record* rather than draw. The shell polls this pair once per frame,
// replays the commands through the same Win2D renderer the C# port already uses, then calls
// `wp81_draw_reset` so the next frame starts clean.
//
// The two calls are split because the C# side needs the count before it can size its read:
//
//   let n = wp81_draw_encode();          // translate the frame's commands
//   let cmds = wp81_draw_commands();     // -> PortCmd[n]
//   let verts = wp81_draw_vertices();    // -> PortVertex[...]  (only for Geometry records)
//
// The returned pointers are into the DLL's own static arenas and stay valid until the next
// `wp81_draw_encode`, so the shell must not cache them across frames.
// ===========================================================================

/// Flatten this frame's recorded draw commands and return how many there are.
///
/// Returns 0 when nothing was drawn (which is the normal state until the Rust scene is driven
/// from the shell - see the note below).
#[no_mangle]
pub extern "C" fn wp81_draw_encode() -> u32 {
    macroquad::draw::encode()
}

/// Pointer to the flat command array. Re-read after every `wp81_draw_encode`.
#[no_mangle]
pub extern "C" fn wp81_draw_commands() -> *const macroquad::draw::PortCmd {
    macroquad::draw::port_commands()
}

/// Pointer to the flat vertex array, indexed by a `Geometry` record's `vstart`.
#[no_mangle]
pub extern "C" fn wp81_draw_vertices() -> *const macroquad::draw::PortVertex {
    macroquad::draw::port_vertices()
}

/// Pointer to the u16 index arena, read by a `Geometry` record's `istart`/`icount`.
#[no_mangle]
pub extern "C" fn wp81_draw_indices() -> *const u16 {
    macroquad::draw::port_indices()
}

/// Clear the frame's buffers. Call once the commands have been replayed.
#[no_mangle]
pub extern "C" fn wp81_draw_reset() {
    macroquad::draw::drain_reset();
}

// ===========================================================================
// the texture registry
//
// A `Texture` record in the drain carries only a `u32` id, and the shim allocates those ids where
// it sees pixels rather than filenames - so the shell cannot resolve an id to an image by itself.
// These three calls are how it does: enumerate the ids, ask for one's bytes, build a
// `CanvasBitmap`, keep it. One copy per texture per session, not per frame.
// ===========================================================================

/// How many textures the shim has registered. The shell iterates `0..count`.
#[no_mangle]
pub extern "C" fn wp81_texture_count() -> u32 {
    macroquad::draw::texture_count()
}

/// The id of the `index`-th registered texture, or 0 when out of range.
#[no_mangle]
pub extern "C" fn wp81_texture_id_at(index: u32) -> u32 {
    macroquad::draw::texture_id_at(index)
}

/// Copy a texture's RGBA pixels out.
///
/// `width`/`height` receive the dimensions (nullable for a metadata-only query). `out`/`cap` receive
/// the pixels; passing a null `out` returns the byte length so the caller can size its buffer.
///
/// Returns the byte count, -1 when the id is unknown, or -2 when it does not fit `cap`.
///
/// # Safety
/// `out` must have room for `cap` bytes; `width`/`height` must be valid for one `u32` each or null.
#[no_mangle]
pub unsafe extern "C" fn wp81_texture_data(
    id: u32,
    width: *mut u32,
    height: *mut u32,
    out: *mut u8,
    cap: usize,
) -> i32 {
    macroquad::draw::texture_data(id, width, height, out, cap)
}

/// Textures the registry had to drop for lack of room. Non-zero means a chart exceeded its budget.
#[no_mangle]
pub extern "C" fn wp81_texture_overflow() -> u32 {
    macroquad::draw::texture_overflow()
}

/// Record a test triangle so the drain can be verified end to end on the device.
///
/// The real scene is not driven from the shell yet, so without this the drain has nothing to
/// prove: `wp81_draw_encode` would always return 0 and a broken pointer would look like an idle
/// frame. This pushes one `Geometry` batch with a triangle-sized payload, which is exactly the
/// path `prpr`'s note rendering uses.
///
/// Called from the C# diagnostics path only; it is inert unless invoked.
#[no_mangle]
pub unsafe extern "C" fn wp81_draw_selftest() {
    use macroquad::draw::*;
    let verts = [
        macroquad::Vertex::new(0.0, -0.5, 0.0, 0.0, 0.0, macroquad::Color::new(1.0, 0.0, 0.0, 1.0)),
        macroquad::Vertex::new(0.5, 0.5, 0.0, 1.0, 0.0, macroquad::Color::new(0.0, 1.0, 0.0, 1.0)),
        macroquad::Vertex::new(-0.5, 0.5, 0.0, 0.0, 1.0, macroquad::Color::new(0.0, 0.0, 1.0, 1.0)),
    ];
    let indices: [u16; 3] = [0, 1, 2];
    set_texture(None);
    set_draw_mode(macroquad::DrawMode::Triangles);
    push_geometry(&verts, &indices);
}

// ===========================================================================
// the game host
//
// These are the entry points that make the *real* Rust Phira run: `wp81_game_create` builds a
// `prpr::scene::GameScene` and wraps it in a `Main`, and `wp81_game_frame` ticks one frame, leaving
// its draw commands for `wp81_draw_*` to hand to the shell.
//
// The split is deliberate. Construction is expensive (chart parse, texture registration, scene
// setup) and happens once; a frame is cheap and happens 60 times a second. Keeping them separate
// means the shell pays the cost once and never blocks the render loop on a parse.
// ===========================================================================

/// Build a game for a chart in the shell's `charts\<folder>\` directory.
///
/// `folder` and `chart_name` are UTF-16, NUL-terminated. Returns `0` on success, or a negative code
/// (`PrprPort.cs` mirrors them):
///
///   -1  nothing to load          -2  the chart or scene failed to build
///   -3  a game is already running  -100  the scene panicked
#[no_mangle]
pub unsafe extern "C" fn wp81_game_create(
    folder: *const u16,
    chart_name: *const u16,
) -> i32 {
    let folder = match utf16_to_string(folder) {
        Some(s) => s,
        None => return host::GAME_ERR_NO_CHART,
    };
    let chart_name = match utf16_to_string(chart_name) {
        Some(s) => s,
        None => return host::GAME_ERR_NO_CHART,
    };
    host::create(&folder, &chart_name)
}

/// Advance and render one frame. `dt` is the seconds since the previous call.
///
/// Returns the number of draw commands produced (readable through `wp81_draw_commands`), or a
/// negative code when nothing is running.
#[no_mangle]
pub extern "C" fn wp81_game_frame(dt: f64) -> i32 {
    host::frame(dt)
}

/// Stop the game and release the scene. Safe to call when nothing is running.
#[no_mangle]
pub extern "C" fn wp81_game_destroy() {
    host::destroy()
}

/// How far `wp81_game_create` has progressed, for the shell to sample while it runs.
///
/// This export exists because of one specific failure mode: `create` stops and the process goes with
/// it, leaving no return code and no log line after the last one the shell wrote. The shell polls
/// this from a timer that does not share `create`'s thread and prints the step whenever it changes,
/// so the final line before silence names what was executing. It is the same instrument as the
/// renderer's `SampleTick`, which is what localised the earlier bring-up hangs.
///
/// The step numbers themselves live in `host::progress`, so neither side restates them.
#[no_mangle]
pub extern "C" fn wp81_game_progress() -> u32 {
    host::progress_now()
}

/// Frames rendered so far, or -1 when no game is running.
#[no_mangle]
pub extern "C" fn wp81_game_frames() -> i64 {
    host::frames()
}

/// Whether a game is currently hosted.
#[no_mangle]
pub extern "C" fn wp81_game_running() -> i32 {
    if host::is_running() { 1 } else { 0 }
}

/// Tell the host the shell's screen size, before the first frame.
///
/// `prpr` projects through `screen_width`/`screen_height`/`screen_aspect`, which all read a static
/// in the macroquad shim. Left at 0x0 every projection is degenerate and the chart renders as a
/// single point, so this must be called once after the surface is measured.
#[no_mangle]
pub extern "C" fn wp81_game_set_screen(width: f32, height: f32) {
    host::set_screen(width, height);
}

/// Decode a NUL-terminated UTF-16 string from the shell.
///
/// The shell hands over UTF-16 because that is what .NET strings and the WinRT file APIs use, so
/// converting on the C# side would mean an extra allocation per call.
unsafe fn utf16_to_string(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    if len == 0 {
        return None;
    }
    let slice = core::slice::from_raw_parts(p, len);
    String::from_utf16(slice).ok()
}

/// The PEC sample the C# shell feeds to `RunSelfTest`, kept here so the two cannot drift.
///
/// `MainPage.xaml.cs` holds the same text as a `const string`. If either copy is edited the other
/// has to follow, and the test below is what catches it: the shell's self-test only reports
/// "parse returned -2", which is indistinguishable from a deployment problem, whereas this test
/// names the line and the token that the parser rejected.
///
/// PEC in `prpr` is not the generic PEC dialect - see `prpr/src/parse/pec.rs`:
/// `n1`=Click, `n2`=Hold (an extra end-time token), `n3`=Flick, `n4`=Drag; line 1 is the offset in
/// milliseconds; and every command is a *strict* token count, because the parser rejects any
/// trailing token with `unexpected-extra`.
#[cfg(test)]
const SAMPLE_PEC: &str = "0\nbp 120 4\nn1 0 0 512 0 0\nn2 0 0 256 0 0 512\nn3 0 0 0 1 0\nn4 0 0 768 0 1\ncv 0 0 1\ncp 0 0 0 0\n";

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample must parse, and must produce the four notes the shell expects to see. Asserting
    /// the count matters: a chart that parses but yields 0 notes would still report success.
    #[test]
    fn sample_pec_parses() {
        let chart = prpr::parse::parse_pec(SAMPLE_PEC, prpr::core::ChartExtra::default())
            .expect("the shell's sample PEC must parse - see the error above for the line");
        let notes: usize = chart.lines.iter().map(|l| l.notes.len()).sum();
        assert_eq!(notes, 4, "click + hold + flick + drag");
        assert_eq!(chart.lines.len(), 1, "all four notes are on judge line 0");
    }

    /// The entry points the C# side calls, exercised through the same path the DLL uses.
    ///
    /// `wp81_parse_pec` is `unsafe` and dereferences a raw pointer, so the test builds the byte
    /// slice explicitly rather than reaching for `as_ptr()` on a temporary.
    #[test]
    fn entry_points_agree_with_the_parser() {
        let bytes = SAMPLE_PEC.as_bytes();
        let notes = unsafe { wp81_parse_pec(bytes.as_ptr(), bytes.len()) };
        assert_eq!(notes, 4, "wp81_parse_pec should report the same 4 notes");

        let mut info = ChartInfo::default();
        let rc = unsafe { wp81_chart_info(&mut info) };
        assert_eq!(rc, 0);
        assert_eq!(info.lines, 1);
        assert_eq!(info.notes, 4);
        // Every note is at beat 0 of a 120 BPM chart, so the chart is essentially instant; the
        // exact value is not the point, "it was computed rather than left at 0" is.
        assert!(info.duration_ms >= 0);
    }

    /// A malformed chart must be reported as an error, not silently accepted - `wp81_parse_pec`'s
    /// `-2` is the only signal the shell gets, so it has to be reliable.
    #[test]
    fn bad_pec_is_rejected() {
        let bad = b"0\nnot-a-command 1 2 3\n";
        let rc = unsafe { wp81_parse_pec(bad.as_ptr(), bad.len()) };
        assert_eq!(rc, -2);
    }
}


// ===========================================================================
// the compat anchor
//
// `wp81-compat` is a *dependency*, but rustc only pulls objects out of a dependency rlib when
// something references them - and nothing here calls `NtCreateFile` or `lstrlenW`. They are
// referenced from **patched std**, which is a separate compilation unit, so without an anchor the
// linker silently drops the whole crate and the link then reports 13 unresolved externals
// (`lstrlenW`, `atexit`, `LoadLibraryA`, `NtCreateFile`, `GetUserProfileDirectoryW`, …) for symbols
// that exist one directory away and are already compiled.
//
// The probe crate hit this first and solved it the same way (`link-probe::wp81_anchor_compat`), so
// this mirrors that: take each shim's address at *runtime* and fold them together. Taking the
// addresses in a `static` initialiser would not work - a pointer has no value in const evaluation -
// and the function is `#[no_mangle]` so the linker keeps it, then `/EXPORT:wp81_anchor_port` in
// build-port.ps1 forces it into the DLL's export table, which is what actually drags the archive
// members in. None of the shims are *called* here: invoking `NtCreateFile` with null arguments
// would be meaningless.
//
// **When the link reports an unresolved external that compat defines, add it here.** The export
// list and this function are one mechanism; a symbol in neither is not linked.
// ===========================================================================

// The whole anchor mechanism below is ARM-only: it exists purely to stop the `/NODEFAULTLIB` link
// from dropping shims and MSVC ARM runtime helpers that are referenced from *outside* this crate.
// On a host link the CRT supplies every one of those names already, `wp81-compat` is not even in
// the dependency graph (see the `cfg` on its manifest entry), and taking the address of the
// externs below would be an unresolved external rather than an anchor.

// The two `global_asm!` divide helpers have no Rust item to name, so they are opened as `extern`
// declarations purely to take their addresses for the anchor.
#[cfg(target_arch = "arm")]
extern "C" {
    fn __rt_udiv64();
    fn __rt_sdiv64();
}

#[cfg(target_arch = "arm")]
#[inline]
fn __rt_udiv64_addr() -> usize {
    __rt_udiv64 as unsafe extern "C" fn() as usize
}

#[cfg(target_arch = "arm")]
#[inline]
fn __rt_sdiv64_addr() -> usize {
    __rt_sdiv64 as unsafe extern "C" fn() as usize
}

#[cfg(target_arch = "arm")]
#[no_mangle]
pub extern "C" fn wp81_anchor_port() -> usize {
    let anchors: [usize; 45] = [
        // console / stdio
        wp81_compat::GetConsoleMode as usize,
        wp81_compat::GetConsoleOutputCP as usize,
        wp81_compat::ReadConsoleW as usize,
        wp81_compat::WriteConsoleW as usize,
        wp81_compat::GetStdHandle as usize,
        wp81_compat::SetStdHandle as usize,
        wp81_compat::GetConsoleCP as usize,
        wp81_compat::SetConsoleCtrlHandler as usize,
        // process environment
        wp81_compat::GetCommandLineW as usize,
        wp81_compat::GetEnvironmentStringsW as usize,
        wp81_compat::FreeEnvironmentStringsW as usize,
        wp81_compat::GetUserProfileDirectoryW as usize,
        wp81_compat::GetEnvironmentVariableW as usize,
        wp81_compat::SetEnvironmentVariableW as usize,
        wp81_compat::GetCurrentDirectoryW as usize,
        wp81_compat::SetCurrentDirectoryW as usize,
        wp81_compat::GetActiveProcessorCount as usize,
        // threads / pipes
        wp81_compat::WaitForMultipleObjects as usize,
        wp81_compat::CreatePipe as usize,
        // NT file layer
        wp81_compat::NtCreateFile as usize,
        wp81_compat::NtOpenFile as usize,
        wp81_compat::NtReadFile as usize,
        wp81_compat::NtWriteFile as usize,
        wp81_compat::NtSetInformationFile as usize,
        wp81_compat::NtCreateNamedPipeFile as usize,
        wp81_compat::RtlNtStatusToDosError as usize,
        wp81_compat::wp81_close_handle as usize,
        // CRNG, loader, strings, exit
        wp81_compat::ProcessPrng as usize,
        wp81_compat::LoadLibraryA as usize,
        wp81_compat::lstrlenW as usize,
        wp81_compat::strlen as usize,
        wp81_compat::wcslen as usize,
        wp81_compat::atexit as usize,
        // `_tls_index` is a `static mut` and the loader writes it from the image's TLS directory at
        // load time, so taking its address is an unsafe read of mutable static. Only the *address*
        // is taken; nothing here reads or writes its value.
        unsafe { core::ptr::addr_of!(wp81_compat::_tls_index) as usize },
        // MSVC ARM runtime helpers. These are called from compiler_builtins and from LLVM-generated
        // code in *any* crate, so like the shims they are referenced from outside this one and need
        // the same anchor. `__rt_udiv64` / `__rt_sdiv64` are `global_asm!` blocks rather than Rust
        // functions - written in assembly precisely because a 16-byte register return cannot be
        // expressed in Rust - so they are reached through the `extern` block below rather than
        // through the crate path.
        wp81_compat::__rt_udiv as usize,
        wp81_compat::__i64tod as usize,
        wp81_compat::__i64tos as usize,
        wp81_compat::__dtoi64 as usize,
        wp81_compat::__dtou64 as usize,
        wp81_compat::__stoi64 as usize,
        wp81_compat::__stou64 as usize,
        wp81_compat::__u64tod as usize,
        wp81_compat::__u64tos as usize,
        __rt_udiv64_addr() as usize,
        __rt_sdiv64_addr() as usize,
    ];
    let mut acc = 0usize;
    for a in anchors {
        acc ^= a;
    }
    acc
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

// Needed on the host build as well as the phone, which is not obvious: the host link here has no
// UCRT in `LIB` (only `VC\lib\amd64` plus `winv6.3\um\x64`), and `msvcrt.dll`'s import library
// exports the C89 names - `sin`, `cos`, `floor`, `pow`, `sqrtf` - and not the C99/C11 ones. Six
// symbols came back `LNK2019` when this block was `cfg`-ed out for the host: `roundf`, `round`,
// `truncf`, `log2f`, `log2`, `fma`. So the forwarders stay unconditional.
//
// They do not collide with `msvcrt.lib` on the host despite both being able to define `sin`: an
// import library contributes a member only for a symbol that is still undefined, and these
// definitions satisfy it first.
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
// `lyon_tessellation`'s stroke-join code calls this one directly; it is the only trig entry point
// the tessellator needs that the earlier list did not already carry.
libm_shim!(acosf, libm::acosf, (x: f32) -> f32);
libm_shim!(atanf, libm::atanf, (x: f32) -> f32);
libm_shim!(sqrtf, libm::sqrtf, (x: f32) -> f32);
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
// Added because the host link of `host_m1` asked for it and the f32 `log2f` above did not cover
// it: `zopfli`'s entropy calculation is `f64`. Same story as the rest - `msvcrt.lib` has no
// `log2`.
libm_shim!(log2, libm::log2, (x: f64) -> f64);

// ===========================================================================
// CRT-free DLL entry point
//
// Same requirement as wp81probe.dll: WP8.1 native DLLs must not import KERNEL32.dll, so the link
// uses /NODEFAULTLIB and the entry point comes from here.
// ===========================================================================

// Both of the following are ARM-only, and for the same reason as `libm_shim!`: they exist because
// the phone's link is `/NODEFAULTLIB` and supplies no CRT. On a host link the CRT supplies all
// three - `_fltused` and `_DllMainCRTStartup` are *defined* by it, so emitting them a second time
// is a duplicate-symbol error. `DllMain` itself is kept unguarded: it is a plain function with a
// name no CRT defines, and `_DllMainCRTStartup` above is its only caller.
#[cfg(target_arch = "arm")]
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

#[cfg(target_arch = "arm")]
#[no_mangle]
pub unsafe extern "system" fn _DllMainCRTStartup(
    module: *mut c_void,
    reason: u32,
    reserved: *mut c_void,
) -> i32 {
    DllMain(module, reason, reserved)
}
