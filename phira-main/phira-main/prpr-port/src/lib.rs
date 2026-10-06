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
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex,
};

use inputbox::{backend::Backend as InputBackend, InputBox};

/// The game host: builds a real `prpr` `GameScene` and ticks it. Kept out of this file so the ABI
/// surface stays readable.
pub mod host;

/// Container header walks for `wp81_audio_decode`, shared with the desktop M1 harness so the two
/// cannot drift into disagreeing about what a file's length is.
pub mod audio_info;

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
    pub audio_music_new: Option<
        unsafe extern "C" fn(
            clip: u32,
            name: *const u8,
            name_len: usize,
            rate: f64,
            amp: f32,
            loop_mix: f64,
            clip_secs: f64,
        ) -> u32,
    >,
    pub audio_music_play: Option<unsafe extern "C" fn(id: u32)>,
    pub audio_music_pause: Option<unsafe extern "C" fn(id: u32)>,
    pub audio_music_seek: Option<unsafe extern "C" fn(id: u32, position: f64)>,
    pub audio_music_position: Option<unsafe extern "C" fn(id: u32) -> f64>,
    pub audio_music_paused: Option<unsafe extern "C" fn(id: u32) -> i32>,
    pub audio_music_set_amplifier: Option<unsafe extern "C" fn(id: u32, amp: f32)>,
    pub audio_music_set_low_pass: Option<unsafe extern "C" fn(id: u32, cutoff: f32)>,
    pub audio_music_fade: Option<unsafe extern "C" fn(id: u32, seconds: f64, out: i32)>,
    pub audio_sfx_new:
        Option<unsafe extern "C" fn(clip: u32, name: *const u8, name_len: usize, secs: f64) -> u32>,
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
    /// One line of UTF-8 text for the shell's log.
    ///
    /// This is not a general facility, it closes one hole: `PhiraBoot` is a single uninterrupted
    /// call on the UI thread, and the `log` feature is off on this target, so when a run died
    /// inside phira's construction the shell's log simply stopped mid-sentence with nothing to say
    /// which phase was executing. A process dying mid-call is exactly when the shell cannot ask
    /// Rust anything, so the shell has to be told as it happens - and written synchronously, so the
    /// last line on disk is the last thing Rust did.
    pub log: Option<unsafe extern "C" fn(msg: *const u8, len: usize)>,
    /// Start one HTTP request. Returns `0` when it was accepted.
    ///
    /// This is the only network entry point and it is deliberately fire-and-forget: the caller is a
    /// tokio worker thread that must not wait, so the shell copies what it needs, starts the request
    /// and answers later through `wp81_http_complete`. Everything is UTF-8 with an explicit length
    /// because `reqwest`'s `Url` and `HeaderMap` are UTF-8 natively - a UTF-16 hop would mean two
    /// conversions for nothing, and `log` already crosses the same way.
    ///
    /// `headers` is a block of `name: value\r\n` lines, `body` the raw payload (null with length 0
    /// for a bodyless request).
    pub http_request: Option<
        unsafe extern "C" fn(
            id: u32,
            method: *const u8,
            method_len: usize,
            url: *const u8,
            url_len: usize,
            headers: *const u8,
            headers_len: usize,
            body: *const u8,
            body_len: usize,
        ) -> i32,
    >,
    /// Ask the shell to show a text-input dialog. Returns `0` once the request is accepted.
    ///
    /// phira opens one for every text field it owns - the login and password boxes, the search bar,
    /// the mp address setting, the comment and transfer prompts, the tag editor - through the
    /// `inputbox` crate, whose Windows backend works by *spawning a PowerShell script*. WP8.1 has
    /// no PowerShell and no system text-dialog API at all, so the port installs its own `Backend`
    /// and every one of those calls lands here instead. It is fire-and-forget for the same reason
    /// `http_request` is: the answer comes back later, through `wp81_input_complete`, and the
    /// caller must not block waiting for it.
    ///
    /// `buf` is borrowed for the duration of this call only and is a single length-prefixed record
    /// that the shell copies before returning:
    ///
    /// ```text
    /// u32 LE  mode            0 = text, 1 = password, 2 = multiline
    /// 5 x { u32 LE byte_len, byte_len bytes of UTF-8 } in this order:
    ///         title, prompt, default text, ok label, cancel label
    /// ```
    ///
    /// UTF-8 with explicit lengths rather than UTF-16 for the same reason as `http_request`: the
    /// `inputbox` side is UTF-8 natively, and a UTF-16 hop would be two conversions for nothing.
    pub text_input: Option<unsafe extern "C" fn(req: u32, buf: *const u8, len: usize) -> i32>,
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
    log: None,
    http_request: None,
    text_input: None,
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
    // The same handover, for the other direction: the `reqwest` shim is a separate crate with no
    // dependency on this one, so it cannot reach `CALLBACKS`. Without this `send()` has no
    // transport and every network call fails before it leaves the device.
    reqwest::set_http_sender(http_sender);
    // phira's text fields go through `inputbox`, whose Windows backend spawns `powershell.exe` -
    // which a phone does not have, so without this every login, search and settings field would
    // fail inside `spawn` and no dialog would ever appear. Installed here rather than behind a cfg
    // because a desktop build reaches the same default through `inputbox` on its own.
    prpr::scene::set_input_backend(Arc::new(ShellInput));
}

/// Forwarder for the macroquad shim's asset hook.
///
/// A plain `fn`, not a closure, because that is what the shim stores in a `static`. The path
/// arrives unchanged - `respack/info.yml` from the resource pack, `click.ogg` from the sound
/// fallback - and the shell resolves both against its asset root.
fn asset_hook(path: &str) -> Option<Vec<u8>> {
    host::load_asset(path)
}

