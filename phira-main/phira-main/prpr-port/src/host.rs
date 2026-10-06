//! The Rust game host: builds a real `prpr` `GameScene` and ticks it.
//!
//! # Why this is a separate module
//!
//! `lib.rs` is the *ABI surface* - flat structs and `#[no_mangle]` entry points, nothing else. The
//! host is where the actual game lives, and it is substantial: it owns the scene, drives the async
//! construction, and pumps one frame at a time on demand from the shell.
//!
//! # The three hard problems
//!
//! **1. `GameScene::new` and `Main::new` are `async`.** They await file IO and texture loads. There
//! is no async runtime on WP8.1 and no tokio - the shell calls in from the UI thread. `pollster`
//! (already in the workspace dependency graph) blocks on a future to completion, which is exactly
//! right here: the loads are all synchronous underneath (they forward to C# over a callback), so
//! the future always completes on the first poll and never actually parks.
//!
//! **2. `prpr`'s scene types are `!Send`.** They hold `Rc`/`RefCell` state, thread-locals and raw
//! pointers. So the scene lives in a `thread_local!` slot on whatever thread calls in, and this
//! module assumes the shell always calls from the same thread - which it does, because the C# side
//! drives it from `CompositionTarget.Rendering` on the UI thread.
//!
//! **3. `TimeManager::new` reads `std::time::Instant`.** `Instant::elapsed()` needs
//! `QueryPerformanceFrequency`, and on WP8.1 that is the call that has historically killed the
//! process. `TimeManager::manual(closure)` takes an injectable clock instead, so the shell's own
//! frame delta drives the whole timeline and `Instant` is never touched. This is not just a
//! workaround - it is also what makes the frame timing deterministic.
//!
//! # What the shell must provide
//!
//! * `HostCallbacks.load_asset(name, out, cap)` - file bytes by name, for the chart and its
//!   textures. Backed by `PrprPort.cs` reading the app's local folder.
//! * The audio callbacks, which the `sasa` shim forwards through.

use core::cell::{Cell, RefCell};

use prpr::{
    config::{Config, Mods},
    ext::SafeTexture,
    fs::FileSystem,
    info::ChartInfo,
    scene::{GameMode, GameScene, Main, Scene},
    time::TimeManager,
    ui::TextPainter,
};

use crate::cb;

// ===========================================================================
// the shell clock
//
// `TimeManager` is built with `manual()`, which stores this closure as its time source. The shell
// advances it once per frame with the real delta, so every `now()` in `prpr` - the judge, the
// tweens, the scene transitions - reads a value this host controls.
// ===========================================================================

thread_local! {
    /// Seconds since the game started, advanced only by `frame`.
    static CLOCK: Cell<f64> = const { Cell::new(0.0) };
}

fn clock_now() -> f64 {
    CLOCK.with(|c| c.get())
}

// ===========================================================================
// the asset FileSystem
// ===========================================================================

/// A `FileSystem` that forwards every read to the shell.
///
/// `prpr` calls this for the chart itself, its textures, and `extra.json`. Returning the bytes over
/// a callback rather than reading them here is what keeps file IO in C#, where the `StorageFile`
/// APIs and the app's local folder already live.
#[derive(Clone)]
pub struct HostFileSystem {
    /// Prefix prepended to every requested path - the chart's folder name. `prpr` asks for names
    /// relative to the chart, and the shell stores each chart in its own subdirectory.
    prefix: String,
}

impl HostFileSystem {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self { prefix: prefix.into() }
    }
}

/// First buffer size offered to the shell, and the largest one ever offered.
///
/// The ABI passes bytes through a buffer the *caller* sizes, so the size has to be guessed before
/// the file has been read. Guessing once, high, is the mistake this replaces: a flat 4 MiB rejected
/// the "Transcendence" chart that came with the test set - an 8.8 MB RPE export - with `-2`, which
/// is also what both shells return for "you asked for a file that is not there". One error code
/// for two unrelated failures cost a debugging round to tell apart.
///
/// So the buffer starts at [`ASSET_START`] and doubles on `-2`, up to [`ASSET_CEILING`]. A file
/// that genuinely does not exist answers `-1` on the first call and never grows anything, and a
/// file past the ceiling falls through to `None`, which is the safe direction: `prpr` substitutes a
/// placeholder rather than parsing a truncated chart.
const ASSET_START: usize = 4 * 1024 * 1024;
const ASSET_CEILING: usize = 64 * 1024 * 1024;
/// What both shells return when the file exists but does not fit the buffer offered.
const ASSET_TOO_BIG: i32 = -2;

