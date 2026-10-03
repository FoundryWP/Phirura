//! Milestone 4 probe: can Rust produce an ARM staticlib that actually links into a WP8.1
//! binary?
//!
//! It uses `std` the way prpr does (HashMap, String, Vec, file I/O, threads, the monotonic
//! clock, OS randomness) and forces every symbol from `wp81-compat` to be included, so the
//! resulting archive is a faithful stand-in for what prpr will produce.
//!
//! The point is not to do anything useful - it is to give the ARM linker something real to
//! chew on, so that the import table of the linked binary can be inspected.

use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

// ===========================================================================
// Panic reporting.
//
// This exists because everything above it was guesswork. A Rust panic with `panic = "abort"` kills
// the process with 0xC0000409 and prints *nothing*: the default hook writes to stderr, and a
// WP8.1 app has no stderr. So a failing `.unwrap()` is indistinguishable from a segfault, and the
// only way to find it is to reason backwards from the source - which is exactly the process that
// produced three wrong theories in a row (threads, then `QueryPerformanceFrequency`, then
// `__rt_udiv64`).
//
// The hook is called *before* the abort, even with `panic = "abort"` - that is the one thing that
// makes this work. It appends the panic location and message to the same phirura.log the C# side
// already writes, so the next device run names the failure outright.
// ===========================================================================

/// Path of the log file to append panics to. Set from C#, because the DLL has no idea where the
/// app's LocalFolder is.
static PANIC_LOG_PATH: Mutex<Option<String>> = Mutex::new(None);

/// Called once from C# with `ApplicationData\LocalFolder\phirura.log`.
///
/// # Safety
/// `path` must be a NUL-terminated UTF-16 string.
#[no_mangle]
pub unsafe extern "C" fn wp81_set_log_path(path: *const u16) {
    if path.is_null() {
        return;
    }
    let mut units: Vec<u16> = Vec::new();
    let mut i = 0isize;
    loop {
        let c = *path.offset(i);
        if c == 0 || i > 4096 {
            break;
        }
        units.push(c);
        i += 1;
    }
    if let Ok(mut guard) = PANIC_LOG_PATH.lock() {
        *guard = Some(String::from_utf16_lossy(&units));
    }
}

#[no_mangle]
pub extern "C" fn wp81_install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let mut msg = String::from("RUST PANIC: ");

        if let Some(loc) = info.location() {
            msg.push_str(loc.file());
            msg.push(':');
            msg.push_str(&loc.line().to_string());
            msg.push_str(": ");
        } else {
            msg.push_str("(no location): ");
        }

        // `panic!("literal")` carries `&'static str`; `panic!("{}", x)` carries `String`.
        let payload = info.payload();
        if let Some(s) = payload.downcast_ref::<&str>() {
            msg.push_str(s);
        } else if let Some(s) = payload.downcast_ref::<String>() {
            msg.push_str(s);
        } else {
            msg.push_str("(non-string panic payload)");
        }
        msg.push_str("\r\n");

        if let Ok(guard) = PANIC_LOG_PATH.lock() {
            if let Some(p) = guard.as_ref() {
                use std::io::Write;
                if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) {
                    let _ = f.write_all(msg.as_bytes());
                    // Explicit: the process is about to abort, so nothing may rely on Drop.
                    let _ = f.flush();
                }
            }
        }
    }));
}

/// Force every `#[no_mangle]` shim from `wp81-compat` into the archive.
///
/// rustc only pulls objects out of a dependency rlib when something references them, and these
/// symbols are referenced from *patched std*, not from here - so without an explicit anchor
/// they can be dropped and the link then fails with unresolved externals.
///
/// The addresses are taken at *runtime*, not in a `static`: a `static` initialiser is const
/// evaluated, and pointers have no integer value there. `#[no_mangle]` keeps this function
/// itself alive. None of the shims are called - invoking `NtCreateFile` with null arguments
/// would be meaningless.
#[no_mangle]
pub extern "C" fn wp81_anchor_compat() -> usize {
    let anchors: [usize; 19] = [
        wp81_compat::GetConsoleMode as usize,
        wp81_compat::GetConsoleOutputCP as usize,
        wp81_compat::ReadConsoleW as usize,
        wp81_compat::WriteConsoleW as usize,
        wp81_compat::GetStdHandle as usize,
        wp81_compat::SetStdHandle as usize,
        wp81_compat::GetCommandLineW as usize,
        wp81_compat::GetEnvironmentStringsW as usize,
        wp81_compat::FreeEnvironmentStringsW as usize,
        wp81_compat::GetUserProfileDirectoryW as usize,
        wp81_compat::WaitForMultipleObjects as usize,
        wp81_compat::ProcessPrng as usize,
        wp81_compat::NtCreateFile as usize,
        wp81_compat::NtOpenFile as usize,
        wp81_compat::NtReadFile as usize,
        wp81_compat::NtWriteFile as usize,
        wp81_compat::NtSetInformationFile as usize,
        wp81_compat::NtCreateNamedPipeFile as usize,
        wp81_compat::RtlNtStatusToDosError as usize,
    ];
    let mut acc = 0usize;
    for a in anchors {
        acc ^= a;
    }
    acc
}