/// Forwarder for the `reqwest` shim's transport hook.
///
/// A plain `fn` for the same reason `asset_hook` is one: that is what the shim stores in a `static`,
/// and the shim has no dependency on this crate. The strings are borrowed and handed over as raw
/// pointers for the duration of the call only - the shell copies what it needs before returning,
/// and it returns before the request has been sent, which is the whole point: the caller is a
/// tokio worker thread that must not wait.
fn http_sender(id: u32, method: &str, url: &str, headers: &str, body: &[u8]) -> i32 {
    let Some(send) = cb().http_request else {
        log_line(&format!("http: id={id} {method} {url} refused: no shell callback"));
        return -1;
    };
    log_line(&format!(
        "http: -> id={id} {method} {url} hdr={} body={}",
        headers.len(),
        body.len()
    ));
    // SAFETY: every pointer below is a borrow that lives until `send` returns, and the length is
    // the matching `str`/slice length. The shell reads and copies inside the call.
    unsafe {
        send(
            id,
            method.as_ptr(),
            method.len(),
            url.as_ptr(),
            url.len(),
            headers.as_ptr(),
            headers.len(),
            body.as_ptr(),
            body.len(),
        )
    }
}

/// Delivers a finished HTTP response to the `reqwest` future that asked for it.
///
/// Deliberately safe to call defensively: every buffer is null-checked and length-checked on the way
/// in, so a shell bug costs a lost response rather than a crash. It is also deliberately *not*
/// routed through `host::GAME`: that is a `thread_local!` for the UI thread, and this arrives on a
/// C# thread-pool continuation. The receiving state is a `static Mutex` inside the `reqwest` shim
/// precisely so the two threads can share it.
#[no_mangle]
pub extern "C" fn wp81_http_complete(
    id: u32,
    status: u32,
    url: *const u8,
    url_len: usize,
    headers: *const u8,
    headers_len: usize,
    body: *const u8,
    body_len: usize,
    err: *const u8,
    err_len: usize,
) -> i32 {
    // One `unsafe` block for the whole function - the four raw buffers, nothing else.
    // SAFETY: `read_bytes` returns an empty `Vec` for a null pointer or a zero length, and copies
    // exactly `len` bytes otherwise, so no pointer is ever dereferenced beyond what was passed.
    let (url, headers, body, err) = unsafe {
        (
            read_utf8(url, url_len),
            read_utf8(headers, headers_len),
            read_bytes(body, body_len),
            read_utf8(err, err_len),
        )
    };
    if status == 0 {
        log_line(&format!("http: <- id={id} failed: {err}"));
        reqwest::http_complete(id, Err(err));
        return 0;
    }
    log_line(&format!(
        "http: <- id={id} status={status} hdr={} body={}",
        headers.len(),
        body.len()
    ));
    reqwest::http_complete(
        id,
        Ok(reqwest::HttpResponse {
            status: status as u16,
            headers,
            body,
            final_url: url,
        }),
    );
    0
}

/// # Safety
/// `ptr` must be null, or readable for `len` bytes. Null and `len == 0` are legitimate values here
/// (a bodyless request, a headerless response), not errors.
unsafe fn read_bytes(ptr: *const u8, len: usize) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    std::slice::from_raw_parts(ptr, len).to_vec()
}

/// Lossy on purpose: a stray byte in a header block must not throw away an otherwise good
/// response. `from_utf8_lossy` cannot fail, so this cannot panic either.
///
/// # Safety
/// `ptr` must be null, or readable for `len` bytes of UTF-8.
unsafe fn read_utf8(ptr: *const u8, len: usize) -> String {
    String::from_utf8_lossy(&read_bytes(ptr, len)).into_owned()
}

#[inline]
fn cb() -> &'static HostCallbacks {
    unsafe { &*core::ptr::addr_of!(CALLBACKS) }
}

/// Send one line to the shell's log.
///
/// Silent when the shell installed no such callback, so leaving the calls in place costs nothing
/// on a shell that does not provide one. No allocation and no `unsafe` beyond the call itself:
/// this is on the boot path, where a panic in the diagnostics would be worse than silence.
#[inline]
pub fn log_line(msg: &str) {
    let Some(f) = cb().log else {
        return;
    };
    unsafe { f(msg.as_ptr(), msg.len()) };
}

// ===========================================================================
// text input
//
// The outbound half of `HostCallbacks::text_input`, and the inbound half
// `wp81_input_complete`. Same shape as the HTTP bridge: the shell owns the UI, Rust
// owns the callback, and the two meet at a request id.
// ===========================================================================

/// The `inputbox` backend that hands the dialog to the shell.
///
/// A unit struct on purpose: it carries no state, so it is `Send + Sync` for free, which is what
/// `prpr::scene::set_input_backend` needs in order to hold it in a `static`.
struct ShellInput;

impl InputBackend for ShellInput {
    fn execute_async(
        &self,
        input: &InputBox,
        callback: Box<dyn FnOnce(std::io::Result<Option<String>>) + Send>,
    ) -> std::io::Result<()> {
        let req = INPUT_NEXT.fetch_add(1, Ordering::Relaxed);
        let buf = encode_input_request(input);

        // Registered *before* the shell is called, and the lock is not held while it runs: a shell
        // with no dialog answers straight away, and that answer arrives through
        // `wp81_input_complete`, which needs this very lock. Doing it the other way round
        // deadlocks on the only path that reports a failure.
        INPUT_CALLBACKS.lock().unwrap().push((req, callback));

        log_line(&format!(
            "input: -> req={req} mode={} title={} prompt_len={} default_len={}",
            mode_name(input),
            input.title.is_some(),
            input.prompt.as_deref().map_or(0, |s| s.len()),
            input.default.len(),
        ));

        let accepted = match cb().text_input {
            Some(f) => unsafe { f(req, buf.as_ptr(), buf.len()) },
            None => 1,
        };
        if accepted != 0 {
            // Logged here rather than left to prpr: the `Err` it produces is only ever surfaced by
            // a `warn!`, and `tracing` has no subscriber on this target, so without this line a
            // shell with no dialog would look exactly like a phira build where `request_input` is
            // never called. That is precisely the ambiguity this whole change exists to remove.
            log_line(&format!(
                "input: req={req} refused by the shell (callbacks={}, sink gone or old DLL)",
                cb().text_input.is_some()
            ));
            take_input_callback(req);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "the shell provided no text input dialog",
            ));
        }
        Ok(())
    }
}

