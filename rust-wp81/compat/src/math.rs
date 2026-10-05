//! The C math library, for a link that has no CRT.
//!
//! # Why this file exists
//!
//! `core`'s float methods that have no ARM instruction in front of them lower into a call on the
//! C runtime. The first phira build proved it rather than assuming it - the link stopped at
//!
//! ```text
//! unresolved external symbol logf    (referenced from std::f32::ln)
//! unresolved external symbol log     (referenced from bytesize::display)
//! ```
//!
//! On any normal Windows target the CRT answers those. It cannot answer here: `/NODEFAULTLIB` is
//! deliberate, because the import gate exists precisely so this DLL links nothing but WP8.1 API
//! sets, and no CRT is one of them. So something in this crate has to be `logf`.
//!
//! # Why the whole set and not only the two that failed
//!
//! Below is every name `libm` exports that C's `<math.h>` also exports. Two of them are missing
//! today; the rest are here because each one discovered on a later build costs a full
//! minutes-long `-Zbuild-std` round trip to find, and they are all three lines apiece. The test
//! for inclusion is "does `<math.h>` export it", never "is something calling it" - the latter can
//! only be answered one build at a time, which is exactly the loop this is meant to close.
//!
//! Deliberately absent: `logb`, `nearbyint`, `drem`, `gamma`. `libm` does not export those names,
//! and writing an implementation for a symbol nobody has asked for is a way to be confidently
//! wrong about arithmetic.
//!
//! # Why `libm` and not `x.ln()`
//!
//! The shortest possible version of this file is
//!
//! ```ignore
//! #[no_mangle] pub extern "C" fn logf(x: f32) -> f32 { x.ln() }
//! ```
//!
//! and it recurses forever: on this target `f32::ln` *is* a call to `logf`, so it would call
//! straight back into itself. The `libm` crate is a `no_std` Rust port of the musl routines - it
//! computes the answer with its own arithmetic and never routes back through the symbol being
//! defined.
//!
//! # Why this is one file of macros
//!
//! The signatures fall into five shapes, and hand-writing ~140 near-identical functions would
//! bury the only interesting line in each of them - the `libm::` path - under boilerplate. Grouping
//! by shape also makes the ABI visible: `unary_f64` is a plain `double -> double`, which is what
//! a caller compiled against `<math.h>` expects, with no `extern "system"` in sight because the
//! only `system`-ABI symbols this crate exposes are the Windows ones.

/// `double -> double`
macro_rules! unary_f64 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(x: f64) -> f64 {
            libm::$name(x)
        }
    )*};
}

/// `float -> float`
macro_rules! unary_f32 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(x: f32) -> f32 {
            libm::$name(x)
        }
    )*};
}

/// `double, double -> double`
macro_rules! binary_f64 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(x: f64, y: f64) -> f64 {
            libm::$name(x, y)
        }
    )*};
}

/// `float, float -> float`
macro_rules! binary_f32 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(x: f32, y: f32) -> f32 {
            libm::$name(x, y)
        }
    )*};
}

/// `double, int -> double` - the two functions C puts an `int` second.
macro_rules! scaled_f64 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(x: f64, n: i32) -> f64 {
            libm::$name(x, n)
        }
    )*};
}

/// `float, int -> float`
macro_rules! scaled_f32 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(x: f32, n: i32) -> f32 {
            libm::$name(x, n)
        }
    )*};
}

/// The Bessel family, where C puts the order *first*: `jn(int, double)`.
macro_rules! bessel_order_f64 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(n: i32, x: f64) -> f64 {
            libm::$name(n, x)
        }
    )*};
}

macro_rules! bessel_order_f32 {
    ($($name:ident),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $name(n: i32, x: f32) -> f32 {
            libm::$name(n, x)
        }
    )*};
}

unary_f64!(
    acos, acosh, asin, asinh, atan, atanh, cbrt, ceil, cos, cosh, erf, erfc, exp, exp2, expm1,
    fabs, floor, lgamma, log, log10, log1p, log2, rint, round, sin, sinh, sqrt, tan, tanh, tgamma,
    trunc, j0, j1, y0, y1,
);