thread_local! {
    /// One buffer for every asset read, grown to the largest file seen and never shrunk.
    ///
    /// Reading a chart is about twenty-five calls in a row, so allocating and zeroing a fresh 4 MiB
    /// each time is ~100 MiB of memset at startup for no benefit - the memory is used again
    /// immediately. `RefCell` rather than a bare `Vec` because the buffer has to stay borrowed
    /// across the callback, and because the failure mode of borrowing it twice is a panic that the
    /// surrounding `catch_unwind` turns into an error code, never undefined behaviour.
    ///
    /// Precondition: the shell must not call back into Rust while servicing a load. Neither does -
    /// the host copies bytes it has already read, and `PrprPort.OnLoadAsset` reads the file and
    /// marshals it - so nothing re-enters while the borrow is held.
    static SCRATCH: core::cell::RefCell<Vec<u8>> = core::cell::RefCell::new(Vec::new());
}

/// Ask the shell for a file's bytes. Returns `None` when the shell does not have it.
///
/// Shared with three callers: `HostFileSystem` (the chart's own files), the font load, and -
/// through `wp81_install_callbacks` - the macroquad shim's `load_file`, which is how `prpr`'s
/// resource pack reaches the same shell. All three go through this one function because the
/// shell's asset directory is the only place any of them exists.
pub fn load_asset(name: &str) -> Option<Vec<u8>> {
    let f = cb().load_asset?;
    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();

    SCRATCH.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut cap = ASSET_START;
        loop {
            if buf.len() < cap {
                buf.resize(cap, 0);
            }
            // The only unsafe in this function: a pointer into memory that outlives the call and
            // is not reallocated while it is outstanding. The shell may write `buf.len()` bytes.
            let n = unsafe { f(wide.as_ptr(), buf.as_mut_ptr(), buf.len()) };
            if n >= 0 {
                return Some(buf[..n as usize].to_vec());
            }
            if n != ASSET_TOO_BIG || cap >= ASSET_CEILING {
                return None;
            }
            cap = (cap * 2).min(ASSET_CEILING);
        }
    })
}

#[async_trait::async_trait]
impl FileSystem for HostFileSystem {
    async fn load_file(&mut self, path: &str) -> anyhow::Result<Vec<u8>> {
        let full = if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{}", self.prefix, path)
        };
        load_asset(&full).ok_or_else(|| anyhow::anyhow!("asset not found: {full}"))
    }

    async fn exists(&mut self, path: &str) -> anyhow::Result<bool> {
        Ok(self.load_file(path).await.is_ok())
    }

    fn list_root(&self) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(self.clone())
    }

    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

// ===========================================================================
// the hosted game
// ===========================================================================

/// Everything one running game needs.
struct Hosted {
    main: Main,
    painter: TextPainter,
    frames: u64,
}

thread_local! {
    /// The one running game. `None` until `create` succeeds.
    static GAME: RefCell<Option<Hosted>> = const { RefCell::new(None) };

    /// The chart descriptor, kept alive for the lifetime of the game.
    static INFO: RefCell<Option<ChartInfo>> = const { RefCell::new(None) };

    /// Frames pumped through phira's own loop.
    ///
    /// The mirror of `Hosted::frames` for the path where `Main` lives *inside* the app future
    /// rather than in a slot this module owns, so the counter cannot ride along on it. Without
    /// this, `frames()` would have to answer `-1` for a running phira - which is exactly what the
    /// shell reads as "nothing started".
    static PHIRA_FRAMES: Cell<u64> = const { Cell::new(0) };
}