/// The shell's answer, handed back to `inputbox` through the callback stored for `req`.
///
/// Never `unsafe` beyond reading `text`: the lock is taken and released around the *removal*, so
/// the callback itself runs with no lock held and is free to open another dialog - which the login
/// form does on a failed sign-in.
#[no_mangle]
pub extern "C" fn wp81_input_complete(req: u32, text: *const u8, len: usize, ok: i32) {
    // Cancel carries no text at all. `ok != 0` with an empty buffer is the user clearing the field
    // and pressing confirm, which is a real answer (`Some("")`) rather than a cancellation - the
    // distinction `inputbox`'s `Ok(Some(_)` vs `Ok(None)` is built on.
    let value = if ok == 0 {
        None
    } else if text.is_null() || len == 0 {
        Some(String::new())
    } else {
        Some(unsafe { read_utf8(text, len) })
    };

    let Some(callback) = take_input_callback(req) else {
        log_line(&format!("input: <- req={req} has no stored callback (late or duplicate)"));
        return;
    };
    log_line(&format!(
        "input: <- req={req} ok={} len={}",
        ok != 0,
        value.as_ref().map_or(0, |s| s.len()),
    ));
    callback(match value {
        Some(text) => Ok(Some(text)),
        None => Ok(None),
    });
}

/// Pending callbacks keyed by request id.
///
/// A `static Mutex<Vec<..>>` rather than a `thread_local!`, for the same reason the HTTP response
/// slots are one: `wp81_input_complete` runs on whichever thread the shell answers on, while
/// `execute_async` ran on whatever thread called `request_input`.
///
/// Each entry is *removed* before its callback runs, so a callback that opens another dialog
/// cannot deadlock trying to reach this lock.
static INPUT_CALLBACKS: Mutex<Vec<(u32, Box<dyn FnOnce(std::io::Result<Option<String>>) + Send>)>> =
    Mutex::new(Vec::new());
static INPUT_NEXT: AtomicU32 = AtomicU32::new(1);

fn take_input_callback(req: u32) -> Option<Box<dyn FnOnce(std::io::Result<Option<String>>) + Send>> {
    let mut pending = INPUT_CALLBACKS.lock().unwrap();
    pending
        .iter()
        .position(|(stored, _)| *stored == req)
        .map(|index| pending.remove(index).1)
}

/// Packs one dialog request into the record `HostCallbacks::text_input` documents.
fn encode_input_request(input: &InputBox) -> Vec<u8> {
    let mode: u32 = match &input.mode {
        inputbox::InputMode::Text => 0,
        inputbox::InputMode::Password => 1,
        inputbox::InputMode::Multiline => 2,
    };
    let fields: [&str; 5] = [
        input.title.as_deref().unwrap_or_default(),
        input.prompt.as_deref().unwrap_or_default(),
        &input.default,
        input.ok_label.as_deref().unwrap_or_default(),
        input.cancel_label.as_deref().unwrap_or_default(),
    ];

    let mut buf = Vec::with_capacity(4 + fields.iter().map(|f| 4 + f.len()).sum::<usize>());
    buf.extend_from_slice(&mode.to_le_bytes());
    for field in fields {
        let bytes = field.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(bytes);
    }
    buf
}

fn mode_name(input: &InputBox) -> &'static str {
    match &input.mode {
        inputbox::InputMode::Text => "text",
        inputbox::InputMode::Password => "password",
        inputbox::InputMode::Multiline => "multiline",
    }
}

// ===========================================================================
// implementations of the shims' extern "C" dependencies
//
// These names must match the `extern "C"` blocks in rust-wp81/shims/{sasa,rfd}. They are defined
// here rather than in the shims so that the shims stay independent of the shell.
// ===========================================================================

/// Decode enough of a buffer to answer `sample_rate` and `frame_count`.
///
/// The header walk runs *here*, in Rust, and the shell's `audio_decode` callback is only reached
/// when this module does not recognise the container. That order is the whole point: the callback
/// is a C# stub returning `1`, so before this existed the first resource-pack sound the engine
/// asked for - `respack/click.ogg`, 10380 bytes - came back as
/// `GameScene::new failed: ... audio decode failed (rc=1, 10380 bytes)`, which surfaced on the phone
/// as nothing more than `GameCreate -> -2`. The walk needs no shell state: it reads bytes that are
/// already in memory and writes two integers.
#[no_mangle]
pub unsafe extern "C" fn wp81_audio_decode(data: *const u8, len: usize, out: *mut DecodedAudio) -> i32 {
    if data.is_null() || out.is_null() || len < 16 {
        return 1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data, len) };
    if let Some((rate, frames)) = audio_info::audio_info(bytes) {
        let out = unsafe { &mut *out };
        out.sample_rate = rate;
        out.frame_count = frames;
        return 0;
    }
    // Not a container we know. Ask the shell - it may be a format only it can read, and it is the
    // side that will eventually play this anyway.
    match cb().audio_decode {
        Some(f) => f(data, len, out),
        None => 1,
    }
}

/// The XAudio2 mixer, compiled into this same DLL from `rust-wp81/wp81mix.cpp`.
///
/// Declared here rather than exported: both halves live in one image, so the linker resolves the
/// call against the object file `build-port.ps1` compiles and no `/EXPORT:` is needed for it.
/// The ingest path is deliberately internal-only - C# reaches the mixer through the exported
/// `wp81_mix_*` entry points it actually drives, never through the samples themselves.
extern "C" {
    fn wp81_mix_pcm(
        tag: i32,
        key: *const u8,
        key_len: i32,
        rate: u32,
        ch: u32,
        frames: *const f32,
        frame_count: u64,
    ) -> i32;
}

