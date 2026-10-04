//! The first byte of this image's TLS template, and the directory that points at it.
//!
//! # Why this is a separate file, compiled to its own object
//!
//! Thread-local storage in a `/NODEFAULTLIB` image does not work by itself, and it fails silently.
//! The Windows loader only gives this DLL a TLS index by walking data directory 9 and writing back
//! through `AddressOfIndex`. That directory comes from a structure called `_tls_used`, which is
//! normally supplied by the CRT's `tlssup.obj` (a member of `libcmt.lib` on this target). Dropping
//! the CRT drops it, so link.exe emits no directory, `_tls_index` stays 0, and `#[thread_local]`
//! resolves as `ThreadLocalStoragePointer[0]` - the *first* module's block, not this DLL's. Nothing
//! faults; the values are simply somebody else's.
//!
//! On the phone that appeared as `GameCreate -> AlreadyRunning` on a brand-new process, with
//! `running=True` and a frame count of `325577230000753816`, before anything had created a game.
//! Eight of `prpr`'s own `thread_local!`s and `std`'s are exposed the same way, which is why this
//! cannot be fixed by rewriting our own thread-locals - `prpr` is the real library.
//!
//! # Why it must be a direct object at the front of the link
//!
//! Two constraints meet here:
//!
//! * The loader copies `[StartAddressOfRawData, EndAddressOfRawData)` into each thread's block, and
//!   every access computes its offset as `secrel` - relative to the start of the section *holding
//!   the variable*. Those agree only if `Start` is exactly offset 0 of `.tls`. One byte out and
//!   every thread-local in the image reads shifted data.
//! * Contributions to `.tls$` are laid out in input order. Members of `prpr_port.lib` are added
//!   when the archive is scanned, so anything in that archive lands somewhere in the middle - the
//!   first attempt sat at offset 776 of 2572. A *direct* object is not resolved from an archive, it
//!   is included outright, and it comes before any library named after it on the command line.
//!
//! So this file exists to be one object, first on the line. `build-port.ps1` compiles it with
//! `rustc --emit=obj` and puts it there; `check_tls.py` then reads `_tls_start` back out of the
//! finished PE and fails the build if it is not at offset 0. The marker's section name has to be
//! `.tls$` exactly - a differently suffixed name (`.tls$zzz`) or plain `.tls` both land in an
//! ordinary data section instead of the template, which is how the first version ended up with two
//! sections called `.tls`.
//!
//! The matching end marker lives in `tlsdir_end.rs` and is linked last, for the mirror-image
//! reason: `End` must reach the far end of the template, and it is the last object that does.

#![no_std]

/// `IMAGE_TLS_DIRECTORY32` from `winnt.h` (Windows Kits 8.1): six `DWORD`s, 24 bytes.
///
/// The address fields are spelled as pointers rather than `u32` only so they can be formed in a
/// `static` initializer - Rust cannot const-evaluate a pointer-to-integer cast - and on a 32-bit
/// target that is byte-for-byte the same struct.
///
/// The `unsafe` in this file is exactly one `Sync` impl. The descriptor is written once at link
/// time and only ever read by the loader; the raw pointers in it are dereferenced by the loader,
/// not by us.
#[repr(C)]
pub struct ImageTlsDirectory32 {
    pub start_address_of_raw_data: *const u8,
    pub end_address_of_raw_data: *const u8,
    pub address_of_index: *mut u32,
    pub address_of_callbacks: *const usize,
    pub size_of_zero_fill: u32,
    pub characteristics: u32,
}

unsafe impl Sync for ImageTlsDirectory32 {}

/// Offset 0 of `.tls`: the byte every thread's block starts at.
///
/// `check_tls.py` asserts both the placement (`Start` must equal the section's address) and the
/// value, so a linker that stops sorting this first fails the build instead of producing an image
/// whose thread-locals all read shifted data on the phone.
#[link_section = ".tls$"]
#[no_mangle]
pub static _tls_start: u8 = 0xa5;

/// Null-terminated callback list, so the loader stops at the first entry instead of walking on.
/// A null `address_of_callbacks` would do as much; naming the array makes it checkable.
#[no_mangle]
pub static _tls_callbacks: usize = 0;

extern "C" {
    /// Owned by `wp81-compat`, which documents why it must not be initialised. The loader writes
    /// the image's TLS index here through `address_of_index` below; on its own it stays 0, which is
    /// precisely the failure this whole file exists to end.
    static mut _tls_index: u32;

    /// The far end of the template, from `tlsdir_end.rs`, linked as the last input.
    static _tls_end: u8;
}

/// Data directory 9 of the finished image.
#[no_mangle]
pub static _tls_used: ImageTlsDirectory32 = ImageTlsDirectory32 {
    start_address_of_raw_data: &_tls_start,
    // `addr_of!` / `addr_of_mut!` rather than `&_tls_end` / `&_tls_index`: taking a reference to an
    // extern static is `unsafe` and cannot be done in a `static` initializer, while the address-of
    // operators form the same pointer without creating one. That is what keeps this file down to a
    // single `unsafe` - the `Sync` impl above - with no `unsafe` block anywhere.
    end_address_of_raw_data: core::ptr::addr_of!(_tls_end),
    address_of_index: core::ptr::addr_of_mut!(_tls_index),
    address_of_callbacks: &_tls_callbacks,
    size_of_zero_fill: 0,
    characteristics: 0,
};
