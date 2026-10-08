prpr_l10n::tl_file!("common" ttl crate::);

#[rustfmt::skip]
#[cfg(closed)]
mod inner;

mod anim;
mod censor;
mod charts_view;
mod client;
mod data;
pub mod deeplink;
mod icons;
mod images;
mod login;
mod mp;
mod page;
mod popup;
mod rate;
mod resource;
mod scene;
mod tabs;
mod tags;
mod threed;
mod uml;

use anyhow::Result;
use data::Data;
use macroquad::prelude::*;
use prpr::{
    build_conf,
    core::{init_assets, PGR_FONT},
    ext::SafeTexture,
    scene::show_error,
    time::TimeManager,
    ui::{cleanup_audio, FontArc, TextPainter},
    Main,
};
use prpr_l10n::set_prefered_locale;
#[cfg(not(feature = "hykb"))]
use prpr_l10n::{GLOBAL, LANGS};
use scene::MainScene;
use std::{
    collections::VecDeque,
    sync::{mpsc, Mutex},
};
use tracing::{error, info};

#[cfg(target_os = "android")]
use jni::{
    objects::{JClass, JString},
    sys::jint,
    EnvUnowned,
};

static MESSAGES_TX: Mutex<Option<mpsc::Sender<bool>>> = Mutex::new(None);
static DATA_PATH: Mutex<Option<String>> = Mutex::new(None);
static CACHE_DIR: Mutex<Option<String>> = Mutex::new(None);
pub static mut DATA: Option<Data> = None;

#[cfg(target_env = "ohos")]
use napi_derive_ohos::napi;

#[cfg(closed)]
pub async fn load_res(name: &str) -> Vec<u8> {
    let bytes = load_file(name).await.unwrap();
    inner::resolve_data(bytes)
}

#[allow(unused)]
pub async fn load_res_tex(name: &str) -> SafeTexture {
    #[cfg(closed)]
    {
        let bytes = load_res(name).await;
        let image = image::load_from_memory(&bytes).unwrap();
        image.into()
    }
    #[cfg(not(closed))]
    prpr::ext::BLACK_TEXTURE.clone()
}

pub fn sync_data() {
    if get_data().language.is_none() {
        #[cfg(feature = "hykb")]
        let default_lang = "zh-CN".to_owned();
        #[cfg(not(feature = "hykb"))]
        let default_lang = LANGS[GLOBAL.order.lock().unwrap()[0]].to_owned();
        get_data_mut().language = Some(default_lang);
    }
    set_prefered_locale(get_data().language.as_ref().and_then(|it| it.parse().ok()));
    let _ = client::set_access_token_sync(get_data().tokens.as_ref().map(|it| &*it.0));
}

pub fn set_data(data: Data) {
    unsafe {
        DATA = Some(data);
    }
}

#[allow(static_mut_refs)]
pub fn get_data() -> &'static Data {
    unsafe { DATA.as_ref().unwrap() }
}

#[allow(static_mut_refs)]
pub fn get_data_mut() -> &'static mut Data {
    unsafe { DATA.as_mut().unwrap() }
}

pub fn save_data() -> Result<()> {
    let path = format!("{}/data.json", dir::root()?);
    let bytes = serde_json::to_string(get_data())?;
    // Logged because the two ways a user reports this - "the option did not stick" and "the write
    // failed and nothing said so" - are indistinguishable without it. `save_data` returns a
    // `Result` that most callers fold into a scene update which has no log of its own, and
    // `std::fs::write` on a phone fails for reasons (full, locked, mid-tombstone) that leave no
    // other trace in the run. Cheap: settings write once after a 0.5 s debounce, records on finish.
    let res = std::fs::write(&path, bytes.as_bytes());
    match &res {
        Ok(()) => log_line(&format!("data: saved {} ({} bytes)", path, bytes.len())),
        Err(err) => log_line(&format!("data: SAVE FAILED {} ({})", path, err)),
    }
    res?;
    Ok(())
}