/// The entry point the C++/CX shell would call.
///
/// Takes a STEP and performs exactly one operation, so a crash can be localised without a
/// debugger. Each step is something prpr actually depends on, and the caller walks the steps in
/// order and logs after each one - the last step that logged before the process died is the
/// culprit. Doing it this way costs one device round trip instead of one per suspect.
#[no_mangle]
pub extern "C" fn wp81_link_probe(step: u32) -> i64 {
    match step {
        // 0: can the function be entered at all
        0 => 0,

        // 1: allocator - HashMap / String / Vec
        1 => {
            let mut map: HashMap<String, Vec<f64>> = HashMap::new();
            map.insert(String::from("notes"), vec![1.0, 2.0, 3.0]);
            map.values().map(|v| v.len() as i64).sum()
        }

        // 2: the raw QueryPerformanceCounter call on its own, no Rust clock machinery
        2 => unsafe { wp81_compat::wp81_raw_qpc() },

        // 14: the frequency `std`'s Instant needs, straight from the compat shim. Returned raw so
        //     the log tells us the device's real tick rate - worth knowing on its own, because if
        //     the platform's own answer is unusable then this is the *measured* fallback, and a
        //     wrong number would make every duration in the game wrong.
        14 => {
            let mut f: i64 = 0;
            unsafe { wp81_compat::QueryPerformanceFrequency(&mut f) };
            f
        }

        // 6: the full Rust Instant, which additionally initialises a `Once` for the frequency
        6 => {
            let t0 = Instant::now();
            t0.elapsed().as_millis() as i64
        }

        // 15: `Instant::now()` on its own. It needs `frequency()` AND `mul_div_u64`, which is
        //     `value / denom` + `value % denom` on u64 - i.e. the `__rt_udiv64` helper.
        15 => {
            let _t = Instant::now();
            1
        }

        // 16: adds `elapsed()`, which goes through `epsilon()` -> `NANOS_PER_SEC / frequency`
        //     (another u64 division) and the Duration subtraction.
        16 => {
            let t0 = Instant::now();
            t0.elapsed().as_millis() as i64
        }

        // 17: pure Duration arithmetic with no Instant involved - isolates the u128 division
        //     inside `as_millis`. Should be 1000.
        17 => Duration::from_secs(1).as_millis() as i64,

        // 18: raw u64 division. Should be 137759.
        //
        // EVERY OPERAND HERE GOES THROUGH black_box, and that is not decoration. The first version
        // of these steps used bare literals, LLVM folded them at compile time, and the helpers were
        // never called - so all four "expected value" tests passed while `__rt_udiv64` was calling
        // itself into a stack overflow. A test that the optimiser can answer is not a test.
        18 => {
            let a = std::hint::black_box(929_877_129_449u64);
            let b = std::hint::black_box(6_750_000u64);
            (a / b) as i64
        }

        // 19: raw u64 REMAINDER - the half of the helper's result that Rust cannot express.
        //     Should be 3879449.
        19 => {
            let a = std::hint::black_box(929_877_129_449u64);
            let b = std::hint::black_box(6_750_000u64);
            (a % b) as i64
        }

        // 20: raw u32 division, i.e. `__rt_udiv`. Should be 571428571.
        20 => {
            let a = std::hint::black_box(4_000_000_000u32);
            let b = std::hint::black_box(7u32);
            (a / b) as i64
        }

        // 20b: raw u32 remainder, which reads the OTHER half of `__rt_udiv`'s two-word result.
        //      Should be 6.
        24 => {
            let a = std::hint::black_box(4_000_000_000u32);
            let b = std::hint::black_box(7u32);
            (a % b) as i64
        }

        // 21: the `QueryPerformanceCounter` *trampoline* - the exact call std makes, as opposed to
        //     the resolver path step 2 uses. Returns the counter value, or -1 if the trampoline
        //     reported failure. `perf_counter::now()` does `cvt(..).unwrap()`, so a 0 here is a
        //     panic, and the panic is what kills the process.
        21 => {
            let mut v: i64 = 0;
            let ret = unsafe { wp81_compat::QueryPerformanceCounter(&mut v as *mut i64 as usize) };
            if ret == 0 { -1 } else { v }
        }

        // 22: `mul_div_u64` reproduced with the real operands - step 2's QPC reading and step 14's
        //     frequency - so the exact arithmetic `Instant::now()` performs runs on its own.
        //     Expected 138303315018222.
        22 => {
            let value: u64 = std::hint::black_box(933_547_376_373);
            let numerator: u64 = 1_000_000_000;
            let denom: u64 = std::hint::black_box(6_750_000);
            let q = value / denom;
            let r = value % denom;
            (q * numerator + r * numerator / denom) as i64
        }

        // 23: `Duration::from_nanos` with the value `Instant::now()` actually produces - a huge
        //     one, ~4.6e18, because std offsets the clock's epoch by `u64::MAX / 4` to keep
        //     timestamps away from zero. Expected 4611686018.
        23 => Duration::from_nanos(std::hint::black_box(4_611_686_018_427_387_903u64)).as_secs() as i64,

        // 3: file system - goes through the NtCreateFile / NtReadFile shims
        3 => match fs::read_to_string("probe.txt") {
            Ok(t) => t.len() as i64,
            Err(_) => -1,
        },

        // 4: OS randomness - ProcessPrng
        4 => {
            let rng = std::collections::hash_map::RandomState::new();
            use std::hash::{BuildHasher, Hasher};
            let mut h = rng.build_hasher();
            h.write_u32(12345);
            h.finish() as i64
        }

        // 5: threads - CreateThread. Last because it is the most likely to be restricted.
        5 => {
            let h = thread::spawn(|| 7i64);
            h.join().unwrap_or(-1)
        }

        // 7: raw CreateThread, with none of Rust's thread machinery
        7 => unsafe { wp81_compat::wp81_raw_createthread() },

        // 8: the first OS call `spawn_unchecked` makes - reading RUST_MIN_STACK. Isolates
        //    environment access from the thread machinery entirely.
        8 => match std::env::var_os("RUST_MIN_STACK") {
            Some(_) => 1,
            None => 0,
        },

        // 9-13: std's exact CreateThread arguments (2 MiB reservation +
        // STACK_SIZE_PARAM_IS_A_RESERVATION, as in `sys/thread/windows.rs`), with the routine
        // performing one risky operation each. 7 proved the OS will make a thread; these find out
        // which of the three things `thread_start` then does is fatal.
        //
        // The positive return values are the routine's own status codes, so a step that returns
        // e.g. 0x12 means it got all the way through and the resolver worked.
        9 => unsafe { wp81_compat::wp81_raw_createthread_ex(0x0001_0000, 2 * 1024 * 1024, 0) },
        10 => unsafe { wp81_compat::wp81_raw_createthread_ex(0x0001_0000, 2 * 1024 * 1024, 2) },
        11 => unsafe { wp81_compat::wp81_raw_createthread_ex(0x0001_0000, 2 * 1024 * 1024, 4) },
        12 => unsafe { wp81_compat::wp81_raw_createthread_ex(0x0001_0000, 2 * 1024 * 1024, 1) },
        13 => unsafe { wp81_compat::wp81_raw_createthread_ex(0x0001_0000, 2 * 1024 * 1024, 3) },

        _ => -1,
    }
}

