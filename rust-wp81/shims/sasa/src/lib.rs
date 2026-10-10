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
        ///
        /// Kept for the legacy contract; the real decode now happens in Rust (`symphonia`) and
        /// the PCM arrives through `wp81_audio_pcm` below, which is what the mixer consumes.
        pub fn wp81_audio_decode(data: *const u8, len: usize, out: *mut DecodedInfo) -> i32;

        /// Hand one decoded clip to the shell, as interleaved f32 at `rate` and `channels`.
        ///
        /// `tag` is 0 for a music track, 1 for a hit sound; `key`/`key_len` name the asset the
        /// way `wp81_audio_sfx_new` does, because the shell's mixer keys its library on the name,
        /// not on the id (which is 0 for every clip). The bytes are copied by the callee; the
        /// buffer is freed on return. Returns 0 on success, 1 on failure.
        pub fn wp81_audio_pcm(
            tag: u32,
            key: *const u8,
            key_len: usize,
            rate: u32,
            channels: u32,
            frames: *const f32,
            frame_count: u64,
        ) -> i32;

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
    /// The decoded PCM, interleaved at `pcm_channels` channels and `pcm_rate`.
    ///
    /// Kept on the clip rather than handed to the shell at construction because the two owners of
    /// it - the music player and the sfx pool - are decided by `create_music`/`create_sfx`, which
    /// run later and only once. A clip that is never played (a res-pack sound the chart does not
    /// use) therefore costs a decode and no upload.
    pcm: Option<std::sync::Arc<Vec<f32>>>,
    pcm_channels: u32,
    pcm_rate: u32,
}

/// Decode compressed bytes to interleaved f32 PCM via symphonia.
///
/// Returns `(samples, sample_rate, channels)`. The samples are *not* resampled - the shell's
/// mixer runs at the file's own rate, because resampling here would either cost a crate or cost
/// precision, and `MediaStreamSource` takes the rate it is given.
///
/// Symphonia is the decoder `prpr` already depends on, so this adds no new crate to the build.
fn decode_pcm(data: &[u8]) -> Result<(Vec<f32>, u32, u32)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let cursor = std::io::Cursor::new(data.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    let hint = Hint::new();
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| anyhow::anyhow!("probe: {e}"))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow::anyhow!("no default track"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| anyhow::anyhow!("codec: {e}"))?;

    let mut out: Vec<f32> = Vec::new();
    // Reserve up front from the compressed size: without this the vector doubles its way
    // up packet by packet, and the peak transient is ~1.5x the final size (old plus new
    // during the last grows) - on a 228 s track that is the difference between a 40 MB
    // spike laid on top of chart loading and none. 10x covers 128 kbps stereo with margin;
    // unusual ratios simply grow past it the old way, and the per-packet reserve below stays
    // as the top-up for those.
    out.reserve(data.len() * 10);
    let mut sample_rate = 0u32;
    let mut channels = 0u32;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if sample_rate == 0 {
            sample_rate = decoded.spec().rate;
            channels = decoded.spec().channels.count() as u32;
            let frames = decoded.frames() as u64;
            out.reserve((frames * channels as u64) as usize);
        }
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        buf.copy_interleaved_ref(decoded);
        out.extend_from_slice(buf.samples());
    }

    if sample_rate == 0 || out.is_empty() {
        return Err(anyhow::anyhow!("decode produced no samples"));
    }
    Ok((out, sample_rate, channels))
}

/// Enough for the longest asset name in the resource pack, with room to spare.
///
/// 192 rather than 48 because the *music* name now travels through here too, and a music name is
/// not an asset name: it is `preview|<from>|<to>|<local_path>/<file>` -
/// `preview|999.9|999.9|download/79814/4347843821313567.mp3` is 52 bytes - and `phira` composes it
/// without knowing how long the chart's id or the file's name will be. Truncation is silent
/// (`new_named`/`from_raw_named` copy up to the cap and stop), so a cap sized for `click` would
/// have handed the shell a path that points at nothing and the chart would have gone back to
/// playing in silence with a *plausible* log line beside it. `name_len` is a `u8`, so 255 is the
/// ceiling; 192 clears every name seen so far with margin.
pub const NAME_CAP: usize = 192;

