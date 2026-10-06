//! A `sasa` shim for Windows Phone 8.1.
//!
//! # Why
//!
//! `sasa` is the audio engine behind `prpr`. Its backends are `cpal` (desktop WASAPI), `oboe`
//! (Android) and `ohos-audio-sys` (OpenHarmony) - there is no WinRT or XAudio2 backend, and the
//! crate's own mixer runs on a thread it starts itself. So the mixing cannot happen in Rust on this
//! platform without writing an audio backend from scratch.
//!
//! The C# side already has a working audio path (it plays the chart music and the hit sounds in the
//! shipping app), so this shim keeps the **API and the timing surface** that `prpr` depends on and
//! routes playback there.
//!
//! # What is real
//!
//! `Music::position()`, `paused()`, `length()` and `AudioClip::sample_rate()`/`frames()` are load
//! bearing: `prpr` drives its clock from the music position and judges notes against it. Those must
//! report the truth, so they are backed by values the shell supplies rather than being stubbed.
//!
//! # Decoding
//!
//! `AudioClip::new` hands the compressed bytes to the shell over FFI, which decodes them (the C#
//! side already decodes the chart's audio) and returns the sample rate and frame count. The frames
//! themselves stay on the C# side; `frames()` returns an empty slice and is only used by `prpr` to
//! measure length, which is computed from the frame count instead.

#![allow(non_snake_case, unused_variables, dead_code)]

pub use anyhow::Result;

/// One stereo sample. Matches upstream `sasa::Frame`, which is a tuple struct.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Frame(pub f32, pub f32);

impl Frame {
    pub fn avg(&self) -> f32 {
        (self.0 + self.1) / 2.0
    }
    pub fn interpolate(&self, other: &Self, f: f32) -> Self {
        Frame(
            self.0 + (other.0 - self.0) * f,
            self.1 + (other.1 - self.1) * f,
        )
    }
}

impl core::ops::Add for Frame {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Frame(self.0 + rhs.0, self.1 + rhs.1)
    }
}

impl core::ops::Mul<f32> for Frame {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self::Output {
        Frame(self.0 * rhs, self.1 * rhs)
    }
}

// ===========================================================================
// the shell bridge
//
// Declared here, implemented by the C# side. Nothing in this crate links until the final
// staticlib/DLL, so an rlib build of this crate succeeds without them.
// ===========================================================================
mod shell {
    #[repr(C)]
    pub struct DecodedInfo {
        pub sample_rate: u32,
        pub frame_count: u64,
    }

    extern "C" {
        /// Decode a compressed audio buffer. Returns 0 on success.
        ///
        /// The frames are not copied back: only the rate and count are needed on the Rust side, and
        /// the audio itself is played by the shell from the same buffer.
        pub fn wp81_audio_decode(data: *const u8, len: usize, out: *mut DecodedInfo) -> i32;

        /// Create a music stream. `rate` is `playback_rate`, `amp` the amplifier, `clip_secs` the
        /// clip's duration in seconds. `name`/`name_len` say *which* file this is, exactly as
        /// `wp81_audio_sfx_new` does: `clip_id` is 0 for every clip, and before this arrived the
        /// shell had to guess the chart's track from its duration alone - which is right for the
        /// one chart packaged with the app and wrong for every other one. Kept in step with
        /// `prpr_port::wp81_audio_music_new`.
        pub fn wp81_audio_music_new(
            clip_id: u32,
            name: *const u8,
            name_len: usize,
            rate: f64,
            amp: f32,
            loop_mix_time: f64,
            clip_secs: f64,
        ) -> u32;
        pub fn wp81_audio_music_play(id: u32);
        pub fn wp81_audio_music_pause(id: u32);
        pub fn wp81_audio_music_seek(id: u32, position: f64);
        pub fn wp81_audio_music_position(id: u32) -> f64;
        pub fn wp81_audio_music_paused(id: u32) -> i32;
        pub fn wp81_audio_music_set_amplifier(id: u32, amp: f32);
        pub fn wp81_audio_music_set_low_pass(id: u32, cutoff: f32);
        pub fn wp81_audio_music_fade(id: u32, seconds: f64, out: i32);

        /// `clip_id` is 0 for every clip, so the name and the duration are the only things that say
        /// *which* sound this is - see `AudioClip::new_named`. The name is a UTF-8 pointer/length
        /// pair rather than a Rust `&str` because this crosses an `extern "C"` boundary.
        pub fn wp81_audio_sfx_new(clip_id: u32, name: *const u8, name_len: usize, secs: f64) -> u32;
        pub fn wp81_audio_sfx_play(id: u32, amp: f32);
    }
}

// ===========================================================================
// clips
// ===========================================================================