/// How far `create` has got, so the shell can sample it *while* it runs.
///
/// `GameCreate` is slow enough on a phone to be worth watching, and its failure mode is the worst
/// kind: the process stops without returning an error code and without writing a line, so after the
/// fact the only question left is "where". The shell polls this from a timer that is independent of
/// the thread `create` runs on and logs every change - so when the log's last line says step N, step
/// N is what was executing. The renderer already does exactly this (`SampleTick`), and it is what
/// localised the earlier bring-up hangs, which is the only reason it works.
///
/// `Relaxed` because this is a diagnostic, not a synchronisation point: the reader only needs a
/// value that was true at some recent moment, and the process dying is precisely the event that
/// makes that distinction irrelevant.
pub static PROGRESS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Step numbers. Named rather than bare so the shell can print something readable without either
/// side restating the other's constants in its own words.
pub mod progress {
    pub const IDLE: u32 = 0;
    /// Inside `create`, before anything has been built.
    pub const ENTERED: u32 = 1;
    /// The filesystem wrapper exists.
    pub const FS_READY: u32 = 2;
    /// Reading `info.yml` (or inferring it) - this is where the first asset request happens.
    pub const INFO_LOADING: u32 = 3;
    /// Chart metadata resolved.
    pub const INFO_READY: u32 = 4;
    /// `GameScene::new` - chart parse, every texture, the whole respack.
    pub const SCENE_BUILDING: u32 = 5;
    /// Inside `boot_phira`: phira's entry has been called and its own construction is being
    /// pumped to the first `next_frame`. Its phases are phira's, not ours, so this is one step
    /// rather than a sequence - what the shell can tell is *whether* construction is under way.
    pub const PHIRA_BOOTING: u32 = 6;
    /// phira's `MainScene` exists and the app is parked at its first frame, ready to be pumped.
    pub const PHIRA_READY: u32 = 7;
}

#[inline]
fn step(n: u32) {
    PROGRESS.store(n, core::sync::atomic::Ordering::Relaxed);
}

/// Current step, for `wp81_game_progress`.
#[inline]
pub fn progress_now() -> u32 {
    PROGRESS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Result codes shared with the shell. Keep in step with `PrprPort.cs`.
pub const GAME_OK: i32 = 0;
pub const GAME_ERR_NO_CHART: i32 = -1;
pub const GAME_ERR_PARSE: i32 = -2;
pub const GAME_ERR_ALREADY: i32 = -3;
pub const GAME_ERR_PANIC: i32 = -100;

/// Why the most recent attempt failed, so the shell can read it back after a negative return.
///
/// Every failure in `create` is reported with `tracing::error!`, and on the phone there is no
/// subscriber: the event is dropped and `GameCreate -> -2` is the whole diagnosis. That one code
/// covered three unrelated causes - a chart the parser rejects, a `GameScene::new` that fails
/// mid-texture, and a `Main::new` that never gets a font - and on the device it was whichever of
/// them happened, with nothing to tell them apart. Parking the message here costs one uncontended
/// lock per failure and makes the difference between "something failed" and "this failed".
///
/// `Mutex` rather than `static mut` is what keeps this free of `unsafe`: `Option<String>` is `Send`,
/// so the lock is `Sync` and the static needs no `impl` and no wrapper. The shell reads it only in
/// the turn right after the call that produced it, so a writer and a reader never overlap.
static LAST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Record `msg` as the reason the current attempt failed.
fn set_error(msg: String) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(msg);
    }
}

/// Drop any previous reason.
///
/// Called on entry to `create`, after the already-running check, so a stale message can never be
/// reported against a later attempt that failed for a different reason.
fn clear_error() {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = None;
    }
}

/// Hand the last failure message to `f`, or `-1` if there is none.
///
/// The message goes out through a closure rather than as a reference or a guard, so the borrow ends
/// here and nothing outside can hold it past the next `set_error`.
pub fn with_last_error<F: FnOnce(&str) -> i32>(f: F) -> i32 {
    match LAST_ERROR.lock() {
        Ok(slot) => match slot.as_deref() {
            Some(msg) => f(msg),
            None => -1,
        },
        Err(_) => -1,
    }
}