// ---------------------------------------------------------------------------
// What a host has to tell phira before it starts, and what it gets back.
//
// `the_main` picks its data directory at compile time for ohos and ios and leaves `DATA_PATH`
// unset everywhere else, which lands on `dir::root()`'s fallback of a relative `./data`. There is
// no working directory worth the name on a phone, so `create_dir_all` fails, `the_main` returns
// before it has drawn anything, and the reason goes into an `error!` that has no subscriber on
// this target (the `log` feature is off). A host that hands over a directory it knows is writable
// fixes the first half; these accessors fix the second by letting it ask why.
//
// The chart request exists for the same reason phira has a deeplink at all: its only ways into a
// song are a download or a tap in the charts view, and a host that already has the chart in its
// package should not have to fake a tap to reach it.
// ---------------------------------------------------------------------------

/// Where phira may create and write its data directory (`<path>/data`, its charts, its cache).
///
/// Must be called before [`boot`]: `build_global_window_conf` reads it while the window is being
/// configured, which happens inside `boot` itself.
pub fn set_data_path(path: String) {
    *DATA_PATH.lock().unwrap() = Some(path);
}

static AUTO_OPEN: Mutex<Option<String>> = Mutex::new(None);

/// The local chart to open on its song page as soon as the library is up, written the way
/// [`data::LocalChart::local_path`] writes it - `custom/190`, not a filesystem path. `None` stops
/// at the library, which is what an ordinary phira does. Call before [`boot`].
pub fn set_auto_open(local_path: Option<String>) {
    *AUTO_OPEN.lock().unwrap() = local_path;
}

/// [`set_auto_open`]'s request, taken on first read. Taken rather than copied because the library
/// rebuilds itself as scenes come and go, and a request that outlived the first read would open
/// the chart again every time it did.
pub fn take_auto_open() -> Option<String> {
    AUTO_OPEN.lock().unwrap().take()
}

static AUTO_PLAY: Mutex<bool> = Mutex::new(false);

/// Whether [`set_auto_open`]'s chart should start itself instead of waiting on a tap. Kept separate
/// from which chart, because the two are decided by different things: the chart is what the host
/// packaged, whether to drop the player straight into it is a product choice the host may not want
/// to make the same way every time.
pub fn set_auto_play(on: bool) {
    *AUTO_PLAY.lock().unwrap() = on;
}

/// [`set_auto_play`]'s request, taken on first read, for the same reason as [`take_auto_open`].
pub fn take_auto_play() -> bool {
    let mut on = AUTO_PLAY.lock().unwrap();
    let was = *on;
    *on = false;
    was
}

static BOOT_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Why `the_main` failed. Kept as well as logged because the log is dropped here, and a host told
/// only "exited before its first frame" cannot tell an unwritable data directory from a missing
/// font or a chart the parser rejected - which are three completely different repairs.
pub fn set_boot_error(msg: String) {
    *BOOT_ERROR.lock().unwrap() = Some(msg);
}

/// Take [`set_boot_error`]'s message, clearing it so a later failure cannot be reported as this one.
pub fn boot_error() -> Option<String> {
    BOOT_ERROR.lock().unwrap().take()
}

static LOG_SINK: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// Install the host's log sink.
///
/// A plain `fn`, not a closure, because a function pointer is all a host needs and it keeps this
/// `static` free of lifetime questions about who owns the callback.
pub fn set_log_sink(sink: fn(&str)) {
    *LOG_SINK.lock().unwrap() = Some(sink);

    // A panic on this target is not a crash report, it is the process simply ending: there is no
    // console, the `log` crate is compiled out, and the shell only ever sees that the callback it
    // invoked never came back. Every `unwrap` on a chart that came back unlike what was expected -
    // a missing id, a truncated json, a note index past the end of its line - therefore looked
    // exactly like "opening this chart closes the game", with no line anywhere to say which one.
    // This hook is the only account of a panic that survives the unwind across the FFI boundary,
    // so it is installed the moment the sink that has to receive it exists.
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            String::from("non-string panic payload")
        };
        let at = match info.location() {
            Some(l) => format!("{}:{}", l.file(), l.line()),
            None => String::from("?"),
        };
        crate::log_line(&format!("PANIC {at}: {msg}"));
    }));
}