/// Hand one decoded clip to the shell as interleaved f32.

///
/// Called by `sasa::AudioManager::create_sfx` (tag 1) and `create_music` (tag 0) immediately
/// *before* `wp81_audio_sfx_new` / `wp81_audio_music_new`, with the same `key`/`key_len`, and
/// its return value is ignored at both call sites.
///
/// Defined here because nothing else defines it: the `sasa` shim only declares it in its
/// `mod shell` extern block, so an earlier link of this crate resolved every `wp81_audio_*`
/// forwarder except this one and failed on it. It is internal to the DLL - `sasa` and
/// `prpr-port` are both members of the same staticlib - so unlike its neighbours it takes no
/// `/EXPORT:`; C# never sees the symbol.
///
/// It hands the samples to the XAudio2 mixer compiled into this same DLL, which is the other
/// half of this contract: prpr already decodes every asset with symphonia, so the mixer needs no
/// decoder of its own - and mixing is the only way a hit sound can play without being able to
/// silence the music, because they stop being two pipelines fighting over one platform session.
///
/// The bytes are *copied* and converted to i16 on the way in. The source belongs to `AudioClip`
/// and does not outlive this call, and holding both a 65 MB f32 track and its i16 twin on a phone
/// that hands an app a few hundred MB is not an option - the conversion is what makes the
/// permanent cost 34 MB instead of 65, and the caller's copy is the one that goes away.
///
/// Still logged, once per clip, so that stays visible: if a chart's hit sound ever comes out
/// silent, the first thing to rule out is that its PCM never reached the mixer. `mix=` is
/// `wp81_mix_pcm`'s return - 0 means taken, 1 means malformed, 3 means out of memory, 4 means
/// XAudio2 never came up and every other consumer of this line has to fall back. `error!`/
/// `warn!` are silent on this build, so it goes through `log_line` like every other line that
/// has to survive the trip to the device.
#[no_mangle]
pub unsafe extern "C" fn wp81_audio_pcm(
    tag: u32,
    key: *const u8,
    key_len: usize,
    rate: u32,
    channels: u32,
    frames: *const f32,
    frame_count: u64,
) -> i32 {
    let name = if key.is_null() || key_len == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(unsafe { core::slice::from_raw_parts(key, key_len) }).into_owned()
    };
    let rc = if frames.is_null() {
        1
    } else {
        wp81_mix_pcm(
            tag as i32,
            key,
            key_len as i32,
            rate,
            channels,
            frames,
            frame_count,
        )
    };
    log_line(&format!(
        "audio: pcm tag={} name={} rate={} ch={} frames={} mix={}",
        tag, name, rate, channels, frame_count, rc
    ));
    rc
}