/// Build a game from a chart the shell has already located.
///
/// `chart_name` is the file name inside the chart's folder (e.g. `chart.json`); `folder` is that
/// folder's name under the app's local `charts\` directory. Both are needed because `prpr` resolves
/// textures relative to the chart, and the shell stores each chart separately.
///
/// Returns [`GAME_OK`] or one of the negative codes above.
pub fn create(folder: &str, chart_name: &str) -> i32 {
    step(progress::ENTERED);
    if GAME.with(|g| g.borrow().is_some()) {
        return GAME_ERR_ALREADY;
    }
    clear_error();

    let res = std::panic::catch_unwind(|| {
        let mut fs: Box<dyn FileSystem> = Box::new(HostFileSystem::new(folder));
        step(progress::FS_READY);

        // Chart metadata, read the way Phira reads it: an explicit manifest (`:info`, `info.yml`,
        // `info.txt`, `info.csv`) when the folder has one, otherwise the fields are inferred from
        // the chart itself - and for an RPE chart that means `META.song` becomes `info.music`.
        //
        // The previous version of this function built a `ChartInfo` by hand with `music` left
        // empty, on the theory that "the chart parses its own header, so only the file names have
        // to be right". It is the wrong way round: `music` *is* one of the file names, and with it
        // empty `ResourcePack` does `fs.load_file("")`, which resolves to the folder itself. That
        // is the whole reason `GameCreate` returned -2 with nothing but `asset not found: demo/`
        // in the log.
        //
        // Seeding `chart` before inferring is what makes it work. `fix_info_with` bails with
        // "cannot find chart" unless it already knows the name, and `list_root` deliberately
        // returns nothing (the shell has no listing callback), so nothing else would supply it.
        step(progress::INFO_LOADING);
        let info = match pollster::block_on(prpr::fs::load_info(&mut *fs)) {
            Ok(i) => i,
            Err(e) => {
                tracing::debug!("no info manifest ({e:#}), inferring from the chart's META");
                let mut seed = ChartInfo {
                    name: folder.to_string(),
                    chart: chart_name.to_string(),
                    ..Default::default()
                };
                if let Err(e) = pollster::block_on(prpr::fs::fix_info_with(&mut *fs, &mut seed, true))
                {
                    tracing::error!("chart metadata: {e:#}");
                    set_error(format!("chart metadata: {e:#}"));
                    return GAME_ERR_PARSE;
                }
                seed
            }
        };
        tracing::debug!(
            "chart info: name={} chart={} music={} illustration={}",
            info.name,
            info.chart,
            info.music,
            info.illustration
        );

        step(progress::INFO_READY);

        // Autoplay by default. The judge needs a music clock it can trust, and the shell has not
        // wired its audio position back yet - autoplay is what lets the scene run and render
        // without one, which is the whole point of this milestone.
        let mut config = Config::default();
        config.mods.insert(Mods::AUTOPLAY);

        // The scene wants background + illustration textures. The shell owns the real art; a 1x1
        // black handle is enough here because all `prpr` does with it is measure and tint.
        let placeholder: SafeTexture = macroquad::Texture2D::from_rgba8(1, 1, &[0, 0, 0, 255]).into();

        // The expensive step: chart parse, every texture in the respack, the judge. On a phone this
        // is where the run either takes seconds or stops, so it gets a step of its own.
        step(progress::SCENE_BUILDING);
        let scene = pollster::block_on(GameScene::new(
            GameMode::Normal,
            info.clone(),
            config,
            fs,
            None,             // no player profile: offline play needs none
            placeholder.clone(),
            placeholder,
            None,             // upload_fn
            None,             // update_fn
            None,             // save_fn
        ));

        let scene = match scene {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("GameScene::new failed: {e:?}");
                set_error(format!("GameScene::new failed: {e:#}"));
                return GAME_ERR_PARSE;
            }
        };

        // The injected clock. `manual` captures the closure and calls it for every `now()`, so the
        // whole timeline follows `CLOCK` - and `Instant` is never constructed.
        let tm = TimeManager::manual(Box::new(clock_now));

        // The scene is boxed as `dyn Scene` because that is what `Main` stores - and it must be,
        // since `Main` walks a stack of them for overlays.
        let boxed: Box<dyn Scene> = Box::new(scene);

        // `TextPainter` needs a real font. The shell ships one, so it is read through the same
        // callback the chart uses.
        let font = load_asset("bold.ttf")
            .and_then(|b| font_from_bytes(b))
            .unwrap_or_else(font_fallback);
        let painter = TextPainter::new(font, None);

        let main = match pollster::block_on(Main::new(boxed, tm, None)) {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Main::new failed: {e:?}");
                set_error(format!("Main::new failed: {e:?}"));
                return GAME_ERR_PARSE;
            }
        };

        CLOCK.with(|c| c.set(0.0));
        INFO.with(|slot| *slot.borrow_mut() = Some(info));
        GAME.with(|slot| {
            *slot.borrow_mut() = Some(Hosted {
                main,
                painter,
                frames: 0,
            })
        });

        GAME_OK
    });

    match res {
        Ok(code) => code,
        // A panic in the scene constructor must not take the process down: the shell reports the
        // code, and the message is taken off the payload because there is no stderr on the phone -
        // this string is the only trace such a panic leaves anywhere.
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic with a non-string payload".to_string());
            tracing::error!("panic in wp81_game_create: {msg}");
            set_error(format!("panic in wp81_game_create: {msg}"));
            GAME_ERR_PANIC
        }
    }
}