unary_f32!(
    acosf, acoshf, asinf, asinhf, atanf, atanhf, cbrtf, ceilf, cosf, coshf, erff, erfcf, expf,
    exp2f, expm1f, fabsf, floorf, lgammaf, logf, log10f, log1pf, log2f, rintf, roundf, sinf, sinhf,
    sqrtf, tanf, tanhf, tgammaf, truncf, j0f, j1f, y0f, y1f,
);

binary_f64!(atan2, copysign, fdim, fmax, fmin, fmod, hypot, nextafter, pow, remainder);

binary_f32!(atan2f, copysignf, fdimf, fmaxf, fminf, fmodf, hypotf, nextafterf, powf, remainderf);

scaled_f64!(ldexp, scalbn);
scaled_f32!(ldexpf, scalbnf);

bessel_order_f64!(jn, yn);
bessel_order_f32!(jnf, ynf);

// -------------------------------------------------------------------------
// The three that carry an out-parameter in C and a tuple in Rust.
//
// This is the only place in the file that needs `unsafe`, and the reason is that C's
// `frexp(double, int*)` cannot be spelled without a pointer. The write is guarded exactly the
// way the caller's own ABI contract requires - a NULL second argument is legal in C and means
// "I only want the mantissa" - so the `unsafe` is one conditional store per function, not a
// region of trust.
// -------------------------------------------------------------------------

/// `double frexp(double, int*)`
#[no_mangle]
pub extern "C" fn frexp(x: f64, exp: *mut i32) -> f64 {
    let (m, e) = libm::frexp(x);
    if !exp.is_null() {
        // SAFETY: the caller asked for a place to put the exponent, and it is not null.
        unsafe { *exp = e };
    }
    m
}

/// `float frexpf(float, int*)`
#[no_mangle]
pub extern "C" fn frexpf(x: f32, exp: *mut i32) -> f32 {
    let (m, e) = libm::frexpf(x);
    if !exp.is_null() {
        // SAFETY: as `frexp` above.
        unsafe { *exp = e };
    }
    m
}

/// `double modf(double, double*)`
#[no_mangle]
pub extern "C" fn modf(x: f64, iptr: *mut f64) -> f64 {
    let (frac, int) = libm::modf(x);
    if !iptr.is_null() {
        // SAFETY: as `frexp` above.
        unsafe { *iptr = int };
    }
    frac
}

/// `float modff(float, float*)`
#[no_mangle]
pub extern "C" fn modff(x: f32, iptr: *mut f32) -> f32 {
    let (frac, int) = libm::modff(x);
    if !iptr.is_null() {
        // SAFETY: as `frexp` above.
        unsafe { *iptr = int };
    }
    frac
}

/// `double remquo(double, double, int*)`
#[no_mangle]
pub extern "C" fn remquo(x: f64, y: f64, quo: *mut i32) -> f64 {
    let (r, q) = libm::remquo(x, y);
    if !quo.is_null() {
        // SAFETY: as `frexp` above.
        unsafe { *quo = q };
    }
    r
}

/// `float remquof(float, float, int*)`
#[no_mangle]
pub extern "C" fn remquof(x: f32, y: f32, quo: *mut i32) -> f32 {
    let (r, q) = libm::remquof(x, y);
    if !quo.is_null() {
        // SAFETY: as `frexp` above.
        unsafe { *quo = q };
    }
    r
}

// -------------------------------------------------------------------------
// The two that return an integer rather than a float: `ilogb(double) -> int`.
// -------------------------------------------------------------------------

/// `int ilogb(double)`
#[no_mangle]
pub extern "C" fn ilogb(x: f64) -> i32 {
    libm::ilogb(x)
}

/// `int ilogbf(float)`
#[no_mangle]
pub extern "C" fn ilogbf(x: f32) -> i32 {
    libm::ilogbf(x)
}

// -------------------------------------------------------------------------
// The three-argument fused multiply-add.
//
// `llvm.fma` has no ARMv7 instruction in front of it - `VFMA` arrived with ARMv8 / FPv4, and this
// target is `+vfp3` - so `f32::mul_add` and any crate's `fma` reach here as a libcall.
// -------------------------------------------------------------------------

/// `double fma(double, double, double)`
#[no_mangle]
pub extern "C" fn fma(x: f64, y: f64, z: f64) -> f64 {
    libm::fma(x, y, z)
}

/// `float fmaf(float, float, float)`
#[no_mangle]
pub extern "C" fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    libm::fmaf(x, y, z)
}