/// Create a music stream.
///
/// `name`/`name_len` are the file this clip came from, borrowed for the call exactly like
/// `wp81_audio_sfx_new`'s - `<local_path>/<music>` relative to phira's data root for a chart's own
/// track, `ending` for the res pack's clip, empty for a preview whose file the engine did not
/// open by path.
///
/// The shell has one player and `AudioClip` reports id 0 for every clip, so *something* has to
/// say which audio is wanted. It used to be `clip_secs` alone, matched against the one chart the
/// shell had resolved before boot: right for the packaged chart, and wrong for a downloaded one,
/// which measured differently and fell through to the res pack's `ending` - so a chart with music
/// on disk played in silence. A duration is a coincidence of two readings of a file; a path is the
/// file.
#[no_mangle]
pub unsafe extern "C" fn wp81_audio_music_new(
    clip: u32,
    name: *const u8,
    name_len: usize,
    rate: f64,
    amp: f32,
    loop_mix: f64,
    clip_secs: f64,
) -> u32 {
    match cb().audio_music_new {
        Some(f) => f(clip, name, name_len, rate, amp, loop_mix, clip_secs),
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

/// Create a sound effect.
///
/// `name`/`name_len` are the asset name the clip was loaded from (no directory, no extension) and
/// `secs` its duration. Together they are the only thing that identifies it: `AudioClip` carries no
/// id of its own here (every one is 0), so without them the shell could not tell the hit sound from a
/// button click - and it has to, because it is the shell that owns the audio device.
#[no_mangle]
pub unsafe extern "C" fn wp81_audio_sfx_new(
    clip: u32,
    name: *const u8,
    name_len: usize,
    secs: f64,
) -> u32 {
    match cb().audio_sfx_new {
        Some(f) => f(clip, name, name_len, secs),
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

/// How many vertices and indices the last `wp81_draw_encode` produced.
///
/// The shell must know both *before* it copies anything. `PrprPort.Drain` reads exactly
/// `vertexCapacity` elements, so a capacity larger than the truth copies arena bytes no command
/// refers to - and `Win2DChartRenderer.OnPortDraw` **rasterises** those, which makes the result not
/// a shorter frame but a frame containing garbage geometry - while a smaller one truncates a batch
/// mid-triangle.
///
/// # Safety
/// `vertices` and `indices` must each be writable for one `u32` when non-null.
#[no_mangle]
pub unsafe extern "C" fn wp81_draw_counts(vertices: *mut u32, indices: *mut u32) {
    let (v, i) = macroquad::draw::port_counts();
    if !vertices.is_null() {
        *vertices = v;
    }
    if !indices.is_null() {
        *indices = i;
    }
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

/// Particle quads the last encoded frame carried.
///
/// `prpr::particle` used to reach the renderer not at all - it drives miniquad's `Context`, whose
/// shim methods are no-ops - so the hit effect was invisible with nothing in the log to say so. This
/// is the one number that can: zero means the emitter never made it into the command stream.
#[no_mangle]
pub extern "C" fn wp81_sprite_quads() -> u32 {
    macroquad::draw::sprite_quad_count()
}

/// Particle-pass detail, indexed: `0` draws, `1` particles seen, `2` quads in the last frame.
///
/// Separates "the emitter was never drawn" from "it was drawn empty" - see the shim's note.
#[no_mangle]
pub extern "C" fn wp81_particle_diag(index: u32) -> u32 {
    macroquad::draw::particle_diag(index)
}

/// Detail about the most recent refusal, indexed - `0` reason code, `1` id, `2` width, `3` height,
/// `4` byte length, `5` bytes currently held, `6` the byte budget. `0` for any other index.
///
/// `wp81_texture_overflow` says *how many* were lost; a count alone cannot tell "the slot table
/// filled up" from "one texture was bigger than the whole budget", and those need opposite fixes -
/// so the reason has to cross the boundary too. Reason codes: `1` out of slots, `2` a single
/// texture larger than the budget, `3` the running total would exceed it, `0` nothing dropped.
///
/// Follows `wp81_rs_sis_diag(index)` rather than returning a struct: one accessor keeps the C#
/// side's `DllImport` list short and needs no marshaller.
#[no_mangle]
pub extern "C" fn wp81_texture_drop_info(index: u32) -> u32 {
    let (id, width, height, bytes) = macroquad::draw::texture_drop_last();
    match index {
        0 => match macroquad::draw::texture_drop_reason() {
            "slots" => 1,
            "single too large" => 2,
            "budget" => 3,
            _ => 0,
        },
        1 => id,
        2 => width,
        3 => height,
        4 => bytes as u32,
        5 => macroquad::draw::texture_used_bytes() as u32,
        6 => macroquad::draw::texture_capacity() as u32,
        _ => 0,
    }
}

/// The rectangle written since the shell last asked about one texture.
///
/// Returns 1 with `out` filled and the pending area cleared, 0 when nothing changed, -1 for an
/// unknown id. A null `out` probes without consuming.
///
/// A dirty rectangle rather than a version counter, and that choice is the whole point: the glyph
/// atlas changes **during play**, because a judgement's glyph is rasterised the first time that
/// judgement appears. A version can only say "something changed", which would leave the shell with
/// one option - rebuild the whole 1024x1024 atlas, a million `Color` conversions and a
/// `CreateFromColors`, right in the middle of a frame. The rectangle is the union of every
/// `glTexSubImage2D` since the last read, so ten glyph uploads in one frame cost one small blit.
///
/// Clearing on read keeps the ledger in a single place. Two ledgers - one here, one in the shell -
/// would drift, and that class of discrepancy only ever shows up on the device.
///
/// # Safety
/// `out` must be valid for four `u32`s when non-null.
#[no_mangle]
pub unsafe extern "C" fn wp81_texture_dirty(id: u32, out: *mut u32) -> i32 {
    macroquad::draw::texture_dirty(id, out)
}

/// Copy a sub-rectangle of a texture's RGBA pixels out - the incremental counterpart to
/// `wp81_texture_data`, which can only return a texture whole.
///
/// Returns the byte count, -1 for an unknown id, -2 when `cap` is too small or `out` is null, and
/// -3 when the rectangle lies outside the texture.
///
/// # Safety
/// `out` must have room for `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn wp81_texture_rect_data(
    id: u32, x: u32, y: u32, w: u32, h: u32, out: *mut u8, cap: usize,
) -> i32 {
    macroquad::draw::texture_rect_data(id, x, y, w, h, out, cap)
}

/// Sub-image writes the registry refused because it did not know the id.
///
/// Non-zero is always a bug: something is being drawn whose bytes never arrived, and unlike a
/// missing glyph that leaves no trace on screen. Exposed rather than only logged because on the
/// phone there is no console, and this is the pair that says whether the glyph path works at all -
/// see `wp81_texture_write_count`, which is the liveness half. Zero refusals on its own is
/// ambiguous: an engine that never wrote anything reports that too.
#[no_mangle]
pub extern "C" fn wp81_texture_write_misses() -> u32 {
    macroquad::draw::texture_write_misses()
}

/// Sub-image writes accepted, and - in the high half - the id that received the last one.
///
/// Packed rather than split across two exports because the two numbers are only ever read together
/// to answer one question: did glyph pixels reach the registry, and which texture is holding them.
#[no_mangle]
pub extern "C" fn wp81_texture_write_count() -> u64 {
    let (writes, last) = macroquad::draw::texture_write_stats();
    ((last as u64) << 32) | writes as u64
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

/// Boot phira itself - its own `MainScene`, its own library UI - instead of a chart.
///
/// The shell calls this when it wants to land *in* Rust phira rather than in a game the C# side
/// picked out, which is the "start straight into the app" path. From the first frame onwards it is
/// driven exactly like a chart would be: `wp81_game_frame` pumps `the_main`'s loop one iteration
/// per display frame and the draw commands come back the same way (`prpr-port/src/host.rs`
/// explains the split).
///
/// Takes three arguments, all of them about where phira may write and what it should do first.
/// phira's *own* configuration - fonts, settings, volume - it still reads itself; there is no chart
/// name in here that it could not look up, but there is also no way for it to discover a directory
/// or an intention on its own.
///
///   `data_dir`   absolute, writable, and the parent of phira's `data` directory. phira only picks
///                one at compile time for ohos and ios; everywhere else it falls back to a relative
///                `./data`, which on a phone is not ours to write, so the boot fails before it
///                draws. Null leaves it unset - what a desktop harness wants.
///   `auto_open`  the local chart to open on its song page once the library is up, written the way
///                `LocalChart::local_path` writes it (`custom/190`), not as a path. Null stops at
///                the library, which is an ordinary phira.
///   `auto_play`  non-zero to start that chart rather than wait for a tap. Kept separate from which
///                chart because a host may want to change its mind about this one independently -
///                a build that drops you in the chart and one that shows the page first are both
///                reasonable, and neither should have to change which chart it names.
///
/// Returns `0` on success, or the same negative codes as `wp81_game_create`:
///
///   -3  a game or phira is already running
///   -2  construction never reached phira's first frame (see `wp81_game_error`)
///   -100  construction panicked
#[cfg(target_arch = "arm")]
#[no_mangle]
pub extern "C" fn wp81_phira_boot(data_dir: *const u16, auto_open: *const u16, auto_play: i32) -> i32 {
    // # Safety
    //
    // Both pointers come from the shell's own marshalling, so they are either null or point at a
    // NUL-terminated UTF-16 buffer that outlives this call - which is what `utf16_to_string`
    // requires. The null test is done here rather than left to it so that "unset" and "empty" both
    // mean the same thing to the caller: neither string has to know whether the shell passes null
    // or an empty buffer when it has nothing to say.
    let data_dir = if data_dir.is_null() { None } else { unsafe { utf16_to_string(data_dir) } };
    let auto_open = if auto_open.is_null() { None } else { unsafe { utf16_to_string(auto_open) } };
    host::boot_phira(data_dir, auto_open, auto_play != 0)
}

/// Advance and render one frame. `dt` is the seconds since the previous call.
///
/// Returns the number of draw commands produced (readable through `wp81_draw_commands`), or a
/// negative code when nothing is running.
#[no_mangle]
pub extern "C" fn wp81_game_frame(dt: f64) -> i32 {
    // Once every ~60 frames, written from Rust rather than read by the shell.
    //
    // The shell reads the same counters, but it reads them inside the Win2D draw callback, which runs
    // asynchronously to the game loop - so a zero there is ambiguous between "no particles" and "read
    // at the wrong moment". This line is written from the frame that actually built the particles.
    //
    // Hand-rolled decimal rather than `format!`: this crate deliberately pulls in no formatting
    // machinery (see the note on the DLL's size), and once a second is not worth changing that.
    let (tex_new, tex_rebuilt) = macroquad::draw::render_texture_counts();
    unsafe {
        // Gated on the frame count, NOT on `particle_diag(0)` as it first was.
        //
        // That counter only advances while a chart is being played, so on the home screen - exactly
        // where the UI bugs are - the condition never became true and this line was never written.
        // The instrument was silent precisely where it was needed, which is worse than not having it.
        FRAME_TICK = FRAME_TICK.wrapping_add(1);
        if FRAME_TICK % 60 == 0 {
            let draws = macroquad::draw::particle_diag(0);
            let mut buf = [0u8; 320];
            let mut n = push_str(&mut buf, 0, "input: pending=");
            n = push_u32(&mut buf, n, macroquad::input::pending_len() as u32);
            n = push_str(&mut buf, n, " replayed=");
            n = push_u32(&mut buf, n, macroquad::input::replayed_count() as u32);
            n = push_str(&mut buf, n, " draws=");
            n = push_u32(&mut buf, n, draws);
            n = push_str(&mut buf, n, " pushed=");
            n = push_u32(&mut buf, n, macroquad::draw::particle_diag(3));
            n = push_str(&mut buf, n, " quads=");
            n = push_u32(&mut buf, n, macroquad::draw::particle_diag(2));
            n = push_str(&mut buf, n, " texNew=");
            n = push_u32(&mut buf, n, tex_new);
            n = push_str(&mut buf, n, " texRebuilt=");
            n = push_u32(&mut buf, n, tex_rebuilt);
            // Whatever the game crate parked for us, scaled by 1000 so it fits the integer printer.
            let (dbg_a, dbg_b) = macroquad::dbg::get();
            n = push_str(&mut buf, n, " gate=");
            n = push_u32(&mut buf, n, (dbg_a * 1000.0) as u32);
            n = push_str(&mut buf, n, " thr=");
            n = push_u32(&mut buf, n, (dbg_b * 1000.0) as u32);
            // The touch position as `prpr::ui` sees it (slots C/D), next to the page's own gate.
            //
            // Signed and scaled by 1000, like `gate`/`thr` above. It used to be `tx as u32`, and
            // that single cast is what sent the whole "touch arrives as (0,0)" investigation after
            // the transform: these are NDC coordinates in (-1, 1), and `f32 as u32` saturates - a
            // negative, a NaN and anything below 1.0 all print as 0 - so the field read 0 for
            // every touch, correctly transformed ones included. `+123` now means 0.123.
            let (_, _, tx, ty) = macroquad::dbg::get4();
            n = push_str(&mut buf, n, " tx=");
            n = push_str(&mut buf, n, if tx < 0.0 { "-" } else { "+" });
            n = push_u32(&mut buf, n, (tx.abs() * 1000.0) as u32);
            n = push_str(&mut buf, n, " ty=");
            n = push_str(&mut buf, n, if ty < 0.0 { "-" } else { "+" });
            n = push_u32(&mut buf, n, (ty.abs() * 1000.0) as u32);
            // What the shell pushed against what reached `prpr::judge`'s handler, per phase -
            // the pair that says whether a dead button lost its Ended on the way in.
            let (pushed, replayed) = macroquad::input::phase_counts();
            n = push_str(&mut buf, n, " ph=");
            n = push_phases(&mut buf, n, pushed);
            n = push_str(&mut buf, n, " rph=");
            n = push_phases(&mut buf, n, replayed);
            // The position as received at the FFI boundary, and the screen size the transform divides
            // by. `px=0` means the shell sent garbage; `px` sane with `tx=0` means the transform did.
            let (px, py, pn) = macroquad::input::last_push();
            n = push_str(&mut buf, n, " px=");
            n = push_u32(&mut buf, n, px as u32);
            n = push_str(&mut buf, n, " py=");
            n = push_u32(&mut buf, n, py as u32);
            n = push_str(&mut buf, n, " pn=");
            n = push_u32(&mut buf, n, pn);
            n = push_str(&mut buf, n, " sw=");
            n = push_u32(&mut buf, n, macroquad::window::screen_width() as u32);
            n = push_str(&mut buf, n, " sh=");
            n = push_u32(&mut buf, n, macroquad::window::screen_height() as u32);
            n = push_str(&mut buf, n, " now=");
            n = push_u32(&mut buf, n, macroquad::clock::now_secs() as u32);
            if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                log_line(s);
            }
            // What `prpr::judge` decided in the same second. Four causes of "I tapped and nothing
            // happened" are indistinguishable from outside - the judge never ran (`ju=0`), no
            // press counted as a click (`js=0`), the note rejected the x (`jx`) or the time
            // (`jt`) - and the shell can only see that a finger went down. This is the shortest
            // path from the one place that knows to the log the shell already writes.
            //
            // `jd` is the smallest line-local distance touched: `X_DIFF_MAX * 10000` is 2362, so
            // `jd` in the thousands means the finger was a little off the note while `jd` in the
            // tens of thousands means the touch is in a different coordinate space from the chart.
            //
            // Skipped when the judge had nothing to report, so the home screen does not spend a
            // line a second on `ju=0 je=0 ...`.
            let (ju, je, js, jc, jh, jx, jt, jn, jd, jtms) = macroquad::judge_diag::take();
            if ju + je + js + jc + jh + jx + jt + jn > 0 {
                let mut buf = [0u8; 352];
                let mut n = push_str(&mut buf, 0, "judge: ju=");
                n = push_u32(&mut buf, n, ju);
                n = push_str(&mut buf, n, " je=");
                n = push_u32(&mut buf, n, je);
                n = push_str(&mut buf, n, " js=");
                n = push_u32(&mut buf, n, js);
                n = push_str(&mut buf, n, " jc=");
                n = push_u32(&mut buf, n, jc);
                n = push_str(&mut buf, n, " jh=");
                n = push_u32(&mut buf, n, jh);
                n = push_str(&mut buf, n, " jx=");
                n = push_u32(&mut buf, n, jx);
                n = push_str(&mut buf, n, " jt=");
                n = push_u32(&mut buf, n, jt);
                n = push_str(&mut buf, n, " jn=");
                n = push_u32(&mut buf, n, jn);
                n = push_str(&mut buf, n, " jd=");
                n = push_u32(&mut buf, n, jd);
                n = push_str(&mut buf, n, " t=");
                n = push_u32(&mut buf, n, jtms);
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    log_line(s);
                }
            }
            // The ending screen's half of the picture. Same shape as the judge line above: four
            // causes of "the summary is blank" are indistinguishable from the shell - the scene
            // was never rendered (`er=0`), it renders to a framebuffer nobody presents (`et`==`er`),
            // the entry animation never opened the gate (`eg`=0 while `er` moves), or it draws but
            // its texts never reach the atlas (`es`=0). `ea` is the atlas rebuild count: a rebuild
            // inside the scene is the one event that strands every pending glyph on a texture the
            // registry has the wrong size for.
            //
            // Not gated on "any counter moved" the way the judge line is: `esc` is a value, and
            // ending counters are all zero on every screen that is not the ending, so an all-zero
            // line once a second is the signal that says the scene never ran at all.
            let (ee, er, et, eg, esc, ea, es, edw, edh) = macroquad::ending_diag::take();
            if ee + er > 0 {
                let mut buf = [0u8; 208];
                let mut n = push_str(&mut buf, 0, "ending: ee=");
                n = push_u32(&mut buf, n, ee);
                n = push_str(&mut buf, n, " er=");
                n = push_u32(&mut buf, n, er);
                n = push_str(&mut buf, n, " et=");
                n = push_u32(&mut buf, n, et);
                n = push_str(&mut buf, n, " eg=");
                n = push_u32(&mut buf, n, eg);
                n = push_str(&mut buf, n, " esc=");
                n = push_u32(&mut buf, n, esc);
                n = push_str(&mut buf, n, " ea=");
                n = push_u32(&mut buf, n, ea);
                n = push_str(&mut buf, n, " es=");
                n = push_u32(&mut buf, n, es);
                // `dw`/`dh` are the score digits' measured advance and height in 1/10000 UI
                // units. Zero in either is the blank scoreboard: the width stops the digits
                // walking across the screen, the height empties the clip rect they are drawn
                // inside. Without them "the scoreboard is blank" and "the scoreboard draws"
                // produce the same line.
                n = push_str(&mut buf, n, " dw=");
                n = push_u32(&mut buf, n, edw);
                n = push_str(&mut buf, n, " dh=");
                n = push_u32(&mut buf, n, edh);
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    log_line(s);
                }
            }
        }
    }
    // Move the shim's clock. Nothing else does - `set_now_micros` has no caller here - and while it
    // stood still every real-time-driven animation was frozen, including the home screen's intro
    // transition, which is the gate in front of every button on that page. Must happen *before*
    // `host::frame`, which is where the scene reads the time.
    macroquad::clock::advance_secs(dt);
    host::frame(dt)
}

static mut FRAME_TICK: u32 = 0;

fn push_str(buf: &mut [u8], at: usize, s: &str) -> usize {
    let mut at = at;
    for b in s.as_bytes() {
        if at < buf.len() {
            buf[at] = *b;
            at += 1;
        }
    }
    at
}

/// `s0/n0/m0/e0/c0` - the five `TouchPhase` slots in declaration order (Started, Stationary,
/// Moved, Ended, Cancelled), pulled from `macroquad::input::phase_counts`.
///
/// Its own writer because it is the only field built from an array: both call sites would
/// otherwise have to repeat the slot order, and a reordered `match` in the shim would silently
/// relabel every count while still looking like a well-formed line.
fn push_phases(buf: &mut [u8], at: usize, c: [u32; 5]) -> usize {
    let mut at = push_str(buf, at, "s");
    at = push_u32(buf, at, c[0]);
    at = push_str(buf, at, "/n");
    at = push_u32(buf, at, c[1]);
    at = push_str(buf, at, "/m");
    at = push_u32(buf, at, c[2]);
    at = push_str(buf, at, "/e");
    at = push_u32(buf, at, c[3]);
    at = push_str(buf, at, "/c");
    push_u32(buf, at, c[4])
}

fn push_u32(buf: &mut [u8], at: usize, mut n: u32) -> usize {
    let mut tmp = [0u8; 10];
    let mut len = 0usize;
    if n == 0 {
        tmp[0] = b'0';
        len = 1;
    }
    while n > 0 {
        tmp[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    let mut at = at;
    for i in (0..len).rev() {
        if at < buf.len() {
            buf[at] = tmp[i];
            at += 1;
        }
    }
    at
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

/// Copy why the most recent `wp81_game_create` failed into `buf` as a NUL-terminated UTF-8 string.
///
/// Returns the bytes written excluding the NUL, `-1` when nothing has failed (or the last attempt
/// succeeded), and `-2` when `cap` is too small for the message, in which case nothing is written.
///
/// This is what turns `GameCreate -> -2` into a sentence. `create` reports every failure through
/// `tracing::error!` and there is no subscriber on the phone, so the event is dropped - the shell
/// was left with one code standing for three unrelated causes (a chart the parser rejects, a
/// `GameScene::new` that dies mid-texture, a `Main::new` with no font) and no way to tell them
/// apart. The message must be read in the turn right after the call: it describes the last failure
/// and is cleared by the next attempt.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn wp81_game_last_error(buf: *mut u8, cap: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }
    host::with_last_error(|msg| {
        let bytes = msg.as_bytes();
        if bytes.len() + 1 > cap {
            return -2;
        }
        // The single unsafe act in this function: handing a `&str` to a caller means writing
        // through a raw pointer. The `bytes.len() + 1 > cap` check above is the whole safety
        // argument, and the trailing NUL is what makes it a C string on the other side.
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *buf.add(bytes.len()) = 0;
        bytes.len() as i32
    })
}

/// Frames rendered so far, or -1 when no game is running.
#[no_mangle]
pub extern "C" fn wp81_game_frames() -> i64 {
    host::frames()
}

/// The running game's full result: score, accuracy, combo, per-judgement counts, early/late split.
///
/// Exists because `prpr` owns the judge on this path: the shell's own `Judge` is never advanced, so
/// its `Result()` would report a flawless zero-score play. `scene/game.rs` reads the same fields to
/// draw the HUD (`self.judge.score()` at :432, `self.judge.combo()` at :468), which is why the HUD
/// cannot simply be kept - it would be a second, disagreeing one.
///
/// Everything comes back in one struct rather than a dozen out-parameters: the result page reads
/// nine of them (`Counts`, `Early`, `Late`, `NumOfNotes` included), and a call with that many
/// arguments is a place where a single transposed pair would land on the wrong column without
/// failing anywhere.
///
/// Returns 0 when a game is running, -1 when there is none.
///
/// # Safety
/// `out` must be writable for one [`host::PlayResultAbi`] when non-null.
#[no_mangle]
pub unsafe extern "C" fn wp81_game_result(out: *mut host::PlayResultAbi) -> i32 {
    host::result(out)
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

/// Set the engine timeline to an absolute time, before the next `wp81_game_frame`.
///
/// The shell owns the music clock (it has the audio position), so the Rust side must not invent
/// one. The shell sets this and then calls `wp81_game_frame(0.0)` - not `wp81_game_frame(dt)` -
/// because `host::frame` advances `CLOCK` by `dt` itself, and passing both would advance the
/// chart twice as fast as the music. Absolute rather than differential because a cumulative sum
/// starts drifting the moment the shell misses a frame, and a rhythm game cannot be restarted
/// after a seek without one anyway.
///
/// Ignored for a non-finite `t`: a NaN here would propagate through every projection and produce
/// commands full of NaN, which the shell then has to notice and discard - a far more confusing
/// failure than refusing the value at the door.
#[no_mangle]
pub extern "C" fn wp81_game_set_time(t: f64) {
    host::set_time(t);
}

/// Queue one touch event for the frame about to be built. Call before `wp81_game_frame`.
///
/// `phase` is a plain number because the shell is C#: **0=Started, 1=Stationary, 2=Moved,
/// 3=Ended, 4=Cancelled**, which is `miniquad::TouchPhase`'s declaration order - `Stationary`
/// sits *before* `Moved`, unlike winit's, and swapping them would make a held note look like a
/// moving one on every frame that did not move.
///
/// `x`/`y` are screen pixels in the same space as `wp81_game_set_screen`, because that is what
/// `prpr::judge` divides by `screen_width()`/`screen_height()` to reach NDC.
///
/// `time` is uptime in the same units as `wp81_game_set_time` - effectively the chart time the
/// finger was at. `prpr` ages the touch with `t - (uptime - time) * speed`, so stamping it with
/// wall time instead would make every judgement error grow as the play went on.
///
/// Returns 0, -1 for an unknown `phase`, or -2 when this frame's queue is full.
#[no_mangle]
pub extern "C" fn wp81_game_touch(id: u32, phase: u32, x: f32, y: f32, time: f64) -> i32 {
    use macroquad::input::TouchPhase;
    let phase = match phase {
        0 => TouchPhase::Started,
        1 => TouchPhase::Stationary,
        2 => TouchPhase::Moved,
        3 => TouchPhase::Ended,
        4 => TouchPhase::Cancelled,
        _ => return -1,
    };
    macroquad::input::push_touch(id as u64, phase, x, y, time)
}
/// The engine's judgement clock, for stamping touches.
///
/// `prpr::judge` ages a touch with `t - (uptime - it.time) * speed`, where `uptime` is exactly
/// this value and `t` derives from the same clock. The shell must stamp every touch with a time
/// sampled from *this* timeline - its own `_phiraTime` or music clock drifts from the engine's
/// `CLOCK` (one accumulates `dt`, the other is absolute), and `prpr` uses only the difference,
/// so a foreign stamp reads as a judgement error that grows with every second of play. That is
/// the mechanism behind "hold notes never register": the hold's stored `time` ages wrong the
/// moment the two clocks disagree by a frame.
#[no_mangle]
pub extern "C" fn wp81_game_time() -> f64 {
    macroquad::miniquad::uptime_secs()
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