/// Parse a font, or `None` if the bytes are not one.
///
/// `glyph_brush` re-exports `ab_glyph`, which is where `TextPainter` gets its `FontArc` from, so
/// this reaches the font parser through `prpr`'s own dependency rather than adding a second copy of
/// the crate to the graph.
fn font_from_bytes(bytes: Vec<u8>) -> Option<glyph_brush::ab_glyph::FontArc> {
    glyph_brush::ab_glyph::FontArc::try_from_vec(bytes).ok()
}

/// The fallback font, embedded so text has something to render with.
///
/// `FontArc` has no empty constructor, so a real one is needed. A minimal single-glyph TTF keeps
/// the binary small: text draws as nothing rather than crashing, which is the right failure mode
/// for a missing cosmetic asset.
fn font_fallback() -> glyph_brush::ab_glyph::FontArc {
    glyph_brush::ab_glyph::FontArc::try_from_slice(include_bytes!("fallback.ttf"))
        .expect("the embedded fallback font must parse")
}

/// Advance and render one frame.
///
/// `dt` is the seconds since the previous call, supplied by the shell because the Rust side must
/// not read its own clock here. Draw calls the frame produced are readable afterwards via
/// `wp81_draw_*`.
///
/// Returns the number of draw commands the frame produced, or a negative code when nothing runs.
/// Boot phira itself rather than a chart: build its `MainScene` and leave it parked, ready to be
/// pumped by [`frame`].
///
/// This is the "start straight into Rust phira" path. Where [`create`] builds a `GameScene` from a
/// chart the shell located, this runs phira's own `the_main` - data directory, fonts, the library
/// UI - and stops at the first `next_frame().await`, because from there the shell owns the clock.
///
/// # Why the boot is pumped rather than blocked on
///
/// `the_main` is async, and nothing here may block the thread it runs on: that is the UI thread,
/// the same one `CompositionTarget.Rendering` calls `wp81_game_frame` from, so blocking it freezes
/// the phone rather than waiting politely. Every await phira makes before its loop is served
/// synchronously by the shell hooks (`load_file`, `load_image`, the texture registry), so polling
/// until it parks is not a workaround for missing async - it is the actual shape of the work. The
/// bound exists because a run that *never* parks is waiting for something this thread will never
/// deliver, and an unbounded loop there would freeze instead of report.
///
/// Returns [`GAME_OK`], or the same negative codes [`create`] uses: the shell already knows how to
/// print those, and `-2` carries the reason through `wp81_game_error`.
#[cfg(target_arch = "arm")]
pub fn boot_phira(data_dir: Option<String>, auto_open: Option<String>, auto_play: bool) -> i32 {
    step(progress::ENTERED);
    if GAME.with(|g| g.borrow().is_some()) || macroquad::app::is_running() {
        return GAME_ERR_ALREADY;
    }
    clear_error();
    step(progress::PHIRA_BOOTING);

    // Both land before `phira::boot`, and the data directory has to be first by more than taste:
    // `build_global_window_conf` reads it *during* `from_config`, so a setter called after `boot`
    // would arrive too late. phira leaves `DATA_PATH` unset for this target and falls back to a
    // relative `./data`, which is not ours to write on a phone - the first thing `the_main` would
    // do is fail, and until this existed the reason went into a log with no subscriber.
    if let Some(dir) = data_dir {
        phira::set_data_path(dir);
    }
    phira::set_auto_open(auto_open);
    phira::set_auto_play(auto_play);
    // From here to the `phira: boot_phira -> ` line the shell sees nothing: this is one call on
    // the UI thread, and phira's own `tracing` has no subscriber on this target. These three lines
    // are the only thing that distinguishes "died before phira ran", "died inside phira's
    // construction" and "phira booted and the shell never got the return value".
    phira::set_log_sink(crate::log_line);
    crate::log_line("phira: pump start (the_main not yet polled)");

    let res = std::panic::catch_unwind(|| {
        phira::boot();
        crate::log_line("phira: window stored, polling the_main");

        let mut polls = 0u32;
        for i in 0..BOOT_POLLS {
            if macroquad::app::reached_first_frame() {
                return Ok(());
            }
            let poll = macroquad::app::poll();
            polls = i + 1;
            if i == 0 {
                // Whether the *first* poll returned Pending or Ready is the whole difference
                // between "construction is running across many frames" and "it finished or died
                // immediately", which is otherwise unrecoverable: the shell only sees the return.
                crate::log_line(match poll {
                    std::task::Poll::Ready(()) => "phira: poll 0 -> Ready (the_main finished)",
                    std::task::Poll::Pending => "phira: poll 0 -> Pending (parked, booting)",
                });
            }
            if let std::task::Poll::Ready(()) = poll {
                // A future that has completed is either `the_main` returning an error or the
                // window closing, and the two are indistinguishable here. Ask phira which: the
                // message is only `None` when nothing actually failed, in which case the old
                // wording was the honest one after all.
                return Err(phira::boot_error()
                    .unwrap_or_else(|| "the application exited before its first frame".to_owned()));
            }
        }
        Err(format!(
            "phira never reached its first frame after {polls} polls (reached={}, exited={})",
            macroquad::app::reached_first_frame(),
            macroquad::app::has_exited()
        ))
    });

    match res {
        Ok(Ok(())) => {
            // phira's *first* poll runs a whole loop iteration before parking, so the queue
            // already holds one frame's worth of commands. Drop them: they belong to no
            // `wp81_game_frame`, and leaving them would make the shell's first read look like it
            // had drawn something before ever being asked to.
            macroquad::draw::drain_reset();
            step(progress::PHIRA_READY);
            crate::log_line("phira: boot_phira -> 0 (first frame reached)");
            GAME_OK
        }
        Ok(Err(msg)) => {
            crate::log_line(&format!("phira: boot_phira -> -2: {msg}"));
            set_error(msg);
            GAME_ERR_PARSE
        }
        Err(_) => {
            crate::log_line("phira: boot_phira -> -100 (panic inside the pump)");
            GAME_ERR_PANIC
        }
    }
}