#[derive(Clone, Debug)]
pub struct AudioClip {
    id: u32,
    sample_rate: u32,
    frame_count: u64,
    /// The asset name this clip was loaded from, without directory or extension.
    ///
    /// Carried because the shell has no other way to tell one sound from another: `id` is 0 for
    /// every clip (there is no audio engine behind this shim, only the shell's), so `sfx_new` used to
    /// arrive as "clip 0" every time and the shell could not know whether it was asked for the hit
    /// sound or a button click. Fixed size, no allocation - see the DLL size note.
    name: [u8; NAME_CAP],
    name_len: u8,
}

/// Enough for the longest asset name in the resource pack, with room to spare.
///
/// 96 rather than 48 because the *music* name now travels through here too, and a music name is
/// not an asset name: it is `<local_path>/<file>` - `download/79814/4347843821313567.mp3` is 35
/// bytes - and `phira` composes it without knowing how long the chart's id or the file's name will
/// be. Truncation is silent (`new_named` copies up to the cap and stops), so a cap sized for
/// `click` would have handed the shell a path that points at nothing and the chart would have gone
/// back to playing in silence with a *plausible* log line beside it.
pub const NAME_CAP: usize = 96;

impl AudioClip {
    /// Decode a compressed buffer via the shell.
    pub fn new(data: Vec<u8>) -> Result<Self> {
        Self::new_named("", data)
    }

    /// As [`Self::new`], but records which asset this is.
    ///
    /// The caller knows the name (`load_clip!` has it as a literal, the RPE parser has it as a map
    /// key); this shim does not, which is why it has to be handed in rather than derived.
    pub fn new_named(name: &str, data: Vec<u8>) -> Result<Self> {
        let mut info = shell::DecodedInfo {
            sample_rate: 0,
            frame_count: 0,
        };
        let rc = unsafe { shell::wp81_audio_decode(data.as_ptr(), data.len(), &mut info) };
        if rc != 0 || info.sample_rate == 0 {
            return Err(anyhow::anyhow!(
                "audio decode failed (rc={}, {} bytes)",
                rc,
                data.len()
            ));
        }
        // Truncated rather than rejected: a long name still identifies the sound far better than no
        // name at all, and refusing the clip would silence it.
        let mut buf = [0u8; NAME_CAP];
        let mut len = 0usize;
        for b in name.bytes() {
            if len == NAME_CAP {
                break;
            }
            buf[len] = b;
            len += 1;
        }
        Ok(AudioClip {
            id: 0,
            sample_rate: info.sample_rate,
            frame_count: info.frame_count,
            name: buf,
            name_len: len as u8,
        })
    }

    /// Pointer and length of the recorded name, for the `sfx_new` callback.
    pub fn name_parts(&self) -> (*const u8, usize) {
        (self.name.as_ptr(), self.name_len as usize)
    }

    /// `phira`'s song page needs the frames themselves, not a count: `load_local_tuple` takes
    /// `frames.len()` as the track length and rejects the chart when the preview range runs past
    /// it, and `with_effects` drains a range out of the vector before handing what is left to
    /// `from_raw`.
    ///
    /// The decode itself happens in the shell, exactly like `new` - same buffer, decoded once, and
    /// the shell keeps it for `create_music`, which asks for it by clip id. What is returned here
    /// is that buffer's *length* materialised as frames, whose values nothing reads: this shim's
    /// `from_raw` keeps only `frame_count` and drops the vector, `Music` plays the shell's copy,
    /// and `AudioClip::sample` answers `None`, so no path ever observes a sample. Filling them
    /// with silence therefore cannot change behaviour - the length is the whole payload.
    ///
    /// Decoding a second time in Rust (with `symphonia`, which `prpr` already pulls in) would
    /// yield the same length and the same unread samples at the cost of a decoder in this shim.
    /// Note the allocation: it is proportional to the track, and upstream's is too - it is freed
    /// again by `from_raw` a few lines later.
    pub fn decode(data: Vec<u8>) -> Result<(Vec<Frame>, u32)> {
        let mut info = shell::DecodedInfo {
            sample_rate: 0,
            frame_count: 0,
        };
        let rc = unsafe { shell::wp81_audio_decode(data.as_ptr(), data.len(), &mut info) };
        if rc != 0 || info.sample_rate == 0 {
            return Err(anyhow::anyhow!(
                "audio decode failed (rc={}, {} bytes)",
                rc,
                data.len()
            ));
        }
        Ok((vec![Frame(0.0, 0.0); info.frame_count as usize], info.sample_rate))
    }

    /// Build a clip from already-decoded frames. The frames are not retained - see the module note.
    pub fn from_raw(frames: Vec<Frame>, sample_rate: u32) -> Self {
        AudioClip {
            id: 0,
            sample_rate,
            frame_count: frames.len() as u64,
            name: [0u8; NAME_CAP],
            name_len: 0,
        }
    }