/// Forward a line to the host, if one was installed.
///
/// Why this exists rather than `tracing`: the `log` feature is off for this target, so every
/// `info!`/`warn!` in phira and prpr is dropped - and even a working subscriber would say nothing
/// about *phases*. The host's `PhiraBoot` is one uninterrupted call on the UI thread, so the only
/// account of what it was doing when the process vanished is what Rust wrote down as it went.
/// Silent when no sink is installed, so it costs nothing anywhere else.
pub fn log_line(msg: &str) {
    if let Some(sink) = *LOG_SINK.lock().unwrap() {
        sink(msg);
    }
}

mod dir {
    use anyhow::Result;

    use crate::{CACHE_DIR, DATA_PATH};

    fn ensure(s: &str) -> Result<String> {
        let s = format!("{}/{}", DATA_PATH.lock().unwrap().as_ref().map(|it| it.as_str()).unwrap_or("."), s);
        let path = std::path::Path::new(&s);
        if !path.exists() {
            if let Err(err) = std::fs::create_dir_all(path) {
                // The path is composed here from whatever the host handed in, so this is the one
                // place a malformed directory can be named together with the name that was asked
                // for. `os error 123` arrived in the report with neither, and a caller that only
                // ever sees `Err` cannot tell a bad path from a full disk.
                crate::log_line(&format!("dir: create '{s}' failed: {err}"));
                return Err(err.into());
            }
        }
        Ok(s)
    }

    pub fn cache() -> Result<String> {
        if let Some(cache) = &*CACHE_DIR.lock().unwrap() {
            ensure(cache)
        } else {
            ensure("cache")
        }
    }

    pub fn bold_font_path() -> Result<String> {
        Ok(format!("{}/bold.ttf", root()?))
    }

    pub fn cache_image_local() -> Result<String> {
        // `cache()` hands back an already *rooted* path - `ensure` prefixed `DATA_PATH` on the way
        // out - so feeding it straight back into `ensure` prefixed it a second time, giving
        // `{DATA_PATH}/{DATA_PATH}/cache/image`. With a rooted `DATA_PATH` that puts a second
        // `C:` in the middle of the path, which Windows rejects outright as os error 123: the
        // error reported after a chart download. It is also why every cover went black.
        // `thumbnail_path` builds on this, so `Path::exists` said no, the JPEG save then failed
        // with the same 123, the illustration task returned Err, and `Illustration::settle` had
        // only a silent `warn!` for it. The folder still has to exist, so it is created here -
        // once, without re-entering `ensure`.
        let s = format!("{}/image", cache()?);
        let path = std::path::Path::new(&s);
        if !path.exists() {
            std::fs::create_dir_all(path)?;
        }
        Ok(s)
    }

    pub fn root() -> Result<String> {
        ensure("data")
    }

    pub fn charts() -> Result<String> {
        ensure("data/charts")
    }

    pub fn collections() -> Result<String> {
        ensure("data/collections")
    }

    pub fn custom_charts() -> Result<String> {
        ensure("data/charts/custom")
    }

    pub fn downloaded_charts() -> Result<String> {
        ensure("data/charts/download")
    }

    pub fn respacks() -> Result<String> {
        ensure("data/respack")
    }
}