/// How many polls `boot_phira` gives phira's construction before calling it stuck.
///
/// One is the expected number - every await before the loop resolves synchronously - so this is
/// not a timeout, it is a ceiling on how long a *broken* boot may hold the UI thread. 64 is
/// arbitrary but small: the shell's next frame arrives ~16 ms later, and construction needing more
/// than 64 resumptions to reach its first `next_frame` is looping, not loading.
#[cfg(target_arch = "arm")]
const BOOT_POLLS: u32 = 64;

pub fn frame(dt: f64) -> i32 {
    // Two engines can be running here, and they are driven differently. `phira` owns its own loop -
    // `the_main` calls `main.update()`/`main.render()` and then parks at `next_frame().await` (see
    // `macroquad::app`) - so the shell pumps it. The legacy host owns the `Main` and is driven from
    // below. Deciding up front also makes "nothing is running" one answer rather than two.
    let pump_phira = macroquad::app::is_running();
    let hosted = GAME.with(|g| g.borrow().is_some());
    if !pump_phira && !hosted {
        return GAME_ERR_NO_CHART;
    }

    // A fresh frame starts with no complaint. Without this the shell would read last frame's -
    // or, worse, the create's - against a frame that failed for a different reason.
    clear_error();

    // Advance the injected clock first: everything `prpr` reads this frame goes through it.
    let now = CLOCK.with(|c| c.get()) + dt.max(0.0);
    CLOCK.with(|c| c.set(now));

    // Publish the uptime the judge ages touches with. `prpr::judge::get_uptime` reads it back
    // through `miniquad::native::windows::get_uptime`, and uses it in exactly one place:
    // `t - (uptime - it.time) * spd`, which rewinds a touch to the chart time it happened at.
    //
    // It is this clock and not a second, independent one on purpose. `t` is `TimeManager::now`
    // fed by `clock_now` above, so the two sides of that subtraction would otherwise come from
    // oscillators that drift apart - a bug that shows up as judgement error growing over a
    // play rather than as a visible failure. `std::time::Instant` is the obvious other choice
    // and is rejected here for two reasons: it is what module doc problem 3 says this port
    // avoids (it needs `QueryPerformanceFrequency`, the call that has historically taken the
    // process down on the phone), and it would pull Win32 imports into the ARM DLL that the
    // `check_imports.py` gate exists to catch.
    //
    // Whoever stamps a touch later must stamp it with `miniquad::uptime_secs()` - this value
    // - because `prpr` uses only the difference between the two. Today nothing stamps one at
    // all: no touch reaches `prpr::judge` on this platform yet, so `it.time` has no producer
    // and this reads as dead code until the shell's touch pipeline lands.
    macroquad::miniquad::set_uptime_secs(now);

    if pump_phira {
        // Empty the queue first. phira's loop has no equivalent of this, because in upstream
        // macroquad a frame *replaces* the framebuffer instead of appending to a list - and here
        // `update` and `render` happen inside that loop, so they cannot be separated the way the
        // legacy path separates them below. Left uncleared, whatever one frame drew would be
        // replayed next to the next frame's commands forever.
        macroquad::draw::drain_reset();
        if let std::task::Poll::Ready(()) = macroquad::app::poll() {
            // `the_main` returning is a one-way event: its loop ends only when `should_exit()`
            // says so, and nothing here can restart it. Recording it as this frame's error is what
            // turns "the UI went blank" into a sentence the shell can print.
            set_error("the application exited (`the_main` returned)".to_owned());
        }
        macroquad::input::clear_touches();
        PHIRA_FRAMES.with(|f| f.set(f.get().wrapping_add(1)));
        return macroquad::draw::encode() as i32;
    }

    GAME.with(|slot| {
        let mut guard = slot.borrow_mut();
        let game = match guard.as_mut() {
            Some(g) => g,
            None => return GAME_ERR_NO_CHART,
        };

        if let Err(e) = game.main.update() {
            tracing::error!("Main::update failed: {e:?}");
            set_error(format!("Main::update failed: {e:#}"));
        }

        // Cleared here rather than by the shell so a frame that fails to render cannot leave stale
        // commands for the next one.
        macroquad::draw::drain_reset();

        if let Err(e) = game.main.render(&mut game.painter) {
            tracing::error!("Main::render failed: {e:?}");
            // This one matters more than the rest put together. `render` returning `Err` before it
            // has drawn anything makes `encode()` hand back zero commands, and zero commands looks
            // exactly like "the renderer is not wired up yet" from the shell - a diagnosis that
            // sends you off rewriting the wrong half of the system. `{e:#}` because this is
            // `anyhow`, and the alternate form is what prints the whole `caused by` chain instead
            // of only the outermost line.
            set_error(format!("Main::render failed: {e:#}"));
        }

        game.frames += 1;
        // The queue lives until here rather than being drained by its first reader: `Judge::update`
        // takes the touch *state* from `touches()` while `repeat_all_miniquad_input` takes the
        // *events*, and `Scene` reads `get_touches()` again while rendering. Clearing at any point
        // before all three have run would hand one of them a frame older than the one it is drawing.
        macroquad::input::clear_touches();
        macroquad::draw::encode() as i32
    })
}

