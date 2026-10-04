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
}

/// Result codes shared with the shell. Keep in step with `PrprPort.cs`.
pub const GAME_OK: i32 = 0;
pub const GAME_ERR_NO_CHART: i32 = -1;
pub const GAME_ERR_PARSE: i32 = -2;
pub const GAME_ERR_ALREADY: i32 = -3;
pub const GAME_ERR_PANIC: i32 = -100;

/// Build a game from a chart the shell has already located.
///
/// `chart_name` is the file name inside the chart's folder (e.g. `chart.json`); `folder` is that
/// folder's name under the app's local `charts\` directory. Both are needed because `prpr` resolves
/// textures relative to the chart, and the shell stores each chart separately.
///
/// Returns [`GAME_OK`] or one of the negative codes above.
pub fn create(folder: &str, chart_name: &str) -> i32 {
    if GAME.with(|g| g.borrow().is_some()) {
        return GAME_ERR_ALREADY;
    }

    let res = std::panic::catch_unwind(|| {
        let mut fs: Box<dyn FileSystem> = Box::new(HostFileSystem::new(folder));

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

        // Autoplay by default. The judge needs a music clock it can trust, and the shell has not
        // wired its audio position back yet - autoplay is what lets the scene run and render
        // without one, which is the whole point of this milestone.
        let mut config = Config::default();
        config.mods.insert(Mods::AUTOPLAY);

        // The scene wants background + illustration textures. The shell owns the real art; a 1x1
        // black handle is enough here because all `prpr` does with it is measure and tint.
        let placeholder: SafeTexture = macroquad::Texture2D::from_rgba8(1, 1, &[0, 0, 0, 255]).into();

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
        // code and the log has the message.
        Err(_) => {
            tracing::error!("panic in wp81_game_create");
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
pub fn frame(dt: f64) -> i32 {
    GAME.with(|slot| {
        let mut guard = slot.borrow_mut();
        let game = match guard.as_mut() {
            Some(g) => g,
            None => return GAME_ERR_NO_CHART,
        };

        // Advance the injected clock first: everything `prpr` reads this frame goes through it.
        let now = CLOCK.with(|c| c.get()) + dt.max(0.0);
        CLOCK.with(|c| c.set(now));

        if let Err(e) = game.main.update() {
            tracing::error!("Main::update failed: {e:?}");
        }

        // Cleared here rather than by the shell so a frame that fails to render cannot leave stale
        // commands for the next one.
        macroquad::draw::drain_reset();

        if let Err(e) = game.main.render(&mut game.painter) {
            tracing::error!("Main::render failed: {e:?}");
        }

        game.frames += 1;
        macroquad::draw::encode() as i32
    })
}

/// Tear the game down. Safe to call when nothing is running.
pub fn destroy() {
    GAME.with(|slot| *slot.borrow_mut() = None);
    INFO.with(|slot| *slot.borrow_mut() = None);
    CLOCK.with(|c| c.set(0.0));
    macroquad::draw::drain_reset();
}

/// Frames rendered since `create`, or -1 when nothing is running.
pub fn frames() -> i64 {
    GAME.with(|slot| slot.borrow().as_ref().map(|g| g.frames as i64).unwrap_or(-1))
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
pub fn is_running() -> bool {
    GAME.with(|slot| slot.borrow().is_some())
}