async fn the_main() -> Result<()> {
    // `log::register()` - the call that installed `prpr`'s terminal tracing subscriber - is gone
    // with it: the `log` feature is off for this target (see `phira/Cargo.toml`), and WP8.1 has no
    // terminal to receive ANSI-coloured output. Nothing else in the workspace called it.
    #[cfg(target_env = "ohos")]
    {
        *DATA_PATH.lock().unwrap() = Some("/data/storage/el2/base".to_owned());
        *CACHE_DIR.lock().unwrap() = Some("/data/storage/el2/base/cache".to_owned());
        prpr::core::DPI_VALUE.store(250, std::sync::atomic::Ordering::Relaxed);
    };

    init_assets();
    log_line("the_main: assets registered");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();
    log_line("the_main: tokio runtime up (4 workers)");

    #[cfg(target_os = "ios")]
    {
        use objc2_foundation::{NSSearchPathDirectory, NSSearchPathDomainMask, NSSearchPathForDirectoriesInDomains};

        let directories = NSSearchPathForDirectoriesInDomains(NSSearchPathDirectory::LibraryDirectory, NSSearchPathDomainMask::UserDomainMask, true);
        let path = directories.firstObject().unwrap().to_string();
        *DATA_PATH.lock().unwrap() = Some(path);
        *CACHE_DIR.lock().unwrap() = Some("Caches".to_owned());
    }

    let dir = dir::root()?;
    log_line(&format!("the_main: data root = {dir}"));
    let mut data: Data = std::fs::read_to_string(format!("{dir}/data.json"))
        .map_err(anyhow::Error::new)
        .and_then(|s| Ok(serde_json::from_str(&s)?))
        .unwrap_or_default();
    log_line("the_main: data.json read (or defaulted)");
    data.init().await?;
    log_line("the_main: Data::init scanned the chart library");
    set_data(data);
    sync_data();
    save_data()?;
    log_line("the_main: data stored");

    // Warm up the offline banned-word automaton so local edits can check
    // synchronously. No-op without the `aa` feature.
    tokio::spawn(censor::preload());

    let rx = {
        let (tx, rx) = mpsc::channel();
        *MESSAGES_TX.lock().unwrap() = Some(tx);
        rx
    };

    unsafe { get_internal_gl() }
        .quad_context
        .display_mut()
        .set_pause_resume_listener(on_pause_resume);

    log_line("the_main: loading phigros.ttf + font.ttf");
    let pgr_font = FontArc::try_from_vec(load_file("phigros.ttf").await?)?;
    PGR_FONT.with(move |it| *it.borrow_mut() = Some(TextPainter::new(pgr_font, None)));

    let font = FontArc::try_from_vec(load_file("font.ttf").await?)?;
    let mut painter = TextPainter::new(font.clone(), None);

    log_line("the_main: fonts ok, building MainScene");
    let mut main = Main::new(Box::new(MainScene::new(font).await?), TimeManager::default(), None).await?;
    log_line("the_main: MainScene built, entering the frame loop");

    let tm = TimeManager::default();
    let mut fps_time = -1;

    const FPS_BUF_SIZE: usize = 60;
    let mut fps_times = VecDeque::<f32>::with_capacity(FPS_BUF_SIZE);
    let mut last_frame_start = f32::NAN;
    let mut fps_time_sum = 0.;
    // Phira's first iteration is where the auto-opened song page is entered and the chart launch
    // is spawned onto a tokio worker, so it is the one frame worth naming: if the process dies
    // between this line and the pump's own "first frame reached", the work that killed it is
    // inside `update`/`render` or inside that task.
    let mut frame_no: u32 = 0;

    'app: loop {
        if main.paused() {
            match rx.recv() {
                Ok(false) => {
                    main.resume()?;
                }
                Ok(true) => {}
                Err(_) => break 'app,
            }
        }

        let frame_start = tm.real_time();
        if !last_frame_start.is_nan() {
            if fps_times.len() == FPS_BUF_SIZE {
                fps_time_sum -= fps_times.pop_front().unwrap();
            }
            let frame_time = frame_start as f32 - last_frame_start;
            fps_times.push_back(frame_time);
            fps_time_sum += frame_time;
        }
        last_frame_start = frame_start as f32;
        if frame_no == 0 {
            log_line("the_main: first frame: update() + render()");
        }
        frame_no += 1;

        // A heartbeat, one line every ~30 frames.
        //
        // Everything before this point is logged exactly once, during boot, and then the frame
        // loop falls silent - so "the chart is playing and nothing new happens" and "the pump
        // stopped at the last frame we ever wrote" produced byte-identical output. That is not a
        // hypothetical: it is how a round in which the app was merely idle was read as a crash.
        //
        // `frame_start` is the same clock the FPS bookkeeping a few lines up already reads, so the
        // line also settles whether time is advancing at all - `set_now_micros` has no caller in
        // this shim, and a frozen clock is indistinguishable from a frozen scene without a number.
        // `scenes` is the stack depth (1 = main menu, 2 = song page pushed, 3 = in the chart), and
        // `paused` is the one that would otherwise explain a stack which is deep but not moving.
        // The `Scene` trait has no name to print, so depth is what identifies the scene.
        if frame_no % 30 == 0 {
            log_line(&format!(
                "the_main: frame={} t={:.3} scenes={} paused={}",
                frame_no,
                frame_start,
                main.scenes.len(),
                main.paused()
            ));
        }
        let res = || -> Result<()> {
            main.update()?;
            main.render(&mut painter)?;
            if let Ok(paused) = rx.try_recv() {
                if paused {
                    main.pause()?;
                } else {
                    main.resume()?;
                }
            }
            prpr::ext::flush_pending_texture_deletions();
            Ok(())
        }();
        if let Err(err) = res {
            error!("uncaught error: {err:?}");
            show_error(err);
        }
        if main.should_exit() {
            break 'app;
        }

        let t = tm.real_time();

        let fps_now = t as i32;
        if fps_now != fps_time {
            fps_time = fps_now;
            if fps_times.len() == FPS_BUF_SIZE {
                let actual_fps = 1. / (fps_time_sum / FPS_BUF_SIZE as f32);
                let current_fps = 1. / (t - frame_start);
                info!("FPS {} (capped at {})", current_fps as u32, actual_fps as u32);
            }
        }

        // While backgrounded the scene is paused; the blocking `recv_timeout`
        // above already parks this thread, so nothing extra is needed here.
        next_frame().await;
    }
    Ok(())
}