/// Set the engine timeline to an absolute time.
///
/// The shell owns the music clock - it is the side with the audio position - so `CLOCK` is a
/// mirror of it rather than a clock of its own. `frame` then reads `CLOCK` to publish the uptime
/// `prpr` ages touches with, so this one write is what keeps three things on the same oscillator:
/// the chart time the renderer draws, the judgement window, and the timestamp every touch carries.
/// Three clocks that each advance would disagree by a gap that grows as the play goes on, and that
/// is not a visible failure - it is a judgement that gets worse the longer you play.
///
/// Ignored for a non-finite value: a NaN entering here would reach every projection in the frame
/// and come out as NaN command coordinates, and "a frame full of NaN" is far harder to diagnose
/// than a time that did not move.
pub fn set_time(t: f64) {
    if !t.is_finite() {
        return;
    }
    CLOCK.with(|c| c.set(t));
}

/// Tear the game down. Safe to call when nothing is running.
pub fn destroy() {
    GAME.with(|slot| *slot.borrow_mut() = None);
    INFO.with(|slot| *slot.borrow_mut() = None);
    // phira's app future is torn down in the same breath. `destroy` means "the shell is finished
    // with this surface", and leaving the future in the thread-local would let the next
    // `wp81_game_frame` resume a loop whose window, input queue and audio all belong to the page
    // that just went away.
    macroquad::app::drop_app();
    PHIRA_FRAMES.with(|f| f.set(0));
    CLOCK.with(|c| c.set(0.0));
    // Reset the published uptime together with the clock it is sampled from. Without this a read
    // between `destroy` and the next `frame` would answer with the previous game's value while
    // `CLOCK` already said zero, and the two are only ever meaningful as a pair.
    macroquad::miniquad::set_uptime_secs(0.0);
    macroquad::draw::drain_reset();
}

/// Frames rendered since `create`, or -1 when nothing is running.
pub fn frames() -> i64 {
    if let Some(n) = GAME.with(|slot| slot.borrow().as_ref().map(|g| g.frames as i64)) {
        return n;
    }
    // phira is running but owns no `Hosted`, so answer from its own counter rather than -1: the
    // shell logs this every frame, and a permanent `-1` would read as a failed start.
    if macroquad::app::is_running() {
        return PHIRA_FRAMES.with(|f| f.get() as i64);
    }
    -1
}