// ---------------------------------------------------------------------------
// CRT-free DLL entry point.
//
// WP8.1 native DLLs must not import KERNEL32.dll directly - Win2D, which loads fine on the
// same device, imports it zero times and only uses the api-ms-win-* API sets. The desktop
// static CRT (libcmt.lib) drags in roughly 40 kernel32 functions from its startup code, which
// is what sank the first device test with ERROR_MOD_NOT_FOUND.
//
// Rust does not actually need the CRT: std allocates with HeapAlloc rather than malloc, and
// compiler_builtins supplies the mem* intrinsics when built with `compiler-builtins-mem`. So
// the entry point is supplied here and the link uses /NODEFAULTLIB.
// ---------------------------------------------------------------------------

/// The CRT normally defines this, and any code touching floating point references it. Without
/// the CRT it would be an unresolved external.
#[no_mangle]
pub static _fltused: i32 = 0;

#[no_mangle]
pub unsafe extern "system" fn DllMain(_module: *mut core::ffi::c_void, _reason: u32,
                                      _reserved: *mut core::ffi::c_void) -> i32 {
    // Deliberately NOT calling DisableThreadLibraryCalls: Rust std's thread-locals want the
    // thread attach/detach notifications to arrive.
    1 // TRUE
}

#[no_mangle]
pub unsafe extern "system" fn _DllMainCRTStartup(module: *mut core::ffi::c_void, reason: u32,
                                                 reserved: *mut core::ffi::c_void) -> i32 {
    DllMain(module, reason, reserved)
}