fn build_global_window_conf() -> Conf {
    let mut conf = build_conf();
    conf.window_title = "Phira".to_owned();
    conf.icon = Some(miniquad::conf::Icon {
        small: *include_bytes!("../icon/small"),
        medium: *include_bytes!("../icon/medium"),
        big: *include_bytes!("../icon/big"),
    });

    #[cfg(target_os = "windows")]
    {
        conf.fullscreen = dir::root()
            .ok()
            .and_then(|r| std::fs::read_to_string(std::path::Path::new(&r).join("data.json")).ok())
            .and_then(|s| serde_json::from_str::<Data>(&s).ok())
            .is_some_and(|d| d.config.fullscreen_mode);
    }

    conf
}

#[no_mangle]
pub extern "C" fn quad_main() {
    macroquad::Window::from_config(build_global_window_conf(), async {
        if let Err(err) = the_main().await {
            error!(?err, "global error");
        }
    });
    cleanup_audio();
}

/// Boot phira for a host that pumps frames itself.
///
/// This is [`quad_main`] minus `cleanup_audio()`, and the difference is load-bearing.
/// `quad_main` is macroquad's shape, where `from_config` runs the app *to completion*, so the call
/// after it is an exit hook. The shell's `from_config` only stores the future - `the_main` then
/// runs one loop iteration per `wp81_game_frame` - so returning from it means "booted", not
/// "exited".
///
/// Running `cleanup_audio()` at that moment would be the **first** access to `UI_AUDIO`, whose
/// `thread_local` initializer *builds* the audio manager (`prpr/src/ui.rs:1405`); taking it
/// straight back out would leave `SafeAudio(None)` for the rest of the session, which is UI audio
/// silently dead rather than any visible failure. `cleanup_audio` is a genuine shutdown hook - it
/// exists so the manager is dropped before thread-local destructors run - and this program never
/// shuts down: the process dies with the app.
pub fn boot() {
    macroquad::Window::from_config(build_global_window_conf(), async {
        if let Err(err) = the_main().await {
            // Recorded as well as logged. The `error!` below is dropped on this target - there is
            // no subscriber - so without this the host would be handed "exited before its first
            // frame" standing in for every cause `the_main` can return, and there are at least
            // three it hits before it draws anything (the data directory, the tokio runtime, the
            // fonts).
            set_boot_error(format!("{err:#}"));
            error!(?err, "global error");
        }
    });
}