/// The running game's score, combo and accuracy, or -1 when nothing is running.
///
/// `scene::Main` owns the judge (`scene/game.rs:123`, `pub judge: Judge`), so this is the only way
/// for the shell to learn how a play went. The shell's own `Judge` is never advanced on this path
/// and would report a flawless zero-score run - a result page that says "0 points, 100% accuracy"
/// after a real play is worse than one that says nothing, because it looks like a real verdict.
/// The shell's result, laid out for FFI.
///
/// A port of `prpr::judge::PlayResult` rather than that type itself, because `PlayResult` has no
/// `#[repr(C)]` - its field order and padding are Rust's to choose, so a C# mirror could not
/// promise to match them. Copying field by field confines the risk to this one struct literal:
/// a field added upstream breaks here at compile time, not silently on a result screen.
///
/// Every field is 4 bytes, `accuracy` carried as `f32` rather than the original `f64` on purpose.
/// That makes the layout 72 bytes of `u32`/`f32` with no padding anywhere, which in turn means the
/// two sides never have to agree on an alignment rule first - the usual way an `f64` in the middle
/// of an FFI struct goes wrong is that C and .NET round its offset differently, and there is no
/// offset to round here. The precision loss is irrelevant: the shell prints two decimal places of
/// a ratio spanning 0..1, and `f32` carries seven significant digits.
#[repr(C)]
pub struct PlayResultAbi {
    pub accuracy: f32,
    pub score: u32,
    pub max_combo: u32,
    pub num_of_notes: u32,
    pub counts: [u32; 4],
    pub early: u32,
    pub late: u32,
    pub early_kind: [u32; 4],
    pub late_kind: [u32; 4],
}

/// Fills `out` with the running game's result, or returns -1 when there is none.
pub fn result(out: *mut PlayResultAbi) -> i32 {
    GAME.with(|slot| {
        let mut guard = slot.borrow_mut();
        let game = match guard.as_mut() {
            Some(g) => g,
            None => return -1,
        };
        // The judge lives on `GameScene`, which `create` handed to `Main::new` as a
        // `Box<dyn Scene>` - this is the only route back to it. Every step returns -1 rather than
        // a default because there is no safe default: reporting "0 points, 0 notes" after a real
        // play is worse than reporting nothing, since it looks like a real verdict. A loading or
        // ending scene on top simply has no judge and reports -1, which the shell handles.
        let scene = match game.main.scenes.last_mut() {
            Some(s) => s,
            None => return -1,
        };
        let any = match scene.as_any_mut() {
            Some(a) => a,
            None => return -1,
        };
        let gs = match any.downcast_mut::<GameScene>() {
            Some(g) => g,
            None => return -1,
        };
        let r = gs.judge.result();
        // The only `unsafe`, and deliberately as small as it can be: `r` was already read through
        // entirely safe code, so what the block needs is exactly what `wp81_game_result`
        // documents - `out` writable for one `PlayResultAbi` when non-null.
        if !out.is_null() {
            unsafe {
                *out = PlayResultAbi {
                    accuracy: r.accuracy as f32,
                    score: r.score,
                    max_combo: r.max_combo,
                    num_of_notes: r.num_of_notes,
                    counts: r.counts,
                    early: r.early,
                    late: r.late,
                    early_kind: r.early_kind,
                    late_kind: r.late_kind,
                };
            }
        }
        0
    })
}

/// Register the shell's screen size before the first frame.
///
/// `prpr` projects through `screen_width`/`screen_height`/`screen_aspect`, all of which read the
/// shim's `SCREEN` static. Leaving it at 0x0 makes every projection degenerate and the chart
/// renders as a single point.
pub fn set_screen(width: f32, height: f32) {
    unsafe {
        macroquad::window::SCREEN = macroquad::Vec2::new(width, height);
    }
}

/// Whether a game is currently hosted.
///
/// Covers both engines: the legacy `Hosted` slot and phira's own pumped loop. The shell asks this
/// before it decides whether a surface is live, so answering only for one of them would report a
/// running phira as idle and invite a second boot over the top of the first.
pub fn is_running() -> bool {
    GAME.with(|slot| slot.borrow().is_some()) || macroquad::app::is_running()
}
