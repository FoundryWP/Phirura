pub mod bin;
pub mod config;
pub mod core;
pub mod dir;
pub mod ext;
pub mod fs;
pub mod info;
pub mod judge;
pub mod parse;
pub mod particle;
pub mod scene;
pub mod task;
pub mod time;
pub mod ui;

#[cfg(feature = "log")]
pub mod log;

#[rustfmt::skip]
#[cfg(closed)]
pub mod inner;

use std::sync::Mutex;

pub use scene::Main;

/// The chart's own music file, relative to phira's data root, as of the last launch.
///
/// `Resource::new` opens the soundtrack with `fs.load_file(&info.music)` and hands the bytes to an
/// `AudioClip` that carries no path, so nothing downstream of it - least of all the shell, which
/// sees a clip id of 0 and a duration - could say which file it was. The shell therefore matched
/// the duration against the one chart it had resolved before boot: right for the chart packaged
/// with the app, wrong for every other one. A downloaded track measured 274.9 s against the
/// packaged chart's 214.8 s, missed, and the res pack's `ending` clip was played in its place -
/// a chart with its music sitting on disk, playing nothing.
///
/// `phira` knows the path (`global_launch` holds the `local_path` and has just loaded `info`) and
/// `prpr` is the crate that opens the file, which is the direction this already depends on, so
/// this static is the whole channel: no new callback, no new field, nothing for the shell to keep
/// in step with.
///
/// Written on every launch rather than cleared on exit. A stale value is only ever read by the
/// next launch, which overwrites it before `Resource::new` runs.
static MUSIC_SOURCE: Mutex<Option<String>> = Mutex::new(None);

/// Publishes the music file the next `Resource::new` should name its clip with. `None` when the
/// launch has no local file to point at.
pub fn set_music_source(path: Option<String>) {
    *MUSIC_SOURCE.lock().unwrap() = path;
}

/// [`set_music_source`]'s path, copied out so the caller does not hold the lock.
pub fn music_source() -> Option<String> {
    MUSIC_SOURCE.lock().unwrap().clone()
}

pub fn build_conf() -> macroquad::window::Conf {
    macroquad::window::Conf {
        window_title: "Phira".to_string(),
        window_width: 973,
        window_height: 608,
        ..Default::default()
    }
}
