//! An `rfd` shim for Windows Phone 8.1.
//!
//! # Why
//!
//! `rfd` is the native file-dialog crate. On Windows it binds the Win32 common dialogs through
//! `windows-sys` - and `windows-sys` is the **only** crate in the whole phira dependency graph that
//! fails to compile for this target. Its `Win32_UI_Shell` module references types (`APPBARDATA`,
//! `SHFILEOPSTRUCTA/W`, `SHFILEINFOA/W`, `SHSTOCKICONINFO`, `SHQUERYRBINFO`, `NOTIFYICONDATAA`)
//! that its own feature gating does not define, so the module cannot be built at all.
//!
//! # What is needed
//!
//! Exactly two calls in the entire workspace:
//!
//! * `prpr::scene::choose_file` - `FileDialog::new().pick_file()`
//! * `phira::page::library`   - `FileDialog::new().set_title(..).set_file_name(..).save_file()`
//!
//! WP8.1 has its own pickers, and the C# side already drives `FileOpenPicker` for chart import, so
//! these forward there.
//!
//! # Why `PathBuf` and not a handle
//!
//! Both call sites immediately turn the result into a display string and hand it to the chart
//! loader, which then opens it by path. The shell copies the picked file into the app's local
//! folder and returns that path, so a plain `PathBuf` is the honest type - no new abstraction is
//! needed at the boundary.

#![allow(unused_variables)]

use std::path::PathBuf;

/// The shell bridge. Declared here, implemented by the C# side.
///
/// Nothing in this crate links until the final staticlib/DLL, so an rlib build succeeds without
/// them - which is what lets the shim be verified on its own.
mod shell {
    extern "C" {
        /// Show the file picker. Returns a UTF-16 path written into `out`, or 0 if cancelled.
        ///
        /// `out` must have room for `cap` UTF-16 units.
        pub fn wp81_pick_file(title: *const u16, out: *mut u16, cap: usize) -> i32;

        /// Show the save dialog. Same contract as `wp81_pick_file`.
        pub fn wp81_save_file(
            title: *const u16,
            suggested: *const u16,
            out: *mut u16,
            cap: usize,
        ) -> i32;
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn from_wide(buf: &[u16]) -> Option<PathBuf> {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    if end == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..end]);
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

#[derive(Clone, Debug)]
pub struct FileDialog {
    title: String,
    file_name: String,
    directory: Option<PathBuf>,
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl FileDialog {
    pub fn new() -> Self {
        FileDialog {
            title: String::new(),
            file_name: String::new(),
            directory: None,
        }
    }

    pub fn set_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = name.into();
        self
    }

    pub fn set_directory(mut self, dir: impl Into<PathBuf>) -> Self {
        self.directory = Some(dir.into());
        self
    }

    /// Accepted and ignored. `prpr` and `phira` never call it; it is here so a future call site
    /// does not silently fail to compile.
    pub fn add_filter(mut self, _name: impl Into<String>, _ext: &[&str]) -> Self {
        self
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        let title = to_wide(&self.title);
        let mut buf = vec![0u16; 1024];
        let ok = unsafe { shell::wp81_pick_file(title.as_ptr(), buf.as_mut_ptr(), buf.len()) };
        if ok == 0 {
            None
        } else {
            from_wide(&buf)
        }
    }

    /// `prpr`'s call site is singular. Present so the plural form would compile if used.
    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        self.pick_file().map(|p| vec![p])
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        // The WP8.1 picker browses files, not folders; `phira` does not call this.
        None
    }

    pub fn save_file(self) -> Option<PathBuf> {
        let title = to_wide(&self.title);
        let suggested = to_wide(&self.file_name);
        let mut buf = vec![0u16; 1024];
        let ok = unsafe {
            shell::wp81_save_file(title.as_ptr(), suggested.as_ptr(), buf.as_mut_ptr(), buf.len())
        };
        if ok == 0 {
            None
        } else {
            from_wide(&buf)
        }
    }

    pub async fn pick_file_async(self) -> Option<PathBuf> {
        self.pick_file()
    }

    pub async fn save_file_async(self) -> Option<PathBuf> {
        self.save_file()
    }
}

/// Upstream also exposes free functions; kept so call sites written against them compile.
pub fn pick_file() -> Option<PathBuf> {
    FileDialog::new().pick_file()
}

pub fn save_file() -> Option<PathBuf> {
    FileDialog::new().save_file()
}