/// Copy a name into the fixed slot, truncating rather than rejecting.
///
/// Two constructors need this and neither may fail: refusing a clip would silence a sound that is
/// otherwise fine, and a truncated name still identifies the sound far better than no name at all.
fn copy_name(name: &str) -> ([u8; NAME_CAP], u8) {
    let mut buf = [0u8; NAME_CAP];
    let mut len = 0usize;
    for b in name.bytes() {
        if len == NAME_CAP {
            break;
        }
        buf[len] = b;
        len += 1;
    }
    (buf, len as u8)
}

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
        let (pcm, rate, channels) = decode_pcm(&data)?;
        let frame_count = (pcm.len() as u64) / channels.max(1) as u64;
        // The shell still hears the old callback for its own bookkeeping, but the answer it
        // returns is no longer load-bearing - the clip carries its own length now, from the
        // real decode rather than from a callback that was never wired.
        let mut info = shell::DecodedInfo {
            sample_rate: 0,
            frame_count: 0,
        };
        let _ = unsafe { shell::wp81_audio_decode(data.as_ptr(), data.len(), &mut info) };
        // Truncated rather than rejected: see `copy_name`.
        let (buf, len) = copy_name(name);
        Ok(AudioClip {
            id: 0,
            sample_rate: rate,
            frame_count,
            name: buf,
            name_len: len,
            pcm: Some(std::sync::Arc::new(pcm)),
            pcm_channels: channels,
            pcm_rate: rate,
        })
    }

    /// As [`Self::new_named`], but keeps only the length - no symphonia decode, no PCM.
    ///
    /// The game track's samples are never read on this target: `sample` answers `None`,
    /// `create_music` skips tag 0 in the mixer (`-2`, the shell plays its own decode), and
    /// `Music` drives the shell's copy. All `Resource` needs is `length()` (count and rate),
    /// which the shell's decode reports. A 228 s song is ~80 MB of f32 that used to be
    /// allocated here, in the middle of chart loading, where the budget kills the process
    /// with no marker - the same class of spike as the preview silence vector. The name is
    /// still recorded: it is the only thing on the `music_new` boundary that identifies the
    /// audio, and losing it plays the res pack's `ending` instead of the chart's own music.
    pub fn new_named_len(name: &str, data: Vec<u8>) -> Result<Self> {
        let (frame_count, sample_rate) = Self::decode_count(data)?;
        Ok(Self::from_count_named(name, frame_count, sample_rate))
    }

    /// Pointer and length of the recorded name, for the `sfx_new` callback.
    pub fn name_parts(&self) -> (*const u8, usize) {
        (self.name.as_ptr(), self.name_len as usize)
    }

    /// `phira`'s song page needs the track length and the preview window's length, not
    /// samples: `load_local_tuple` takes the count as the track length and rejects the chart
    /// when the preview range runs past it, and the preview clip keeps only its window's
    /// length (see `from_count_named`).
    ///
    /// The decode itself happens in the shell, exactly like `new` - same buffer, decoded
    /// once - and only the count and the rate cross back. Upstream's `decode` materialises
    /// the whole track as frames, and this shim used to do the same with silence: a 228 s
    /// song is ten million frames, and eighty megabytes of zeros were allocated only for
    /// `with_effects` to drain most of them away and `from_raw_named` to drop the rest.
    /// That spike landed in the middle of chart loading, which is where the budget kills
    /// the process with no marker. The values were never read by any path - the shell
    /// keeps the audio for `create_music`, `Music` plays the shell's copy, and
    /// `AudioClip::sample` answers `None` - so not allocating them cannot change behaviour.
    pub fn decode_count(data: Vec<u8>) -> Result<(u64, u32)> {
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
        Ok((info.frame_count, info.sample_rate))
    }

    /// Build a clip from already-decoded frames. The frames are not retained - see the module note.
    pub fn from_raw(frames: Vec<Frame>, sample_rate: u32) -> Self {
        Self::from_raw_named("", frames, sample_rate)
    }

    /// Build a clip from a frame count alone, for callers that never had samples - see
    /// [`Self::decode_count`]. Identical to [`Self::from_raw_named`] with a vector of that
    /// length: the vector is dropped there anyway and only its length survives, so passing
    /// the length directly allocates nothing.
    pub fn from_count_named(name: &str, frame_count: u64, sample_rate: u32) -> Self {
        let (buf, len) = copy_name(name);
        AudioClip {
            id: 0,
            sample_rate,
            frame_count,
            name: buf,
            name_len: len,
            pcm: None,
            pcm_channels: 0,
            pcm_rate: 0,
        }
    }

    /// As [`Self::from_raw`], but records which file these frames came from.
    ///
    /// A preview is *built* rather than read - `phira` slices the chart's soundtrack down to the
    /// `[preview_start, preview_end]` window named in the json - so it can never arrive through
    /// `new_named`, which takes compressed bytes. Without a name the shell heard `""`, failed both
    /// of its path tests, and fell through to the one chart it had resolved before boot: every
    /// preview on the song page played the packaged chart's audio, whole and un-cut. The name is
    /// `preview|<from>|<to>|<local_path>/<file>` so the shell can put the file at the same window
    /// the frames were cut from.
    pub fn from_raw_named(name: &str, frames: Vec<Frame>, sample_rate: u32) -> Self {
        let (buf, len) = copy_name(name);
        AudioClip {
            id: 0,
            sample_rate,
            frame_count: frames.len() as u64,
            name: buf,
            name_len: len,
            pcm: None,
            pcm_channels: 0,
            pcm_rate: 0,
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
        // The PCM goes to the shell's mixer before the handle comes back, so a `sfx_play` that
        // lands in the same frame as `sfx_new` still finds the sound loaded. The return is
        // deliberately unchecked: a refused ingest (mixer down, OOM) must never fail the chart
        // load - the shell falls back to a MediaElement for sounds the mixer never got, while a
        // failed load shows the chart as broken. Do not turn this into a `?`.
        if let (Some(pcm), rate, channels) = (clip.pcm.as_ref(), clip.pcm_rate, clip.pcm_channels) {
            unsafe {
                shell::wp81_audio_pcm(1, name, name_len, rate, channels,
                    pcm.as_ptr(), clip.frame_count);
            }
        }
        Ok(Sfx {
            id: unsafe { shell::wp81_audio_sfx_new(clip.id(), name, name_len, clip.length()) },
        })
    }

    pub fn create_music(&mut self, clip: AudioClip, settings: MusicParams) -> Result<Music> {
        let (name, name_len) = clip.name_parts();
        // Same "never fail the load" rule as `create_sfx`, plus one more: tag 0 always reports
        // rc=-2 ("not attempted") because music plays through the shell's own player, never the
        // mixer - there is no voice that would read an ingested copy. Checking this return and
        // bailing (e.g. `music ingest refused (rc=-2)`) breaks every chart load with a healthy
        // engine. Do not check it.
        if let (Some(pcm), rate, channels) = (clip.pcm.as_ref(), clip.pcm_rate, clip.pcm_channels) {
            unsafe {
                shell::wp81_audio_pcm(0, name, name_len, rate, channels,
                    pcm.as_ptr(), clip.frame_count);
            }
        }
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