    pub fn sample(&self, _position: f64) -> Option<Frame> {
        // Playback and mixing belong to the shell; `prpr` never reads individual samples.
        None
    }

    pub fn frames(&self) -> &[Frame] {
        &[]
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count as usize
    }

    pub fn length(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count as f64 / self.sample_rate as f64
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.id
    }
}

// ===========================================================================
// music
// ===========================================================================

#[derive(Clone, Copy, Debug)]
pub struct MusicParams {
    pub loop_mix_time: f64,
    pub amplifier: f32,
    pub playback_rate: f64,
    pub command_buffer_size: usize,
}

impl Default for MusicParams {
    fn default() -> Self {
        MusicParams {
            loop_mix_time: -1.0,
            amplifier: 1.0,
            playback_rate: 1.0,
            command_buffer_size: 16,
        }
    }
}

/// A playing music stream.
///
/// `Clone` is deliberate and load bearing: `prpr` clones the handle to keep one copy for seeking and
/// one for querying, and expects both to refer to the same underlying stream. The id is what makes
/// that work.
#[derive(Clone, Debug)]
pub struct Music {
    id: u32,
    length: f64,
    playback_rate: f64,
}

impl Music {
    pub fn play(&mut self) -> Result<()> {
        unsafe { shell::wp81_audio_music_play(self.id) };
        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        unsafe { shell::wp81_audio_music_pause(self.id) };
        Ok(())
    }

    pub fn paused(&mut self) -> bool {
        unsafe { shell::wp81_audio_music_paused(self.id) != 0 }
    }

    pub fn set_amplifier(&mut self, amp: f32) -> Result<()> {
        unsafe { shell::wp81_audio_music_set_amplifier(self.id, amp) };
        Ok(())
    }

    pub fn seek_to(&mut self, position: f64) -> Result<()> {
        unsafe { shell::wp81_audio_music_seek(self.id, position) };
        Ok(())
    }

    pub fn set_low_pass(&mut self, cutoff: f32) -> Result<()> {
        unsafe { shell::wp81_audio_music_set_low_pass(self.id, cutoff) };
        Ok(())
    }

    pub fn fade_in(&mut self, time: f64) -> Result<()> {
        unsafe { shell::wp81_audio_music_fade(self.id, time, 0) };
        Ok(())
    }

    pub fn fade_out(&mut self, time: f64) -> Result<()> {
        unsafe { shell::wp81_audio_music_fade(self.id, time, 1) };
        Ok(())
    }

    /// The chart clock reads this every frame, so it comes straight from the shell's player rather
    /// than from anything Rust tracks.
    pub fn position(&self) -> f64 {
        unsafe { shell::wp81_audio_music_position(self.id) }
    }

    /// Seconds, already adjusted for `playback_rate` - `prpr` compares this against chart duration.
    pub fn length(&self) -> f64 {
        if self.playback_rate > 0.0 {
            self.length / self.playback_rate
        } else {
            self.length
        }
    }

    /// Not in upstream `sasa`; `prpr` only ever uses `Option<Music>`, and this keeps that ergonomic.
    pub fn is_none(&self) -> bool {
        self.id == 0
    }
}

// ===========================================================================
// sfx
// ===========================================================================

#[derive(Clone, Copy, Debug)]
pub struct PlaySfxParams {
    pub amplifier: f32,
}