fn on_pause_resume(pause: bool) {
    if let Some(tx) = MESSAGES_TX.lock().unwrap().as_mut() {
        let _ = tx.send(pause);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_initializeEnvironment(env: EnvUnowned, _class: JClass) {
    unsafe {
        inputbox::backend::Android::initialize_raw(env.as_raw()).unwrap();
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_prprActivityOnPause(_env: EnvUnowned, _class: JClass) {
    if let Some(tx) = MESSAGES_TX.lock().unwrap().as_mut() {
        let _ = tx.send(true);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_prprActivityOnResume(_env: EnvUnowned, _class: JClass) {
    if let Some(tx) = MESSAGES_TX.lock().unwrap().as_mut() {
        let _ = tx.send(false);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_prprActivityOnDestroy(_env: EnvUnowned, _class: JClass) {
    std::process::exit(0);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_setDataPath(_env: EnvUnowned, _class: JClass, path: JString) {
    *DATA_PATH.lock().unwrap() = Some(path.to_string());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_setTempDir(_env: EnvUnowned, _class: JClass, path: JString) {
    let path = path.to_string();
    std::env::set_var("TMPDIR", path.clone());
    *CACHE_DIR.lock().unwrap() = Some(path);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_setDpi(_env: EnvUnowned, _class: JClass, dpi: jint) {
    prpr::core::DPI_VALUE.store(dpi as _, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_setChosenFile(_env: EnvUnowned, _class: JClass, file: JString) {
    use prpr::scene::CHOSEN_FILE;
    CHOSEN_FILE.lock().unwrap().1 = Some(file.to_string());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_setDeepLink(_env: EnvUnowned, _class: JClass, url: JString) {
    deeplink::set_deeplink(url.to_string());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_markImport(_env: EnvUnowned, _class: JClass) {
    use prpr::scene::CHOSEN_FILE;

    CHOSEN_FILE.lock().unwrap().0 = Some("_import".to_owned());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_markImportRespack(_env: EnvUnowned, _class: JClass) {
    use prpr::scene::CHOSEN_FILE;

    CHOSEN_FILE.lock().unwrap().0 = Some("_import_respack".to_owned());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_setInputText(_env: EnvUnowned, _class: JClass, text: JString) {
    use prpr::scene::INPUT_TEXT;
    INPUT_TEXT.lock().unwrap().1 = Some(text.to_string());
}

/// Credentials obtained from the native HYKB (好游快爆) login SDK.
pub struct HykbCredential {
    /// SDK result code: 0 on success, otherwise an error / user cancellation.
    pub code: i32,
    pub uid: i64,
    pub nick: String,
    pub access_token: String,
}

impl HykbCredential {
    /// Map the SDK result code to an error, or yield the credential on success.
    /// Centralizes the code → user-facing message translation shared by every
    /// HYKB login/bind entry point.
    #[cfg(feature = "hykb")]
    pub fn ok_or_err(self) -> Result<Self> {
        if self.code == 0 {
            Ok(self)
        } else {
            // A non-zero code is any failure the HYKB SDK reports: 2001 auth
            // failed, 2002 login failed, 2003 cancelled, 2004 exception, 2005
            // developer-requested exit / account logout. A HYKB build mandates a
            // valid, matching HYKB session, so every one of these must tear the
            // in-game session down — otherwise cancelling the HYKB prompt during
            // a silent re-verify would leave the player signed in and bypass the
            // gate entirely.
            force_logout();
            anyhow::bail!("{}", crate::ttl!("hykb-login-cancelled"))
        }
    }
}

/// Slot for the pending HYKB login result. The native callback fulfills it.
static HYKB_TX: Mutex<Option<tokio::sync::oneshot::Sender<HykbCredential>>> = Mutex::new(None);

/// Call a no-arg `void` method on the Android host activity (the HYKB shell).
#[cfg(all(target_os = "android", feature = "hykb"))]
fn call_activity_void(method: &'static jni::strings::JNIStr) {
    use jni::{jni_sig, objects::JObject, vm::JavaVM};

    JavaVM::singleton()
        .unwrap()
        .attach_current_thread(|env| -> jni::errors::Result<()> {
            let ctx = unsafe { JObject::from_raw(env, ndk_context::android_context().context() as _) };
            env.call_method(ctx, method, jni_sig!("()V"), &[])?;
            Ok(())
        })
        .unwrap();
}

/// Ask the Android shell to pop the HYKB account picker (`MainActivity.hykbSwitchAccount`).
/// Used by the explicit login / switch-account flow.
#[cfg(all(target_os = "android", feature = "hykb"))]
fn request_hykb_login() {
    call_activity_void(jni::jni_str!("hykbSwitchAccount"));
}

#[cfg(not(all(target_os = "android", feature = "hykb")))]
fn request_hykb_login() {}

/// Ask the Android shell to sign in using the cached HYKB account without
/// popping the picker (`MainActivity.hykbLogin`). The credentials the SDK
/// reports flow back through `HYKB_TX`, so the caller can verify them against
/// the restored Phira session. Used by the silent startup restore.
#[cfg(all(target_os = "android", feature = "hykb"))]
fn request_hykb_login_silent() {
    call_activity_void(jni::jni_str!("hykbLogin"));
}

#[cfg(not(all(target_os = "android", feature = "hykb")))]
fn request_hykb_login_silent() {}

/// Tell the native HYKB SDK to sign out (`MainActivity.hykbLogout`). Called when the
/// player logs out from their profile.
#[cfg(all(target_os = "android", feature = "hykb"))]
pub fn hykb_logout() {
    call_activity_void(jni::jni_str!("hykbLogout"));
}

#[cfg(not(all(target_os = "android", feature = "hykb")))]
pub fn hykb_logout() {}

/// Tear down the local session: sign out of the native HYKB SDK, clear the
/// stored account and tokens, then re-sync. Shared by every path that must
/// reject a login — a failed/cancelled HYKB verification, a uid mismatch, or
/// the player logging out from their profile.
pub fn force_logout() {
    hykb_logout();
    get_data_mut().me = None;
    get_data_mut().tokens = None;
    let _ = save_data();
    sync_data();
}

/// Trigger the native HYKB login and await its credentials.
#[allow(unused)]
pub async fn obtain_hykb_credential() -> Result<HykbCredential> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    *HYKB_TX.lock().unwrap() = Some(tx);
    request_hykb_login();
    let cred = rx.await.map_err(|_| anyhow::anyhow!("hykb login cancelled"))?;
    Ok(cred)
}

/// Silently restore the HYKB session from the cached account and await its
/// credentials. Unlike [`obtain_hykb_credential`], this does not pop the account
/// picker; used by the blocking startup check to verify the restored session.
#[allow(unused)]
pub async fn obtain_hykb_credential_silent() -> Result<HykbCredential> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    *HYKB_TX.lock().unwrap() = Some(tx);
    request_hykb_login_silent();
    let cred = rx.await.map_err(|_| anyhow::anyhow!("hykb login cancelled"))?;
    Ok(cred)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_hykbLoginCallback(
    _env: EnvUnowned,
    _class: JClass,
    code: jint,
    uid: jni::sys::jlong,
    nick: JString,
    access_token: JString,
) {
    let nick = if nick.is_null() { String::new() } else { nick.to_string() };
    let access_token = if access_token.is_null() {
        String::new()
    } else {
        access_token.to_string()
    };
    if let Some(tx) = HYKB_TX.lock().unwrap().take() {
        let _ = tx.send(HykbCredential {
            code: code as i32,
            uid: uid as i64,
            nick,
            access_token,
        });
    } else if code == 2005 {
        // No login is in flight, so this is the SDK's asynchronous
        // anti-addiction "exit game" action: the player hit a play-time limit
        // and chose to quit from the SDK's own dialog. Honor it by exiting.
        // Other async codes (e.g. 2008 "continue playing") are handled inside
        // the SDK and need no response here. A request-less success (code 0, the
        // SDK switching accounts on its own) is likewise ignored: any signed-in
        // HYKB account is accepted, so a switch no longer tears the session down.
    }
}

#[cfg(target_env = "ohos")]
#[napi]
pub fn set_input_text(text: String) {
    use prpr::scene::INPUT_TEXT;
    INPUT_TEXT.lock().unwrap().1 = Some(text);
}

#[cfg(target_env = "ohos")]
#[napi]
pub fn set_chosen_file(file: String) {
    use prpr::scene::CHOSEN_FILE;
    CHOSEN_FILE.lock().unwrap().1 = Some(file);
}

#[cfg(target_env = "ohos")]
#[napi]
pub fn mark_auto_import() {
    use prpr::scene::CHOSEN_FILE;
    CHOSEN_FILE.lock().unwrap().0 = Some("_import_auto".to_owned());
}

#[cfg(target_env = "ohos")]
#[napi]
pub fn on_foreground() {
    if let Some(tx) = MESSAGES_TX.lock().unwrap().as_mut() {
        let _ = tx.send(false);
    }
}

#[cfg(target_env = "ohos")]
#[napi]
pub fn on_background() {
    if let Some(tx) = MESSAGES_TX.lock().unwrap().as_mut() {
        let _ = tx.send(true);
    }
}
