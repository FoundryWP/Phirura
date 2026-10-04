//! The last byte of this image's TLS template.
//!
//! The mirror image of `tlsdir_start.rs`: `EndAddressOfRawData` has to reach the far end of
//! `.tls`, and contributions to `.tls$` are laid out in input order, so this marker is compiled to
//! its own object and linked as the *last* input - after `prpr_port.lib`, which is where the
//! 0xa0c bytes of real `#[thread_local]` data come from.
//!
//! The directory points `end_address_of_raw_data` *at* this byte rather than past it, excluding it
//! from the template. That is what makes it safe for this to be our own byte: nothing real is
//! dropped, and the alternative was pointer arithmetic inside a `static` initializer for one byte
//! of padding.
//!
//! The placement is not taken on trust. `check_tls.py` reads the byte back out of the finished PE
//! and fails the build unless `End` reaches the end of `.tls` and the value here is `0x5a`.

#![no_std]

#[link_section = ".tls$"]
#[no_mangle]
pub static _tls_end: u8 = 0x5a;