impl Default for PlaySfxParams {
    fn default() -> Self {
        PlaySfxParams { amplifier: 1.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Sfx {
    id: u32,
}

impl Sfx {
    pub fn play(&mut self, params: PlaySfxParams) -> Result<()> {
        unsafe { shell::wp81_audio_sfx_play(self.id, params.amplifier) };
        Ok(())
    }
}

// ===========================================================================
// backends
//
// `prpr::ext::create_audio_manger` picks a backend by `cfg`. Only the non-Android, non-OHOS branch
// can ever be selected here, but all three are declared so that the `cfg` blocks compile wherever
// they are evaluated.
// ===========================================================================

pub mod backend {
    use super::Result;

    /// Opaque: upstream's has `pub(crate)` fields, so nothing outside `sasa` can build one. Only the
    /// backend receives it.
    pub struct BackendSetup {
        _private: (),
    }

    pub trait Backend {
        fn setup(&mut self, setup: BackendSetup) -> Result<()>;
        fn start(&mut self) -> Result<()>;
        fn consume_broken(&self) -> bool;
    }

    pub mod cpal {
        use super::{Backend, BackendSetup};
        use crate::Result;

        /// `Default` is not optional here: `prpr::ui` builds one directly with
        /// `CpalSettings::default()` at ui.rs:1378, so omitting it fails prpr's own compile.
        #[derive(Clone, Debug, Default)]
        pub struct CpalSettings {
            pub preferred_sample_rate: Option<u32>,
            pub buffer_size: Option<u32>,
        }

        /// The desktop WASAPI backend, replaced by the shell's player on this platform.
        #[derive(Default, Debug)]
        pub struct CpalBackend {
            _settings: Option<CpalSettings>,
        }

        impl CpalBackend {
            pub fn new(settings: CpalSettings) -> Self {
                CpalBackend {
                    _settings: Some(settings),
                }
            }
        }

        impl Backend for CpalBackend {
            fn setup(&mut self, _setup: BackendSetup) -> Result<()> {
                Ok(())
            }
            fn start(&mut self) -> Result<()> {
                Ok(())
            }
            fn consume_broken(&self) -> bool {
                false
            }
        }
    }

    pub mod oboe {
        use super::{Backend, BackendSetup};
        use crate::Result;

        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum PerformanceMode {
            #[default]
            None,
            PowerSaving,
            LowLatency,
        }

        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum Usage {
            #[default]
            Media,
            Game,
        }

        #[derive(Clone, Debug, Default)]
        pub struct OboeSettings {
            pub buffer_size: Option<u32>,
            pub performance_mode: PerformanceMode,
            pub usage: Usage,
        }

        #[derive(Default, Debug)]
        pub struct OboeBackend {
            _settings: Option<OboeSettings>,
        }

        impl OboeBackend {
            pub fn new(settings: OboeSettings) -> Self {
                OboeBackend {
                    _settings: Some(settings),
                }
            }
        }

        impl Backend for OboeBackend {
            fn setup(&mut self, _setup: BackendSetup) -> Result<()> {
                Ok(())
            }
            fn start(&mut self) -> Result<()> {
                Ok(())
            }
            fn consume_broken(&self) -> bool {
                false
            }
        }
    }

    pub mod ohos {
        use super::{Backend, BackendSetup};
        use crate::Result;

        #[derive(Clone, Debug, Default)]
        pub struct OhosSettings {
            pub sample_rate: u32,
            pub buffer_size: Option<u32>,
            pub channels: u32,
        }

        #[derive(Default, Debug)]
        pub struct OhosBackend {
            _settings: Option<OhosSettings>,
        }

        impl OhosBackend {
            pub fn new(settings: OhosSettings) -> Self {
                OhosBackend {
                    _settings: Some(settings),
                }
            }
        }

        impl Backend for OhosBackend {
            fn setup(&mut self, _setup: BackendSetup) -> Result<()> {
                Ok(())
            }
            fn start(&mut self) -> Result<()> {
                Ok(())
            }
            fn consume_broken(&self) -> bool {
                false
            }
        }
    }
}

pub use backend::Backend;

// ===========================================================================
// the manager
// ===========================================================================

pub struct AudioManager {
    backend: Box<dyn Backend>,
}

impl AudioManager {
    pub fn new(backend: impl Backend + 'static) -> Result<Self> {
        Ok(AudioManager {
            backend: Box::new(backend),
        })
    }

    pub fn new_box(backend: Box<dyn Backend>) -> Result<Self> {
        Ok(AudioManager { backend })
    }

    pub fn create_sfx(&mut self, clip: AudioClip, _buffer_size: Option<usize>) -> Result<Sfx> {
        let (name, name_len) = clip.name_parts();
        Ok(Sfx {
            id: unsafe { shell::wp81_audio_sfx_new(clip.id(), name, name_len, clip.length()) },
        })
    }

    pub fn create_music(&mut self, clip: AudioClip, settings: MusicParams) -> Result<Music> {
        let (name, name_len) = clip.name_parts();
        let id = unsafe {
            shell::wp81_audio_music_new(
                clip.id(),
                name,
                name_len,
                settings.playback_rate,
                settings.amplifier,
                settings.loop_mix_time,
                clip.length(),
            )
        };
        Ok(Music {
            id,
            length: clip.length(),
            playback_rate: settings.playback_rate,
        })
    }

    pub fn estimate_latency(&self) -> f32 {
        // The shell owns the player; `prpr` only shows this in a debug overlay.
        0.0
    }

    pub fn consume_broken(&self) -> bool {
        self.backend.consume_broken()
    }

    pub fn start(&mut self) -> Result<()> {
        self.backend.start()
    }

    pub fn recover_if_needed(&mut self) -> Result<()> {
        if self.consume_broken() {
            self.start()
        } else {
            Ok(())
        }
    }
}

/// `prpr` builds renderers directly in a couple of places. Kept so those call sites compile.
pub trait Renderer {}
