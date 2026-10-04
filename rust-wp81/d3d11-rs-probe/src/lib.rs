//! Path A, Stage 1: **Rust owns the D3D11 renderer** on Windows Phone 8.1.
//!
//! WP8.1 has no OpenGL, so `prpr`'s GL renderer cannot run as written; the only GPU API is D3D11
//! (measured on device: Adreno 225, feature level 9.3). This crate is the Rust side of that.
//!
//! ## Why the binding is hand-rolled
//!
//! `windows` 0.62 compiles for this target, but its ABI plumbing pulls import descriptors for
//! `kernel32.dll`, `oleaut32.dll`, `combase.dll`, `rpcrt4.dll`, `user32.dll` and `ntdll.dll` -
//! none of which exist on WP8.1. Binding `d3d11.dll` and `dxgi.dll` by hand keeps the import table
//! to exactly those two libraries, which the kit's own ARM import libs prove are present:
//! `WP Kits\8.1\lib\ARM\{d3d11,dxgi,d3dcompiler}.lib`. (The `windows`-crate probe is kept under
//! `_stage1-windows-crate-probe/` as the evidence for that claim.)
//!
//! ## Layout
//!
//! * `direct.rs`   - the binding: types, constants, vtable slots, all taken from the kit's headers.
//! * `renderer.rs` - device, swapchain, shaders, pipeline, the quad.
//! * this file     - the `#[no_mangle]` entry points the C# side calls.
//!
//! ## The entry points
//!
//! ```
//! wp81_rs_start(panel, width, height) -> i32   // 0 = ok; brings up device + swapchain + pipeline
//! wp81_rs_clear(r, g, b, a)           -> i32
//! wp81_rs_draw_test_quad()            -> i32   // one textured quad, the Stage 1 acceptance test
//! wp81_rs_present()                   -> i32
//! wp81_rs_stop()
//! wp81_rs_stage()                     -> i32   // where it got to, for the C# log
//! ```

#![allow(non_snake_case, non_camel_case_types, dead_code)]

pub mod direct;
pub mod renderer;
pub mod surface_source;

use core::ffi::c_void;
use direct::*;
use renderer::*;
use surface_source::*;

// ===========================================================================
// global state
// ===========================================================================
//
// A single renderer instance is all WP8.1 needs: one GamePage, one swapchain. Rust statics rather
// than a heap allocation keeps this usable without the allocator being fully warmed up.

struct State {
    device: ComPtr,
    context: ComPtr,
    swapchain: ComPtr,
    rtv: ComPtr,
    /// The offscreen render-target texture — the WP8.1 phone present path (see
    /// [`renderer::create_offscreen_target`]). `rtv` above points at *this* on that route, and at
    /// the swapchain back buffer on the (dead) swapchain route.
    rt_tex: ComPtr,
    /// The WinRT `IDirect3DSurface` wrapping `rt_tex`, handed to the XAML compositor.
    gfx_surface: ComPtr,
    /// The WinRT `IDirect3DDevice` wrapping our D3D11 device, for the compositor.
    gfx_device: ComPtr,
    /// Which route came up: 0 = swapchain (composition), 1 = offscreen+interop, 2 = CoreWindow,
    /// 3 = XAML `SurfaceImageSource`.
    route: i32,
    /// The XAML `SurfaceImageSource` chain — route 3, the WP8.1 phone present path.
    surface: SurfaceChain,
    /// `(step, hresult)` from the `SurfaceImageSource` path.
    sis_trace: [i32; 2],
    /// `IDXGIDevice` handed to `ISurfaceImageSourceNative::SetDevice`, held for the route's life.
    dxgi_device: ComPtr,
    /// The worst status of the last `wp81_rs_draw_test_quad` on route 3 (0 = the frame drew).
    ///
    /// Kept so `wp81_rs_present` can report the frame's real outcome instead of a blanket success
    /// after the draw has already closed the frame with `EndDraw`.
    frame_err: i32,
    /// Colour the last `wp81_rs_clear` asked for.
    ///
    /// On route 3 a clear cannot happen when it is called: there is no render target until
    /// `BeginDraw` hands one back inside `wp81_rs_draw_test_quad`. So the request is remembered here
    /// and applied after the frame's view is bound. Without that the acceptance test paints only the
    /// quad and leaves whatever the compositor's atlas already held behind it.
    clear_color: [f32; 4],
    /// The white 1x1 texture the Stage 1 acceptance test samples.
    test_tex: ComPtr,
    test_srv: ComPtr,
    pipeline: Option<Pipeline>,
    stage: i32,
    last_hr: i32,
    /// (step, hresult) from the swapchain-attachment path, so a failure names its call.
    sc_trace: [i32; 2],
    /// (step, hresult) from the offscreen-target path.
    ot_trace: [i32; 2],
    /// Which swapchain-descriptor variant was accepted (0 = first/flip-sequential).
    sc_variant: i32,
    width: u32,
    height: u32,
    vertex_count: usize,
    /// A monotonic "how far did we get" counter, bumped *before* each risky native call.
    ///
    /// On the phone there is no debugger and a hard fault kills the process before `AppLog` flushes,
    /// so the last value this reached is the only evidence of where it died. C# reads it with
    /// [`wp81_rs_tick`] right after `wp81_rs_start` returns *and* from a watchdog that runs during
    /// the frame loop - so even a crash inside the native call leaves a number behind.
    tick: i32,
    /// Early-stop limit for [`wp81_rs_start_upto`] (0 = run to completion).
    limit: i32,
    /// The highest tick this instance actually reached, frozen when a limited run stops.
    reached: i32,
}

/// Added to the raw swapchain step (20..25) so `stage` cannot be confused with the coarse
/// 1..6 codes used by the rest of start-up.
const SC_BASE: i32 = 100;

static mut STATE: State = State {
    device: ComPtr::NULL,
    context: ComPtr::NULL,
    swapchain: ComPtr::NULL,
    rtv: ComPtr::NULL,
    rt_tex: ComPtr::NULL,
    gfx_surface: ComPtr::NULL,
    gfx_device: ComPtr::NULL,
    route: -1,
    surface: SurfaceChain::EMPTY,
    sis_trace: [0, 0],
    dxgi_device: ComPtr::NULL,
    frame_err: 0,
    clear_color: [0.06, 0.07, 0.09, 1.0],
    test_tex: ComPtr::NULL,
    test_srv: ComPtr::NULL,
    pipeline: None,
    stage: -1,
    last_hr: 0,
    sc_trace: [0, 0],
    ot_trace: [0, 0],
    sc_variant: -1,
    width: 0,
    height: 0,
    vertex_count: 0,
    tick: 0,
    limit: 0,
    reached: 0,
};

#[inline]
unsafe fn st() -> &'static mut State {
    &mut *core::ptr::addr_of_mut!(STATE)
}

/// Advance the progress counter and return it.
///
/// Called immediately *before* each native call that could fault, so `wp81_rs_tick()` afterwards
/// names the last call that was entered. Cheap enough to keep in release builds.
#[inline]
unsafe fn bump(s: &mut State, n: i32) -> i32 {
    s.tick = n;
    n
}

/// Set the progress tick from another module of this crate (see `renderer.rs`). Kept as a free
/// function so the swapchain code does not need access to `STATE`.
#[inline]
pub(crate) unsafe fn set_tick(n: i32) {
    st().tick = n;
}

// ===========================================================================
// vtable snapshot (diagnostic for the composition-swapchain hard fault)
// ===========================================================================

/// How many vtable slots [`dump_vtable`] records. `IDXGIFactory2`'s last method is slot 24
/// (`CreateSwapChainForComposition`), so 32 covers it plus slack to see if the table simply ends.
pub const VT_SLOTS: usize = 32;

/// The class name (or a marker) of whatever vtable was dumped last.
pub static mut VT_CLASS: [u8; 64] = [0; 64];
/// The recorded function pointers themselves.
pub static mut VT_ENTRIES: [usize; VT_SLOTS] = [0; VT_SLOTS];
/// How many entries were readable before a null/garbage slot was seen.
pub static mut VT_COUNT: usize = 0;

/// Snapshot up to [`VT_SLOTS`] function pointers from a COM object's vtable.
///
/// The point is to answer, on the phone, "does this object actually have a slot 24?" - a question
/// that cannot be asked any other way without a debugger. A slot that reads back as 0 (or as a
/// pointer outside the code range) is the tell that the vtable is shorter than assumed and the
/// call we are about to make will jump into garbage.
pub(crate) unsafe fn dump_vtable(obj: ComPtr) {
    let class = &mut *core::ptr::addr_of_mut!(VT_CLASS);
    for b in class.iter_mut() {
        *b = 0;
    }
    let entries = &mut *core::ptr::addr_of_mut!(VT_ENTRIES);
    for e in entries.iter_mut() {
        *e = 0;
    }
    VT_COUNT = 0;
    if obj.is_null() {
        return;
    }
    let vtable: *const usize = *obj.vtbl;
    if vtable.is_null() {
        return;
    }
    let mut n = 0usize;
    while n < VT_SLOTS {
        let p = *vtable.add(n);
        entries[n] = p;
        n += 1;
        if p == 0 {
            break;
        }
    }
    VT_COUNT = n;
}

/// Number of readable vtable slots in the last [`dump_vtable`] (0 = nothing dumped).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_vt_count() -> u32 {
    VT_COUNT as u32
}

/// The recorded vtable slot `i` (0 if out of range or not dumped). C# formats it as hex.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_vt_entry(i: u32) -> usize {
    if (i as usize) >= VT_SLOTS {
        return 0;
    }
    let entries = &*core::ptr::addr_of!(VT_ENTRIES);
    entries[i as usize]
}

/// True when the current [`wp81_rs_start_upto`] run asked to stop at exactly `n`.
///
/// Used by sub-steps that live *inside* the swapchain path: `renderer.rs` calls this at its own
/// boundaries so a run with `limit = 13` can obtain the DXGI factory and then stop before the call
/// that would fault, letting C# read the vtable snapshot.
#[inline]
pub(crate) unsafe fn limit_is(n: i32) -> bool {
    st().limit == n
}

// ===========================================================================
// factory soundness probe
// ===========================================================================

/// `IDXGIFactory2::IsCurrent()` result from the last swapchain attempt (-1 = not run).
pub static mut FACTORY_IS_CURRENT: i32 = -1;
/// HRESULT of a `QueryInterface(IID_IDXGIFactory2)` round-trip on that same object.
pub static mut FACTORY_QI_HR: i32 = 0;

/// Record the harmless-method probe run just before `CreateSwapChainForComposition`.
///
/// A sane `IsCurrent` plus a successful QI round-trip means the factory pointer and its vtable are
/// valid, which isolates a hard fault at slot 24 to that one entry rather than to corruption.
pub(crate) unsafe fn record_factory_probe(is_current: i32, qi_hr: i32) {
    FACTORY_IS_CURRENT = is_current;
    FACTORY_QI_HR = qi_hr;
}

/// `IDXGIFactory2::IsCurrent()` as observed on the phone; -1 means the probe did not run.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_factory_is_current() -> i32 {
    FACTORY_IS_CURRENT
}

/// HRESULT of the `QueryInterface(IID_IDXGIFactory2)` round-trip that preceded the probe.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_factory_qi_hr() -> i32 {
    FACTORY_QI_HR
}

// ===========================================================================
// panel-interop probe
// ===========================================================================

/// Which XAML panel-interop interface the last attach answered: 0 = `ISwapChainPanelNative`,
/// 1 = `ISwapChainBackgroundPanelNative`, -1 = neither (not run).
///
/// The two panels are the only two ways to put a swapchain into WP8.1 XAML, and a caller cannot
/// tell from the managed type which one a page should hand over, so the native side records which
/// one it actually found.
pub static mut PANEL_IFACE: i32 = -1;

/// Record which panel-interop interface answered the `QueryInterface` (see [`PANEL_IFACE`]).
pub(crate) unsafe fn record_panel_iface(which: i32) {
    PANEL_IFACE = which;
}

/// The panel-interop interface the last attach used; -1 = none.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_panel_iface() -> i32 {
    PANEL_IFACE
}

// ===========================================================================
// CoreWindow probe (the CreateSwapChainForCoreWindow route)
// ===========================================================================

/// Vtable address and object address of the `pWindow` pointer handed to
/// `CreateSwapChainForCoreWindow`, and the HRESULT of `QI(IUnknown)` on it.
///
/// These exist to tell a *proxy* apart from the real object when the call comes back
/// `E_INVALIDARG`. `CreateSwapChainForCoreWindow` QIs `pWindow` for the CoreWindow interface, and a
/// non-agile WinRT RCW can hand C# a marshalled proxy whose vtable lives in combase, not in the
/// CoreWindow implementation. A real object and a proxy are distinguishable by where their vtables
/// point, which is what gets recorded here.
pub static mut COREWINDOW_VTBL: u32 = 0;
pub static mut COREWINDOW_OBJ: u32 = 0;
pub static mut COREWINDOW_QI: i32 = 0x7FFF_FFFF;

/// `IDXGIFactory2::IsWindowedStereoEnabled()` (slot 14) captured on the CoreWindow path, just
/// before `CreateSwapChainForCoreWindow`. BOOL: must be 0 or 1; anything else means the factory
/// vtable is misaligned, the one failure mode that would make every descriptor shape return
/// `DXGI_ERROR_INVALID_CALL` identically. -1 = not probed.
pub static mut CW_FACTORY_STEREO: i32 = -1;

/// Record the CoreWindow path's factory stereo probe (see [`CW_FACTORY_STEREO`]).
pub(crate) unsafe fn record_cw_factory_stereo(v: i32) {
    CW_FACTORY_STEREO = v;
}

/// `IsWindowedStereoEnabled()` as last observed on the CoreWindow path; -1 = not probed.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_cw_factory_stereo() -> i32 {
    CW_FACTORY_STEREO
}

/// Record the CoreWindow pointer's vtable + object addresses (see [`COREWINDOW_VTBL`]).
pub(crate) unsafe fn record_corewindow(vtbl: u32, obj: u32) {
    COREWINDOW_VTBL = vtbl;
    COREWINDOW_OBJ = obj;
}

/// Record the HRESULT of `QI(IUnknown)` on the CoreWindow pointer (see [`COREWINDOW_VTBL`]).
pub(crate) unsafe fn record_corewindow_qi(hr: i32) {
    COREWINDOW_QI = hr;
}

/// Vtable address of the last CoreWindow pointer; 0 = never probed.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_corewindow_vtbl() -> u32 {
    COREWINDOW_VTBL
}

/// Object address of the last CoreWindow pointer; 0 = never probed.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_corewindow_obj() -> u32 {
    COREWINDOW_OBJ
}

/// `QI(IUnknown)` HRESULT on the last CoreWindow pointer; `0x7FFFFFFF` = not probed.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_corewindow_qi() -> i32 {
    COREWINDOW_QI}

// ===========================================================================
// per-variant swapchain HRESULTs
// ===========================================================================

/// HRESULT returned by each `CreateSwapChainForComposition` descriptor variant, in try order.
///
/// All of them being the same error is what says "no shape works with this API", whereas a mix says
/// "this one specific field is the problem". Recorded unconditionally, including on success.
pub static mut SC_HRS: [i32; 4] = [0x7FFF_FFFF; 4];

/// Record variant `i`'s HRESULT (see [`SC_HRS`]).
pub(crate) unsafe fn record_sc_hr(i: usize, hr: i32) {
    if i < SC_HRS.len() {
        SC_HRS[i] = hr;
    }
}

/// Reset the per-variant table (called at the top of every start-up so a stale run's values do not
/// read as this run's).
pub(crate) unsafe fn reset_sc_hrs() {
    for h in SC_HRS.iter_mut() {
        *h = 0x7FFF_FFFF;
    }
}

/// HRESULT of `CreateSwapChainForComposition` descriptor variant `i`; `0x7FFFFFFF` = not tried.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sc_hr(i: u32) -> i32 {
    if (i as usize) < SC_HRS.len() {
        SC_HRS[i as usize]
    } else {
        0x7FFF_FFFF
    }
}

/// Record that tick `n` was reached, and report whether a limited run should stop **before**
/// executing the call that follows it.
///
/// `bump` fires at every step boundary; this is the gate that decides whether the run continues.
/// The rule is: **stop once the tick has reached the limit** (`n >= limit`), so a run with
/// `limit = 10` sails past step 2 and halts at step 10, while `limit = 2` halts immediately. The
/// first draft had this comparison the other way round (`limit >= n`), which made *every* run stop
/// at step 2 - the symptom was `reached=2` for even a `limit=99` call.
///
/// `reached` is advanced here too, so C# can read back the furthest step a limited run proved good.
#[inline]
unsafe fn stop_here(s: &mut State, n: i32) -> bool {
    if n > s.reached {
        s.reached = n;
    }
    s.limit != 0 && n >= s.limit
}

// ===========================================================================
// start-up
// ===========================================================================

/// Bring up the renderer on the **offscreen render-target route** — the WP8.1 phone present path.
///
/// This is the corrected Stage 1. It creates the device, an offscreen `B8G8R8A8` render target and
/// the WinRT `IDirect3DSurface`/`IDirect3DDevice` pair the XAML compositor needs, then builds the
/// pipeline. No swapchain is involved: `CreateSwapChainForCoreWindow` and
/// `CreateSwapChainForComposition` are both unimplemented for a FL 9.3 phone app on this driver,
/// and Win2D — which renders fine on the same device — imports no swapchain entry points at all.
///
/// Returns 0 on success; otherwise the stage code, with `wp81_rs_ot_step()`/`wp81_rs_ot_step_name()`
/// naming the failing call and `wp81_rs_last_hr()` the HRESULT.
///
/// The caller must read the WinRT objects with [`wp81_rs_gfx_surface`] / [`wp81_rs_gfx_device`] and
/// hand them to XAML (see `RustRenderer.cs`); Rust owns the pixels, the compositor owns presentation.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start_offscreen(width: u32, height: u32) -> i32 {
    let s = st();
    release_all(s);
    reset_sc_hrs();
    s.width = width;
    s.height = height;
    s.limit = 0;
    s.reached = 0;

    bump(s, 2); // create_device
    let (device, context, level, _desc) = match create_device() {
        Some(v) => v,
        None => {
            s.stage = 1;
            return 1;
        }
    };
    s.device = device;
    s.context = context;
    s.last_hr = 0;
    let _ = level;

    bump(s, 280); // offscreen: texture + RTV + WinRT surface
    let (rt_tex, rtv, gfx_surface) =
        match renderer::create_offscreen_target(device, width, height, s.ot_trace.as_mut_ptr()) {
            Some(v) => v,
            None => {
                s.stage = OT_BASE + s.ot_trace[0];
                s.last_hr = s.ot_trace[1];
                return 2;
            }
        };
    s.rt_tex = rt_tex;
    s.rtv = rtv;
    s.gfx_surface = gfx_surface;

    bump(s, 283); // offscreen: WinRT IDirect3DDevice for the compositor
    match renderer::wrap_device_for_compositor(device, s.ot_trace.as_mut_ptr()) {
        Some(d) => s.gfx_device = d,
        None => {
            s.stage = OT_BASE + s.ot_trace[0];
            s.last_hr = s.ot_trace[1];
            return 3;
        }
    }

    bump(s, 30); // build_pipeline
    let pipeline = match build_pipeline(device, context) {
        Some(p) => p,
        None => {
            s.stage = 5;
            return 5;
        }
    };

    bump(s, 40); // create_test_texture
    if !create_test_texture(device, s) {
        s.pipeline = Some(pipeline);
        s.stage = 6;
        return 6;
    }
    s.pipeline = Some(pipeline);

    let vp = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width as f32,
        Height: height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let set_viewports: extern "system" fn(*mut c_void, UINT, *const D3D11_VIEWPORT) =
        context.method(D3D11CONTEXT_RSSETVIEWPORTS);
    set_viewports(context.vtbl as *mut c_void, 1, &vp);

    s.route = 1; // offscreen + interop
    bump(s, 99);
    s.stage = 0;
    0
}

/// Bring up the renderer on the **XAML `SurfaceImageSource` route** — the real WP8.1 phone present
/// path, and the one that needs no D3D interop helper.
///
/// This supersedes [`wp81_rs_start_offscreen`]. That route had to project an
/// `IDXGISurface` to a WinRT `IDirect3DSurface` with `CreateDirect3D11SurfaceFromDXGISurface`, and
/// on device that helper is unreachable: the loader probe logged
/// `interop-trace=winrt{h=0,exp=0} d3d11{h=1,exp=0}`, i.e. `winrt.dll` cannot be loaded from an app
/// container and `d3d11.dll` does not export it, and Win2D's own import table shows it never uses
/// the helper either.
///
/// `SurfaceImageSource` needs none of that. It is a XAML type, so a plain
/// `RoGetActivationFactory` — the API set an app container *is* allowed to load, and the one Win2D
/// itself imports — reaches it, and its native interface `ISurfaceImageSourceNative` is obtained by
/// an ordinary `QueryInterface`. Its `BeginDraw` then returns an `IDXGISurface*` that lives inside
/// the compositor's own render target, so drawing into it *is* presenting:
///
///   * no swapchain (which this driver does not implement for phone apps),
///   * no interop helper (which is unreachable from an app),
///   * no full-screen offscreen texture, and no per-frame blit through C#.
///
/// Returns 0 on success; otherwise the stage code, with `wp81_rs_sis_step()` naming the failing call
/// and `wp81_rs_last_hr()` the HRESULT.
///
/// The caller must read the `SurfaceImageSource` `IInspectable*` with [`wp81_rs_sis_source`] and
/// assign it to a XAML `Image.Source` (it is a `Windows.UI.Xaml.Media.ImageSource`). Rust owns the
/// device, the surface and every draw call; XAML only displays.
///
/// ## ⚠ CALL THIS ON THE UI THREAD
///
/// `SurfaceImageSource` is a **XAML** object and therefore carries a XAML thread affinity: creating
/// it, `SetDevice` on it, and every `BeginDraw`/`EndDraw` must happen on the UI thread. Doing any of
/// it from a pool thread fails with **`RPC_E_WRONG_THREAD` (`0x8001010E`)** — measured on device, and
/// the reason [`wp81_rs_start_surface_prepare`] exists separately.
///
/// That is an unfortunate trade for the crash-survivability rule (a native bring-up on the UI thread
/// blocks WinRT `FileIO`, so the log cannot flush if it faults). The split keeps the best of both:
/// the unbounded, crash-prone part — device creation and pipeline build — runs in
/// `_prepare` on a pool thread, and only the small, deterministic surface activation runs here.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start_surface(width: u32, height: u32) -> i32 {
    let rc = wp81_rs_start_surface_prepare(width, height);
    if rc != 0 {
        return rc;
    }
    wp81_rs_start_surface_attach()
}

/// Stage 1 of the route-3 bring-up: device, `IDXGIDevice`, pipeline, test texture. **Any thread.**
///
/// This is the half that can fault in a driver call, so it belongs on a pool thread where the UI
/// thread survives to flush the log. It deliberately does *not* touch WinRT.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start_surface_prepare(width: u32, height: u32) -> i32 {
    let s = st();
    release_all(s);
    reset_sc_hrs();
    s.width = width;
    s.height = height;
    s.limit = 0;
    s.reached = 0;

    bump(s, 2); // create_device
    let (device, context, level, _desc) = match create_device() {
        Some(v) => v,
        None => {
            s.stage = 1;
            return 1;
        }
    };
    s.device = device;
    s.context = context;
    s.last_hr = 0;
    let _ = level;

    // The XAML surface needs the device's DXGI side for `SetDevice`.
    bump(s, 285); // surface: QueryInterface(IDXGIDevice)
    let mut dxgi_device = ComPtr::NULL;
    let hr = device.query_interface(
        &IID_IDXGIDEVICE as *const Guid,
        &mut dxgi_device.vtbl as *mut _ as *mut *mut c_void,
    );
    if hr < 0 || dxgi_device.is_null() {
        s.last_hr = hr;
        s.sis_trace[0] = SIS_QI_DXGIDEVICE;
        s.sis_trace[1] = hr;
        s.stage = SIS_BASE + SIS_QI_DXGIDEVICE;
        return 2;
    }
    s.dxgi_device = dxgi_device;

    bump(s, 30); // build_pipeline
    let pipeline = match build_pipeline(device, context) {
        Some(p) => p,
        None => {
            s.stage = 5;
            return 5;
        }
    };

    bump(s, 40); // create_test_texture
    if !create_test_texture(device, s) {
        s.pipeline = Some(pipeline);
        s.stage = 6;
        return 6;
    }
    s.pipeline = Some(pipeline);
    0
}

/// Stage 2 of the route-3 bring-up: activate the WinRT class, QI `ISurfaceImageSourceNative`,
/// `SetDevice`. **MUST run on the UI thread** — see [`wp81_rs_start_surface`].
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start_surface_attach() -> i32 {
    let s = st();
    if s.device.is_null() || s.dxgi_device.is_null() {
        return 7; // _prepare was not called (or failed) - nothing to attach to
    }

    // Activate the WinRT class, create the instance, QI the native interface.
    bump(s, 286); // surface: activate + create + QI native
    let mut chain = match SurfaceChain::create(s.width as i32, s.height as i32, true) {
        Ok(c) => c,
        Err((step, hr)) => {
            s.sis_trace[0] = step;
            s.sis_trace[1] = hr;
            s.last_hr = hr;
            s.stage = SIS_BASE + step;
            return 3;
        }
    };

    bump(s, 287); // surface: SetDevice
    let hr = chain.set_device(s.dxgi_device.vtbl as *mut c_void);
    if hr < 0 {
        chain.release_all();
        s.sis_trace[0] = SIS_SET_DEVICE;
        s.sis_trace[1] = hr;
        s.last_hr = hr;
        s.stage = SIS_BASE + SIS_SET_DEVICE;
        return 4;
    }

    s.surface = chain;
    s.route = 3; // XAML SurfaceImageSource

    // Read the surface shape *now*, before the first frame can touch it.
    //
    // `CreateRenderTargetView` on this surface is the call that has twice ended the process, and
    // the description that would say why was only ever returned by `draw_test_quad` — i.e. logged
    // after the very call it explains. One `BeginDraw`/`GetDesc`/`EndDraw` at attach time puts
    // `fmt`/`BindFlags` in the log line C# prints immediately after this returns, so if the frame
    // still dies the log names the surface rather than just the tick. Failure is recorded, not
    // fatal: a broken probe would have failed the frame the same way, and losing it must not cost
    // the bring-up line that follows.
    bump(s, 98); // surface probe: BeginDraw/GetDesc/EndDraw
    if let Err((step, hr)) = s.surface.probe(&s.device) {
        s.sis_trace[0] = step;
        s.sis_trace[1] = hr;
        s.last_hr = hr;
        // stage is left alone: attach itself succeeded.
    }

    bump(s, 99);
    s.stage = 0;
    0
}

/// The `SurfaceImageSource` `IInspectable*` C# must assign to a XAML `Image.Source`.
///
/// It is a `Windows.UI.Xaml.Media.ImageSource`, so the C# side can cast it directly after a
/// `Marshal.GetObjectForIUnknown`. Null unless the `SurfaceImageSource` route came up.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_source() -> *mut c_void {
    st().surface.source_inspectable.vtbl as *mut c_void
}

/// The step code the `SurfaceImageSource` bring-up failed at (0 = it did not fail).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_step() -> i32 {
    st().sis_trace[0]
}

/// The HRESULT recorded alongside [`wp81_rs_sis_step`].
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_hr() -> i32 {
    st().sis_trace[1]
}

/// One entry of the route-3 target surface's diagnostics — what the surface *is*, and what each
/// view-building attempt answered.
///
/// Index: 0 `DXGI_FORMAT`, 1 `BindFlags`, 2 width, 3 height, 4 `SampleDesc.Count`,
/// 5 `MiscFlags`, 6 `QI(ID3D11Texture2D)` HRESULT,
/// 7 `CreateRenderTargetView(NULL desc)` HRESULT, 8 the explicit-descriptor retry's HRESULT,
/// 9 the format that retry used,
/// 10 whether the texture's `GetDevice` is our device (1 same, 0 other, -1 unknown),
/// 11 which path the frame took (0 direct, 1 own-texture + `CopyResource`, -1 texture refused,
///    -2 view refused),
/// 12 the HRESULT that refused that path,
/// 13/14 `BeginDraw`'s offset of this frame inside the source's atlas,
/// 15 whether `IDXGISurface*` and `ID3D11Texture2D*` are the same address (1 same, 0 different,
///    -1 unknown). Out-of-range returns -1.
///
/// The surface is XAML's, so this is the only channel through which the phone log can explain a
/// refused view: `E_INVALIDARG` alone does not say whether the format was typeless, the resource
/// was never bound as a render target, or it was made by another device — and those need
/// different fixes (a typed descriptor, a copy into our own texture, or nothing at all).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_diag(index: i32) -> i32 {
    let s = st();
    if index < 0 || index as usize >= s.surface.diag.len() {
        return -1;
    }
    s.surface.diag[index as usize]
}

/// One pixel read back out of the frame the driver actually rasterized, packed `0xAARRGGBB`.
///
/// Index 0..3 selects a probe point (see `SurfaceChain::probe_pixels`); anything else returns
/// `0xFF00FFFF`. Check [`wp81_rs_sis_pxseq`] first — it is `-1` until a probe has run, and that is
/// the difference between "the readback reported black" and "no readback ran", which the pixel
/// value alone cannot show.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_pixel(index: i32) -> u32 {
    let s = st();
    if index < 0 || index as usize >= s.surface.pixels.len() {
        return 0xFF00FFFF;
    }
    s.surface.pixels[index as usize]
}

/// How many pixel readbacks have completed (counted from 0), or `-1` if none ever has.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_pxseq() -> i32 {
    st().surface.pixel_seq
}

/// One entry of the viewport and status recorded alongside those pixels.
///
/// 0 `TopLeftX`, 1 `TopLeftY`, 2 `Width`, 3 `Height` (hundredths of a pixel), 4 `MinDepth`,
/// 5 `MaxDepth` (thousandths), 6 how many probes ran, 7 status (0 = read, otherwise the HRESULT or
/// negative reason that stopped it). Out of range returns `i32::MIN`, so it cannot be mistaken for
/// one of the statuses — `-1` and `-2` are real answers here.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_vdbg(index: i32) -> i32 {
    let s = st();
    if index < 0 || index as usize >= s.surface.vdbg.len() {
        return i32::MIN;
    }
    s.surface.vdbg[index as usize]
}

/// English name of the `wp81_rs_sis_step()` code, for the phone log.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sis_step_name() -> *const u8 {
    match st().sis_trace[0] {
        SIS_OK => b"surface: OK\0".as_ptr(),
        SIS_ACTIVATION_FACTORY => {
            b"surface: RoGetActivationFactory(Windows.UI.Xaml.Media.Imaging.SurfaceImageSource)\0".as_ptr()
        }
        SIS_CREATE_INSTANCE => {
            b"surface: ISurfaceImageSourceFactory::CreateInstanceWithDimensionsAndOpacity\0".as_ptr()
        }
        SIS_QI_NATIVE => {
            b"surface: QueryInterface(IID_ISurfaceImageSourceNative)\0".as_ptr()
        }
        SIS_SET_DEVICE => b"surface: ISurfaceImageSourceNative::SetDevice\0".as_ptr(),
        SIS_BEGIN_DRAW => b"surface: ISurfaceImageSourceNative::BeginDraw\0".as_ptr(),
        SIS_END_DRAW => b"surface: ISurfaceImageSourceNative::EndDraw\0".as_ptr(),
        SIS_QI_SURFACE => b"surface: QueryInterface(IID_IDXGISurface) on the BeginDraw surface\0".as_ptr(),
        SIS_CREATE_RTV => b"surface: CreateRenderTargetView on the BeginDraw surface\0".as_ptr(),
        SIS_QI_DXGIDEVICE => b"surface: QueryInterface(IID_IDXGIDevice) on our device\0".as_ptr(),
        _ => b"\0".as_ptr(),
    }
}

/// Added to the raw offscreen step (40..46) so `stage` cannot collide with the 1..6 start-up codes.
const OT_BASE: i32 = 300;

/// The WinRT `IDirect3DSurface*` (an `IInspectable`) to hand the XAML compositor. 0 if not up.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_gfx_surface() -> *mut c_void {
    st().gfx_surface.vtbl as *mut c_void
}

/// The WinRT `IDirect3DDevice*` (an `IInspectable`) matching [`wp81_rs_gfx_surface`].
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_gfx_device() -> *mut c_void {
    st().gfx_device.vtbl as *mut c_void
}

/// Which route came up: 0 = swapchain, 1 = offscreen+interop, 2 = CoreWindow, -1 = none.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_route() -> i32 {
    st().route
}

/// The raw offscreen sub-step code (40..46).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_ot_step() -> i32 {
    st().ot_trace[0]
}

/// A name for the offscreen sub-step, so a failure reads `offscreen: CreateDirect3D11Surface…`.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_ot_step_name() -> *const u8 {
    let step = st().ot_trace[0];
    let s: &[u8] = match step {
        40 => b"offscreen: bad width/height\0",
        41 => b"offscreen: CreateTexture2D\0",
        42 => b"offscreen: CreateRenderTargetView\0",
        43 => b"offscreen: CreateDirect3D11SurfaceFromDXGISurface\0",
        44 => b"offscreen: (target created)\0",
        45 => b"offscreen: QI(IDXGIDevice) for the compositor device\0",
        46 => b"offscreen: CreateDirect3D11DeviceFromDXGIDevice\0",
        47 => b"offscreen: QI(IDXGISurface) on the render target\0",
        _ => b"(none)\0",
    };
    s.as_ptr()
}

/// Which module the D3D11/XAML interop helpers resolved from: 1 = `winrt.dll` (the phone's home
/// for them), 2 = `d3d11.dll` (desktop Windows), 0 = neither. -1 if never attempted.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_interop_source() -> i32 {
    crate::direct::INTEROP_SOURCE
}

/// One element of the interop-resolution diagnostic trace (see `direct::INTEROP_TRACE`).
///   index 0 = winrt.dll handle obtained, 1 = winrt.dll exported the name,
///   index 2 = d3d11.dll handle obtained, 3 = d3d11.dll exported the name.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_interop_trace(i: i32) -> i32 {
    if i < 0 || i > 3 {
        return -1;
    }
    crate::direct::INTEROP_TRACE[i as usize]
}

///
/// Only meaningful once `wp81_rs_ot_step()` is 47 (the QI step). The value is the second slot of
/// the offscreen trace, where `create_offscreen_target` stashes it: 2 = `IDXGISurface2`,
/// 1 = `IDXGISurface1`, 0 = `IDXGISurface`. -1 means the QI step has not run.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_ot_surface_kind() -> i32 {
    let s = st();
    if s.ot_trace[0] != OT_QI_SURFACE {
        return -1;
    }
    s.ot_trace[1]
}

/// Create the device, the composition swapchain, its render target view, and the pipeline.
///
/// `panel` is the `IInspectable*` of the XAML `SwapChainPanel` (passed from C# as the
/// `SwapChainPanel` object's native interface). `width`/`height` are the panel's physical pixels.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start(panel: *mut c_void, width: u32, height: u32) -> i32 {
    wp81_rs_start_upto(panel, width, height, 0)
}

/// Bring-up with an **early-stop limit** for bisecting a hard fault on device.
///
/// `limit == 0` runs the whole path (identical to [`wp81_rs_start`]). A non-zero `limit` returns
/// *successfully* as soon as the tick reaches it, before executing the next risky call. That turns
/// "it crashes somewhere in start-up" into "it crashes at step N" without a debugger:
///
///   2  = stop right after create_device
///   10 = stop right before touching the panel (swapchain+attach)
///   16 = stop right after the panel attach
///   20 = stop before GetBuffer
///   21 = stop before CreateRenderTargetView
///   30 = stop before build_pipeline (shader compile)
///   40 = stop before create_test_texture
///   50 = stop before RSSetViewports
///
/// The C# side calls it a few times with increasing limits, logging between each, so the last log
/// line before the process dies names the step that faulted.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start_upto(
    panel: *mut c_void,
    width: u32,
    height: u32,
    limit: i32,
) -> i32 {
    let s = st();
    // A limited run leaves partially-built state behind (a device, sometimes a swapchain) and the
    // next run overwrites those fields. Release first so eight bisect steps do not leak eight
    // devices - on a 512 MB phone that is what turns a diagnostic into a second failure.
    release_all(s);
    // Reset the per-variant HRESULT table HERE, not in `release_all` - `wp81_rs_stop()` calls
    // `release_all` between bisect steps, so resetting there wiped the table before C# could log
    // it (every entry read back "n/a").
    reset_sc_hrs();
    s.width = width;
    s.height = height;
    s.limit = limit;
    s.reached = 0;
    // tick 1 is set by the C# side before it even calls in; see RustRenderer.Start.
    bump(s, 2); // create_device: DXGI factory + adapter enumeration + D3D11CreateDevice
    if stop_here(s, 2) {
        return 0;
    }

    let (device, context, level, _desc) = match create_device() {
        Some(v) => v,
        None => {
            s.stage = 1;
            return 1;
        }
    };
    s.device = device;
    s.context = context;
    let _ = level;

    bump(s, 10); // create_swapchain_for_panel_traced (QI/GetAdapter/GetParent/Create)
    if stop_here(s, 10) {
        return 0;
    }
    // Sub-ticks 11..16 live inside the swapchain path, and the DXGI factory - the object whose
    // vtable we need - only exists between ticks 12 and 13. The path honours `s.limit` itself, so a
    // run with limit=13 stops the moment the factory is in hand, *before* the call that faults,
    // leaving a valid snapshot for C# to log.
    let swapchain = match create_swapchain_for_panel_traced(device, panel, width, height, s.sc_trace.as_mut_ptr()) {
        Some(sc) => {
            // SC_CREATE_OK + the accepted variant index, for the log.
            s.sc_variant = s.sc_trace[1];
            sc
        }
        None => {
            // An intentional sub-step stop (limit 11..15, or 18 = "skip the probe and stop before
            // slot 24", or 19 = "use the direct factory, try the create, then stop") returns None
            // without an error; report it as success so the bisect keeps its "rc=0 = the step did
            // not fault" invariant.
            if (s.limit >= 11 && s.limit <= 15) || s.limit == 18 || s.limit == 19 {
                s.reached = s.limit;
                return 0;
            }
            // Report *which* call failed: s.stage carries the sub-step (20..25) and last_hr the
            // HRESULT, so the phone's log names the failing call instead of a bare "stage 2".
            s.stage = SC_BASE + s.sc_trace[0];
            s.last_hr = s.sc_trace[1];
            return 2;
        }
    };
    s.swapchain = swapchain;
    if stop_here(s, 16) {
        return 0; // swapchain created + attached to the panel, nothing else touched
    }

    // Back buffer -> render target view.
    let get_buffer: extern "system" fn(
        *mut c_void,
        UINT,
        *const Guid,
        *mut *mut c_void,
    ) -> HRESULT = swapchain.method(IDXGISWAPCHAIN_GETBUFFER);
    let mut backbuffer = ComPtr::NULL;
    bump(s, 20); // IDXGISwapChain::GetBuffer(0)
    if stop_here(s, 20) {
        return 0;
    }
    if get_buffer(
        swapchain.vtbl as *mut c_void,
        0,
        &IID_ID3D11TEXTURE2D as *const Guid,
        &mut backbuffer.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
        || backbuffer.is_null()
    {
        s.stage = 3;
        return 3;
    }
    let create_rtv: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const c_void,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATERENDERTARGETVIEW);
    let mut rtv = ComPtr::NULL;
    bump(s, 21); // ID3D11Device::CreateRenderTargetView(backbuffer)
    if stop_here(s, 21) {
        backbuffer.release();
        return 0;
    }
    let hr = create_rtv(
        device.vtbl as *mut c_void,
        backbuffer.vtbl as *mut c_void,
        core::ptr::null(),
        &mut rtv.vtbl as *mut _ as *mut *mut c_void,
    );
    backbuffer.release();
    s.last_hr = hr;
    if hr < 0 || rtv.is_null() {
        s.stage = 4;
        return 4;
    }
    s.rtv = rtv;

    bump(s, 30); // build_pipeline: D3DCompile x2, Create*Shader, CreateInputLayout, buffers, states
    if stop_here(s, 30) {
        return 0;
    }
    let pipeline = match build_pipeline(device, context) {
        Some(p) => p,
        None => {
            s.stage = 5;
            return 5;
        }
    };
    // ... and the 1x1 white texture the test quad samples.
    bump(s, 40); // create_test_texture: CreateTexture2D + CreateShaderResourceView
    if stop_here(s, 40) {
        return 0;
    }
    if !create_test_texture(device, s) {
        s.stage = 6;
        return 6;
    }
    s.pipeline = Some(pipeline);

    let vp = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width as f32,
        Height: height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let set_viewports: extern "system" fn(*mut c_void, UINT, *const D3D11_VIEWPORT) =
        context.method(D3D11CONTEXT_RSSETVIEWPORTS);
    bump(s, 50); // RSSetViewports
    if stop_here(s, 50) {
        return 0;
    }
    set_viewports(context.vtbl as *mut c_void, 1, &vp);

    bump(s, 99); // fully up
    s.stage = 0;
    0
}

/// The **CoreWindow** bring-up route, the fallback to `CreateSwapChainForComposition`.
///
/// Everything after the swapchain differs not at all, so this deliberately shares the same global
/// `STATE` and the same downstream code path as [`wp81_rs_start`] - the only difference is the call
/// that creates the swapchain. `wp81_rs_stop()` tears down either route identically.
///
/// `core_window` is the `ICoreWindow*` of `Windows.UI.Core.CoreWindow.GetForCurrentThread()`.
/// Returns 0 on success; `stage` names the failing step (100+ = the CoreWindow sub-steps).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_start_corewindow(
    core_window: *mut c_void,
    width: u32,
    height: u32,
) -> i32 {
    let s = st();

    // First observable act: mark entry. If the log's last line is "calling native" and the tick
    // never even reaches 250, the fault is in the prologue below (release_all / the QI probe)
    // rather than in any D3D call, which is otherwise indistinguishable without a debugger.
    unsafe { bump(s, 250); } // corewindow: entered native

    release_all(s);
    reset_sc_hrs();
    bump(s, 251); // release_all done
    s.width = width;
    s.height = height;
    s.limit = 0;
    s.reached = 0;

    // Probe the CoreWindow pointer FIRST, and encode the outcome into the tick counter so the
    // managed watchdog (a 200 ms DispatcherTimer) records it even if the very next call hard-faults
    // the process. The last run died between "calling native" and any log line, which is exactly
    // the blind spot this closes: a crash here leaves the tick value as the only evidence.
    //
    // Tick code 200+n: 200 = null pointer, 201 = QI(IUnknown) returned S_OK, 202 = QI failed with
    // the HRESULT in `COREWINDOW_QI`, 203 = the vtable read itself faulted (pointer is not a COM
    // object at all).
    unsafe {
        if core_window.is_null() {
            bump(s, 200);
        } else {
            let vtbl = *(core_window as *const *const usize);
            record_corewindow(vtbl as u32, core_window as u32);
            let qi: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
                ComPtr { vtbl: core_window as *const *const usize }.method(0);
            let mut punk = ComPtr::NULL;
            let hr = qi(core_window, &IID_IUNKNOWN, &mut punk.vtbl as *mut _ as *mut *mut c_void);
            record_corewindow_qi(hr);
            punk.release();
            bump(s, if hr == 0 { 201 } else { 202 });
        }
    }
    bump(s, 252); // pointer probed

    bump(s, 2);
    let (mut device, mut context, _level, _desc) = match create_device() {
        Some(v) => v,
        None => {
            s.stage = 1;
            return 1;
        }
    };
    s.device = device;
    s.context = context;

    bump(s, 10);
    // Device-shape sweep. Both swapchain routes returned DXGI_ERROR_INVALID_CALL at the *create*
    // step, with a sane factory vtable (cw-stereo=0), a valid window pointer (cw-qi=S_OK) and the
    // same result for every descriptor shape. That leaves the device as the one unvaried argument,
    // so try the two device shapes WP8.1 samples use:
    //   1. enumerated non-software adapter + D3D_DRIVER_TYPE_UNKNOWN  (current)
    //   2. default adapter + D3D_DRIVER_TYPE_HARDWARE                  (the kit sample's shape)
    // If (2) is what DXGI wants, this same deploy proves it.
    let mut swapchain = create_swapchain_for_corewindow_traced(
        device,
        core_window,
        width,
        height,
        s.sc_trace.as_mut_ptr(),
    );

    if swapchain.is_none() {
        bump(s, 253); // cw: retrying the CoreWindow swapchain with a default-adapter device
        if let Some((dev2, ctx2, _l2, _d2)) = create_device_default_adapter() {
            if let Some(sc2) = create_swapchain_for_corewindow_traced(
                dev2,
                core_window,
                width,
                height,
                s.sc_trace.as_mut_ptr(),
            ) {
                // Adopt the second device so the RTV/pipeline below match the swapchain.
                device.release();
                context.release();
                device = dev2;
                context = ctx2;
                s.device = device;
                s.context = context;
                swapchain = Some(sc2);
            } else {
                dev2.release();
                ctx2.release();
            }
        }
    }

    let swapchain = match swapchain {
        Some(sc) => sc,
        None => {
            s.stage = SC_BASE + s.sc_trace[0];
            s.last_hr = s.sc_trace[1];
            return 2;
        }
    };
    s.swapchain = swapchain;

    finish_up_from_swapchain(s)
}

/// Attach the live swapchain to a XAML panel. **Call this on the UI thread.**
///
/// The composition route creates the swapchain during bring-up, but on WP8.1 the `SetSwapChain`
/// attach belongs to the UI thread (`Dispatcher->RunAsync` in Microsoft's own sample), so C# calls
/// this from the UI thread after a successful `wp81_rs_start`/`wp81_rs_start_upto`. Returns 0 on
/// success, 1 if there is no swapchain, 2 if no interop interface answered.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_attach_panel(panel: *mut c_void) -> i32 {
    let s = st();
    if s.swapchain.is_null() {
        return 1;
    }
    if renderer::attach_swapchain_to_panel(s.swapchain, panel) {
        0
    } else {
        2
    }
}

/// The second half of bring-up, shared by both swapchain routes: back buffer -> RTV -> pipeline ->
/// test texture -> viewport. Returns the same stage codes as [`wp81_rs_start`].
unsafe fn finish_up_from_swapchain(s: &mut State) -> i32 {
    let device = s.device;
    let context = s.context;
    let swapchain = s.swapchain;

    let get_buffer: extern "system" fn(
        *mut c_void,
        UINT,
        *const Guid,
        *mut *mut c_void,
    ) -> HRESULT = swapchain.method(IDXGISWAPCHAIN_GETBUFFER);
    let mut backbuffer = ComPtr::NULL;
    if get_buffer(
        swapchain.vtbl as *mut c_void,
        0,
        &IID_ID3D11TEXTURE2D as *const Guid,
        &mut backbuffer.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
        || backbuffer.is_null()
    {
        s.stage = 3;
        return 3;
    }
    let create_rtv: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const c_void,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATERENDERTARGETVIEW);
    let mut rtv = ComPtr::NULL;
    let hr = create_rtv(
        device.vtbl as *mut c_void,
        backbuffer.vtbl as *mut c_void,
        core::ptr::null(),
        &mut rtv.vtbl as *mut _ as *mut *mut c_void,
    );
    backbuffer.release();
    s.last_hr = hr;
    if hr < 0 || rtv.is_null() {
        s.stage = 4;
        return 4;
    }
    s.rtv = rtv;

    let pipeline = match build_pipeline(device, context) {
        Some(p) => p,
        None => {
            s.stage = 5;
            return 5;
        }
    };
    if !create_test_texture(device, s) {
        s.stage = 6;
        return 6;
    }
    s.pipeline = Some(pipeline);

    let vp = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: s.width as f32,
        Height: s.height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let set_viewports: extern "system" fn(*mut c_void, UINT, *const D3D11_VIEWPORT) =
        context.method(D3D11CONTEXT_RSSETVIEWPORTS);
    set_viewports(context.vtbl as *mut c_void, 1, &vp);

    bump(s, 99);
    s.stage = 0;
    0
}

/// A 1x1 opaque-white RGBA texture + its shader resource view (the Stage 1 test texture).
unsafe fn create_test_texture(device: ComPtr, s: &mut State) -> bool {
    let white: [u8; 4] = [255, 255, 255, 255];
    let td = D3D11_TEXTURE2D_DESC {
        Width: 1,
        Height: 1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: white.as_ptr() as *const c_void,
        SysMemPitch: 4,
        SysMemSlicePitch: 4,
    };
    let create_texture2d: extern "system" fn(
        *mut c_void,
        *const D3D11_TEXTURE2D_DESC,
        *const D3D11_SUBRESOURCE_DATA,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATETEXTURE2D);
    let mut tex = ComPtr::NULL;
    if create_texture2d(
        device.vtbl as *mut c_void,
        &td,
        &data,
        &mut tex.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        return false;
    }
    let create_srv: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const c_void,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATESHADERRESOURCEVIEW);
    let mut srv = ComPtr::NULL;
    if create_srv(
        device.vtbl as *mut c_void,
        tex.vtbl as *mut c_void,
        core::ptr::null(),
        &mut srv.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        tex.release();
        return false;
    }
    s.test_tex = tex;
    s.test_srv = srv;
    true
}

// ===========================================================================
// per-frame
// ===========================================================================

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_clear(r: f32, g: f32, b: f32, a: f32) -> i32 {
    let s = st();
    if s.route == 3 {
        // The compositor presents the finished surface, but a failed BeginDraw is a real error and
        // must not be reported as a clean frame.
        if s.surface.in_frame {
            return -2;
        }
        // Remembered, not applied: there is no render target until `BeginDraw` hands one back
        // inside `wp81_rs_draw_test_quad`. See `State::clear_color`.
        s.clear_color = [r, g, b, a];
        return s.frame_err;
    }
    if s.rtv.is_null() {
        return -1;
    }
    let clear: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT) =
        s.context.method(D3D11CONTEXT_CLEARRENDERTARGETVIEW);
    clear(s.context.vtbl as *mut c_void, s.rtv.vtbl as *mut c_void, [r, g, b, a].as_ptr());
    0
}

/// Draw one full-screen textured quad - the Stage 1 acceptance test.
///
/// This is deliberately the *whole* pipeline in one call: it binds the input layout, vertex
/// buffer, shaders, sampler, blend/raster/depth state and constant buffer, uploads the
/// projection, and issues exactly one `Draw(6, 0)` covering the quad as two triangles. If this
/// shows a quadrilateral on the phone, every piece Stage 3 needs is already proven.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_draw_test_quad() -> i32 {
    let s = st();
    let p = match s.pipeline.as_ref() {
        Some(p) => p,
        None => return -1,
    };
    let ctx = s.context;
    // `set_tick` (not `bump(s, ..)`) because `p` holds an immutable borrow of `s.pipeline` here.
    set_tick(60); // frame: Map(constant buffer) + Map(vertex buffer) + bind + Draw(6)

    // ---- which experiment this frame runs -------------------------------------------------
    //
    // The readback window is five frames long, and one looking at a screen cannot test one
    // hypothesis per deploy. So each of those frames varies exactly one pipeline setting and the
    // pixels say which setting was hiding the quad:
    //
    //   0 baseline — textured, alpha-blended, projection as designed
    //   1 no blend — `OMSetBlendState(NULL)` is D3D11's default, an *opaque copy*. If the quad
    //                 appears, blending was eating it: the pixel shader's output alpha was < 1.
    //                 If it appears **black**, the shader output zero, i.e. the texture is the
    //                 fault and the blend was merely hiding the zero.
    //   2 no blend + quad stretched to fill clip space
    //   3           + quad stretched to fill clip space (blending as normal)
    //
    // Variants 2/3 remove position as an explanation: a quad spanning `-1..1` cannot be off screen,
    // so a screen that still shows only the clear colour with it would be a draw that never
    // rasterized rather than one drawn somewhere else. All four use objects the pipeline already
    // has, so no experiment can fail by not being able to build itself.
    //
    // The variant equals the `pixel_seq` the probe will log for this frame, so the log line and
    // the experiment it belongs to cannot drift apart.
    let variant: i32 = if s.surface.pixel_seq < 4 {
        s.surface.pixel_seq + 1
    } else {
        0
    };

    // ---- open the compositor's frame, and target the surface it hands back -----------------
    //
    // On route 1 (offscreen) we draw into our own texture. On route 3 (XAML SurfaceImageSource)
    // `BeginDraw` hands back an `IDXGISurface` that lives inside the compositor's own render
    // target, and drawing into it *is* presenting. Everything after this block is identical.
    let mut rt = s.rtv.vtbl as *mut c_void;
    // Six floats, not four.
    //
    // `RSSetViewports` reads a `D3D11_VIEWPORT` — TopLeftX/Y, Width, Height, **MinDepth,
    // MaxDepth** — so an array of four leaves the depth range pointing at two floats of whatever
    // the stack happened to hold. Read as (768, 1229) that clamps to (1, 1), which clips every
    // fragment, while `ClearRenderTargetView` ignores the viewport entirely: the screen fills with
    // the clear colour and the draw lands *nothing*. That is precisely the "blue, no quad" the
    // Lumia showed, and `hostcheck` could not catch it because the desktop path builds this struct
    // properly. `MinDepth`/`MaxDepth` are written out below rather than left to be forgotten.
    let mut viewport = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: s.width as f32,
        Height: s.height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    if s.route == 3 {
        if s.surface.native.is_null() {
            s.frame_err = -1;
            return -1;
        }
        s.frame_err = 0;
        if let Err((step, hr)) = s.surface.begin_draw() {
            s.sis_trace[0] = step;
            s.sis_trace[1] = hr;
            s.last_hr = hr;
            s.frame_err = -2;
            return -2;
        }
        // From here the frame is bisected with its own ticks (62..72) rather than one 60 for the
        // whole thing: on the Lumia 925 the first build that actually got past the view hung inside
        // this function, and a single "frame: Map/bind/Draw(6)" can only say *somewhere in here*.
        // The bg watchdog logs on change, so the last line names the call that never returned.
        set_tick(62); // BeginDraw returned
        if s.surface.frame_rtv.is_null() {
            // Record what the surface actually *is* first: if the view below is refused, the phone
            // log needs the format and BindFlags to say why, and there is no second chance to read
            // them (the frame has to end before anything else can be tried).
            s.surface.inspect_surface(&s.device);
            set_tick(63); // GetDesc recorded
            let hr = s.surface.build_rtv(&s.device);
            if hr < 0 {
                s.surface.end_draw();
                s.sis_trace[0] = SIS_CREATE_RTV;
                s.sis_trace[1] = hr;
                s.last_hr = hr;
                s.frame_err = -3;
                return -3;
            }
        }
        set_tick(64); // render target view ready
        rt = s.surface.frame_rtv.vtbl as *mut c_void;
        // The offset is where the compositor parked this frame inside the source's own atlas. A
        // renderer that ignores it draws into the wrong place as soon as XAML returns a sub-rect.
        let off = s.surface.frame_offset;
        viewport.TopLeftX = off.x as f32;
        viewport.TopLeftY = off.y as f32;
        viewport.Width = s.width as f32;
        viewport.Height = s.height as f32;
    } else if s.rtv.is_null() {
        s.frame_err = -1;
        return -1;
    }

    // ---- upload the projection (one float4x4) ----
    let map: extern "system" fn(
        *mut c_void,
        *mut c_void,
        UINT,
        D3D11_MAP,
        UINT,
        *mut D3D11_MAPPED_SUBRESOURCE,
    ) -> HRESULT = ctx.method(D3D11CONTEXT_MAP);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE {
        pData: core::ptr::null_mut(),
        RowPitch: 0,
        DepthPitch: 0,
    };
    if map(
        ctx.vtbl as *mut c_void,
        p.cb.vtbl as *mut c_void,
        0,
        D3D11_MAP_WRITE_DISCARD,
        0,
        &mut mapped,
    ) < 0
    {
        return 1;
    }
    let aspect = s.height as f32 / s.width as f32;
    // Variants 2/3 stretch the quad to the whole of clip space. Same layout convention as
    // `projection`: HLSL reads this column-major, so `mul(v, M)` comes out as `x' = 2x`,
    // `y' = 2y`, `z' = z`, `w' = 1`.
    const WIDE_PROJ: [f32; 16] = [
        2.0, 0.0, 0.0, 0.0, //
        0.0, 2.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    let proj = if variant == 2 || variant == 3 {
        WIDE_PROJ
    } else {
        projection(aspect)
    };
    core::ptr::copy_nonoverlapping(
        proj.as_ptr() as *const u8,
        mapped.pData as *mut u8,
        64,
    );
    let unmap: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_UNMAP);
    unmap(ctx.vtbl as *mut c_void, p.cb.vtbl as *mut c_void, 0);
    set_tick(65); // constant buffer Map/upload/Unmap done

    // ---- upload the quad as two triangles ----
    //
    // Six vertices, not four: `Draw(4, 0)` over a TRIANGLELIST forms only *one* triangle, because
    // the fourth vertex does not complete a primitive and is simply dropped - the GPU's own
    // pipeline counters said `IAtris = 1` while half a quad landed on screen. `(BL,BR,TR)` and
    // `(BL,TR,TL)` cover the whole quad with no index buffer, which is what a sprite batcher will
    // push per quad later as well.
    let quad: [Vertex; 6] = [
        Vertex { x: -0.5, y: -0.5, z: 0.0, u: 0.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        Vertex { x: 0.5, y: -0.5, z: 0.0, u: 1.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        Vertex { x: 0.5, y: 0.5, z: 0.0, u: 1.0, v: 0.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        Vertex { x: -0.5, y: -0.5, z: 0.0, u: 0.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        Vertex { x: 0.5, y: 0.5, z: 0.0, u: 1.0, v: 0.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        Vertex { x: -0.5, y: 0.5, z: 0.0, u: 0.0, v: 0.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
    ];
    if map(
        ctx.vtbl as *mut c_void,
        p.vb.vtbl as *mut c_void,
        0,
        D3D11_MAP_WRITE_DISCARD,
        0,
        &mut mapped,
    ) < 0
    {
        return 2;
    }
    core::ptr::copy_nonoverlapping(
        quad.as_ptr() as *const u8,
        mapped.pData as *mut u8,
        core::mem::size_of_val(&quad),
    );
    unmap(ctx.vtbl as *mut c_void, p.vb.vtbl as *mut c_void, 0);
    set_tick(66); // vertex buffer Map/upload/Unmap done

    // ---- bind ----
    let set_rt: extern "system" fn(*mut c_void, UINT, *const *mut c_void, *mut c_void) =
        ctx.method(D3D11CONTEXT_OMSETRENDERTARGETS);
    set_rt(ctx.vtbl as *mut c_void, 1, &rt, core::ptr::null_mut());

    // XAML may hand back a sub-rectangle of the source, so the viewport has to move with it.
    if s.route == 3 {
        let set_vp: extern "system" fn(*mut c_void, UINT, *const D3D11_VIEWPORT) =
            ctx.method(D3D11CONTEXT_RSSETVIEWPORTS);
        set_vp(ctx.vtbl as *mut c_void, 1, &viewport);

        // Honour the clear `wp81_rs_clear` was asked for earlier in the frame - it could not run
        // then, because this view did not exist yet (see `State::clear_color`). Without it the quad
        // is painted over whatever the compositor's atlas already held, so a working draw would look
        // no different from a broken one.
        let clear: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT) =
            ctx.method(D3D11CONTEXT_CLEARRENDERTARGETVIEW);
        clear(ctx.vtbl as *mut c_void, rt, s.clear_color.as_ptr());
        set_tick(67); // clear issued
    }

    let set_layout: extern "system" fn(*mut c_void, *mut c_void) =
        ctx.method(D3D11CONTEXT_IASETINPUTLAYOUT);
    set_layout(ctx.vtbl as *mut c_void, p.input_layout.vtbl as *mut c_void);

    let set_vb: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void, *const UINT, *const UINT) =
        ctx.method(D3D11CONTEXT_IASETVERTEXBUFFERS);
    let vbv = p.vb.vtbl as *mut c_void;
    let stride = VERTEX_STRIDE;
    let offset = 0u32;
    set_vb(ctx.vtbl as *mut c_void, 0, 1, &vbv, &stride, &offset);

    let set_topo: extern "system" fn(*mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_IASETPRIMITIVETOPOLOGY);
    set_topo(ctx.vtbl as *mut c_void, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

    // `VSSetShader`/`PSSetShader` take **three** parameters - shader, class-instance array,
    // count - not two. Declaring them with only `(shader, 0)` left `NumClassInstances` reading
    // whatever happened to be in the fourth argument register: whenever that garbage was
    // non-zero with a null instance array the runtime rejected the call outright, and the draw
    // then ran with **no vertex shader**, producing no fragments at all. It is why the Lumia
    // showed a perfect clear colour and nothing else through round after round while every HRESULT
    // in the path stayed green - and why the pixel shader happened to bind (the register held a
    // zero by luck). The header's own declaration: `VSSetShader(pVertexShader,
    // ppClassInstances, NumClassInstances)`.
    let set_vs: extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_VSSETSHADER);
    set_vs(
        ctx.vtbl as *mut c_void,
        p.vs.vtbl as *mut c_void,
        core::ptr::null(),
        0,
    );
    let set_ps: extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_PSSETSHADER);
    set_ps(
        ctx.vtbl as *mut c_void,
        p.ps.vtbl as *mut c_void,
        core::ptr::null(),
        0,
    );

    let set_vscb: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
        ctx.method(D3D11CONTEXT_VSSETCONSTANTBUFFERS);
    let cbv = p.cb.vtbl as *mut c_void;
    set_vscb(ctx.vtbl as *mut c_void, 0, 1, &cbv);

    let set_srv: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
        ctx.method(D3D11CONTEXT_PSSETSHADERRESOURCES);
    let srv = s.test_srv.vtbl as *mut c_void;
    set_srv(ctx.vtbl as *mut c_void, 0, 1, &srv);

    let set_samp: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
        ctx.method(D3D11CONTEXT_PSSETSAMPLERS);
    let smp = p.sampler.vtbl as *mut c_void;
    set_samp(ctx.vtbl as *mut c_void, 0, 1, &smp);

    let set_blend: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT, UINT) =
        ctx.method(D3D11CONTEXT_OMSETBLENDSTATE);
    // Variants 1/2 bind *no* blend state. Passing null asks for D3D11's default, which is an
    // opaque copy with no blending at all — the one setting that makes whatever the pixel shader
    // produced reach the surface untouched, which is what the whole experiment is asking.
    if variant == 1 || variant == 2 {
        set_blend(ctx.vtbl as *mut c_void, core::ptr::null_mut(), core::ptr::null(), 0xffffffff);
    } else {
        set_blend(ctx.vtbl as *mut c_void, p.blend.vtbl as *mut c_void, core::ptr::null(), 0xffffffff);
    }

    let set_raster: extern "system" fn(*mut c_void, *mut c_void) =
        ctx.method(D3D11CONTEXT_RSSETSTATE);
    set_raster(ctx.vtbl as *mut c_void, p.raster.vtbl as *mut c_void);

    // No depth buffer exists here, so state that instead of inheriting the default
    // `DepthEnable = TRUE` and letting the driver decide what a depth test against an unbound
    // depth-stencil view means. Stage 1 draws 2D; this makes "2D" mean one thing everywhere.
    let set_depth: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_OMSETDEPTHSTENCILSTATE);
    set_depth(
        ctx.vtbl as *mut c_void,
        p.depth.vtbl as *mut c_void,
        0,
    );
    set_tick(68); // every pipeline slot bound

    // ---- one triangle list: two triangles, six vertices ----
    let draw: extern "system" fn(*mut c_void, UINT, UINT) = ctx.method(D3D11CONTEXT_DRAW);
    draw(ctx.vtbl as *mut c_void, 6, 0);
    s.vertex_count = 6;
    set_tick(69); // Draw(6, 0) issued

    // ---- hand the finished frame back to the compositor ----------------------------------
    //
    // A frame that was drawn into our own texture (XAML's surface refused a view) is not in the
    // surface yet: `EndDraw` publishes whatever is *in the surface*, so without this copy the
    // compositor would be shown whatever the atlas already held — a "working" draw that looks
    // exactly like a broken one. Same size, same format, one `CopyResource`, then `EndDraw`.
    if s.route == 3 && s.surface.need_copy {
        set_tick(75); // about to move the frame into XAML's surface
        let copy: extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
            ctx.method(D3D11CONTEXT_COPYRESOURCE);
        copy(
            ctx.vtbl as *mut c_void,
            // QI'd texture, for the same reason `build_rtv` uses it: `CopyResource` takes
            // `ID3D11Resource*`, and it returns *void*, so a pointer the runtime refuses is not an
            // error you get to see - it is a frame that silently never arrives.
            s.surface.frame_tex.vtbl as *mut c_void,
            s.surface.own_texture.vtbl as *mut c_void,
        );
        set_tick(76); // CopyResource issued
    }
    // ---- measure what was actually rasterized -----------------------------------------------
    //
    // Everything above can succeed and still leave an empty screen: `Draw` returns `void`, and a
    // clear that lands proves only that the view is bound. Static analysis has not found why the
    // Lumia paints the clear colour and no quad, so the frame reads itself back instead — four
    // pixels and the viewport that produced them turn "the screen is blue" into a log line that
    // says *where* the drawing stopped. Gated on `pixel_seq` so the copy and the map run for the
    // first few frames and then stop costing ~4 MB of traffic every frame.
    if s.route == 3 && s.surface.pixel_seq < 4 {
        s.surface.vdbg[0] = (viewport.TopLeftX * 100.0) as i32;
        s.surface.vdbg[1] = (viewport.TopLeftY * 100.0) as i32;
        s.surface.vdbg[2] = (viewport.Width * 100.0) as i32;
        s.surface.vdbg[3] = (viewport.Height * 100.0) as i32;
        s.surface.vdbg[4] = (viewport.MinDepth * 1000.0) as i32;
        s.surface.vdbg[5] = (viewport.MaxDepth * 1000.0) as i32;
        s.surface.probe_pixels(ctx);
        set_tick(77); // frame read back into `surface.pixels`
    }
    if s.route == 3 {
        set_tick(71); // handing the finished frame to the compositor
        let hr = s.surface.end_draw();
        if hr < 0 {
            s.sis_trace[0] = SIS_END_DRAW;
            s.sis_trace[1] = hr;
            s.last_hr = hr;
            s.frame_err = -4;
            return -4;
        }
        set_tick(72); // EndDraw returned - the frame is published
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_present() -> i32 {
    let s = st();
    // Offscreen route: the XAML compositor presents. There is nothing to Present *to* — presenting
    // into our own texture is what the draw already did. Returning success here keeps the frame
    // loop in C# uniform across routes, and `Flush` is the honest "make the GPU do it now" call.
    if s.route == 1 {
        if s.context.is_null() {
            return -1;
        }
        bump(s, 61); // frame finished (compositor owns presentation)
        return 0;
    }
    // XAML `SurfaceImageSource`: `wp81_rs_draw_test_quad` already closed the frame with `EndDraw`,
    // which is what publishes it. Report the frame's own status rather than pretending success.
    if s.route == 3 {
        bump(s, 61); // frame finished (compositor owns presentation)
        return s.frame_err;
    }
    if s.swapchain.is_null() {
        return -1;
    }
    bump(s, 70); // IDXGISwapChain::Present
    let present: extern "system" fn(*mut c_void, UINT, UINT) -> HRESULT =
        s.swapchain.method(IDXGISWAPCHAIN_PRESENT);
    let hr = present(s.swapchain.vtbl as *mut c_void, 1, 0);
    s.last_hr = hr;
    bump(s, 61); // back to "frame finished" so a later crash is not blamed on Present
    hr
}

/// Release every COM object the renderer holds and reset to the pre-start state.
///
/// Called by [`wp81_rs_stop`] and at the top of a limited [`wp81_rs_start_upto`] run, so eight
/// bisect steps do not leak eight devices on a 512 MB phone.
unsafe fn release_all(s: &mut State) {
    if let Some(p) = s.pipeline.take() {
        p.vs.release();
        p.ps.release();
        p.input_layout.release();
        p.vb.release();
        p.cb.release();
        p.sampler.release();
        p.blend.release();
        p.raster.release();
        p.depth.release();
    }
    s.test_srv.release();
    s.test_tex.release();
    s.rtv.release();
    s.rt_tex.release();
    s.surface.release_all();
    s.dxgi_device.release();
    s.gfx_surface.release();
    s.gfx_device.release();
    s.swapchain.release();
    s.context.release();
    s.device.release();
    s.test_srv = ComPtr::NULL;
    s.test_tex = ComPtr::NULL;
    s.rtv = ComPtr::NULL;
    s.rt_tex = ComPtr::NULL;
    s.dxgi_device = ComPtr::NULL;
    s.gfx_surface = ComPtr::NULL;
    s.gfx_device = ComPtr::NULL;
    s.swapchain = ComPtr::NULL;
    s.context = ComPtr::NULL;
    s.device = ComPtr::NULL;
    s.route = -1;
    s.stage = -1;
}

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_stop() {
    release_all(st());
}

/// Where start-up got to (0 = fully up), and the last HRESULT. The C# side logs both.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_stage() -> i32 {
    st().stage
}

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_last_hr() -> i32 {
    st().last_hr
}

/// A NUL-terminated name for the last HRESULT the Rust side saw, e.g. `DXGI_ERROR_INVALID_CALL`.
///
/// Returns an empty string for an HRESULT with no entry in the table, so C# can decide whether to
/// print the hex or the name.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_last_hr_name() -> *const u8 {
    crate::direct::dxgi_error_name(st().last_hr).as_ptr()
}

/// A short ASCII name for a swapchain sub-step (see the `SC_*` codes in `renderer.rs`).
///
/// The returned pointer is a NUL-terminated static string, so C# can read it with
/// `Marshal.PtrToStringAnsi` without a length. Returning a name rather than a number is what lets
/// the phone's log say `GetParent` failed rather than `stage=122`.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sc_step_name() -> *const u8 {
    let step = st().sc_trace[0];
    let s: &[u8] = match step {
        20 => b"QI IDXGIDevice\0",
        21 => b"IDXGIDevice::GetAdapter\0",
        22 => b"IDXGIAdapter::GetParent(IDXGIFactory2)\0",
        23 => b"CreateSwapChainForComposition\0",
        24 => b"SwapChainPanel QI(ISwapChainPanelNative)\0",
        25 => b"ISwapChainPanelNative::SetSwapChain\0",
        26 => b"(swapchain created)\0",
        _ => b"(none)\0",
    };
    s.as_ptr()
}

/// The raw swapchain sub-step code (20..25), for a caller that wants the number.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sc_step() -> i32 {
    st().sc_trace[0]
}

/// Which swapchain-descriptor variant succeeded: -1 = none yet, 0..N = index into the table in
/// `renderer.rs`. Lets the log record the working descriptor shape for the phone's driver.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sc_variant() -> i32 {
    st().sc_variant
}

/// A short ASCII name for the accepted swapchain-descriptor variant ("" if none).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_sc_variant_name() -> *const u8 {
    let i = st().sc_variant;
    if i < 0 || i as usize >= renderer::SC_VARIANT_NAMES.len() {
        return b"\0".as_ptr();
    }
    renderer::SC_VARIANT_NAMES[i as usize].as_ptr()
}

/// Vertices drawn by the last test quad (a sanity check the C# side can read).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_vertex_count() -> u32 {
    st().vertex_count as u32
}

/// The last native step entered (see [`wp81_rs_tick_name`]).
///
/// Read right after `wp81_rs_start` returns, and again from a timer while the frame loop runs: if
/// the process dies inside a native call, this is the highest number the faulting call reached.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_tick() -> i32 {
    st().tick
}

/// The highest tick a [`wp81_rs_start_upto`] run actually reached before it stopped or crashed.
///
/// Distinct from [`wp81_rs_tick`]: `tick` is bumped *before* a call (so it names the call being
/// attempted), while `reached` is the furthest step that completed successfully. When a limited run
/// returns 0, `reached == limit` means that step is proven good.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_reached() -> i32 {
    st().reached
}

/// A short ASCII name for a progress tick. NUL-terminated, so C# can read it with `PtrToStringAnsi`.
///
/// 1 = C# is about to call wp81_rs_start (set by the managed side)
/// 2 = create_device        (DXGI factory, adapter enum, D3D11CreateDevice)
/// 10..16 = the switch/panel attach path inside create_swapchain_for_panel_traced
/// 20 = IDXGISwapChain::GetBuffer(0)
/// 21 = ID3D11Device::CreateRenderTargetView
/// 30 = build_pipeline (D3DCompile x2, shaders, input layout, buffers, states)
/// 40 = create_test_texture
/// 50 = RSSetViewports
/// 60 = frame: Map constant buffer / Map vertex buffer / bind / Draw(6)
/// 70 = IDXGISwapChain::Present
/// 99 = fully up
// ===========================================================================
// emergency tick dump - REMOVED
// ===========================================================================
//
// There was a `wp81_rs_set_raw_log_path` / `wp81_rs_dump_raw` pair here that appended tick lines
// with Rust `std::fs`, on the theory that WinRT FileIO would deadlock under a wedged UI thread.
// It worked in principle but pulled `std::fs` + `PathBuf` + `format!` into the shipping DLL,
// which grew it from 19,968 to 116,736 bytes and added an
// `api-ms-win-core-libraryloader-l1-2-0.dll` import; the DLL then failed to load on device and the
// app crashed at the first native call. Removed. The tick visibility it was meant to provide comes
// instead from running the CoreWindow bring-up on a *pool* thread, which leaves the UI thread free
// so the existing AppLog FileIO path keeps working.

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_tick_name() -> *const u8 {
    let t = st().tick;    let s: &[u8] = match t {
        0 => b"(none)\0",
        1 => b"C# -> wp81_rs_start\0",
        2 => b"create_device (factory/adapter/D3D11CreateDevice)\0",
        3..=9 => b"create_device: inside\0",
        10 => b"swapchain: enter (QI IDXGIDevice)\0",
        11 => b"swapchain: IDXGIDevice::GetAdapter\0",
        12 => b"swapchain: IDXGIAdapter::GetParent(IDXGIFactory2)\0",
        13 => b"swapchain: CreateSwapChainForComposition\0",
        14 => b"swapchain: panel QueryInterface(ISwapChainPanelNative)\0",
        15 => b"swapchain: ISwapChainPanelNative::SetSwapChain\0",
        16 => b"swapchain: done\0",
        17 => b"swapchain: IDXGIFactory2::IsCurrent (soundness probe)\0",
        20 => b"IDXGISwapChain::GetBuffer(0)\0",
        21 => b"ID3D11Device::CreateRenderTargetView\0",
        30 => b"build_pipeline (compile shaders / create states)\0",
        40 => b"create_test_texture\0",
        50 => b"RSSetViewports\0",
        60 => b"frame: Map/bind/Draw(6)\0",
        61 => b"frame: done\0",
        // Route-3 frame bisection (added when the first build to get a view hung inside the frame:
        // one 60 for Map+bind+Draw could only say "somewhere in here").
        62 => b"frame: BeginDraw returned\0",
        63 => b"frame: surface GetDesc recorded\0",
        64 => b"frame: render target view ready\0",
        65 => b"frame: constant buffer uploaded\0",
        66 => b"frame: vertices uploaded\0",
        67 => b"frame: clear issued\0",
        68 => b"frame: pipeline bound\0",
        69 => b"frame: Draw(6) issued\0",
        71 => b"frame: about to EndDraw\0",
        72 => b"frame: EndDraw returned\0",
        // build_rtv's two attempts get a tick each: 63 then stops here tells which one killed the
        // frame, and (with the attach-time probe) the log carries `fmt`/`BindFlags` beforehand.
        73 => b"frame: RTV attempt 1 (NULL descriptor) returned\0",
        74 => b"frame: RTV attempt 2 (explicit descriptor) entered\0",
        // Set only when XAML's surface refused a view and the frame was drawn into our own
        // texture instead: 75 then no 76 says the copy into the surface never returned.
        75 => b"frame: CopyResource into XAML's surface entered\0",
        76 => b"frame: CopyResource issued\0",
        // The readback: reaching it says every call between the Draw and here returned, and the
        // four pixels it leaves behind say what any of them actually produced.
        77 => b"frame: read back into pixels\0",
        70 => b"IDXGISwapChain::Present\0",
        98 => b"surface probe: BeginDraw/GetDesc/EndDraw done\0",
        99 => b"fully up\0",
        200 => b"corewindow: pWindow was NULL\0",
        201 => b"corewindow: QI(IUnknown) on pWindow = S_OK (valid COM object)\0",
        202 => b"corewindow: QI(IUnknown) on pWindow FAILED (not the object DXGI wants)\0",
        250 => b"corewindow: entered native\0",
        251 => b"corewindow: release_all done\0",
        252 => b"corewindow: pointer probed\0",
        253 => b"cw: retrying swapchain with a default-adapter device\0",
        260 => b"cw: entered create_swapchain_for_corewindow\0",
        261 => b"cw: pWindow probe done\0",
        262 => b"cw: QI(IDXGIDevice) from D3D11 device\0",
        263 => b"cw: IDXGIDevice::GetAdapter\0",
        264 => b"cw: IDXGIAdapter::GetParent(IDXGIFactory2)\0",
        265 => b"cw: CreateSwapChainForCoreWindow v0 (STRETCH/PREMULT/2/FLIP_SEQ)\0",
        266 => b"cw: CreateSwapChainForCoreWindow v1 (STRETCH/IGNORE/2/FLIP_SEQ)\0",
        267 => b"cw: CreateSwapChainForCoreWindow v2 (NONE/PREMULT/2/FLIP_SEQ)\0",
        269 => b"cw: CreateSwapChainForCoreWindow - ALL VARIANTS FAILED\0",
        270 => b"cw: swapchain CREATED with variant v0 (STRETCH/PREMULT)\0",
        271 => b"cw: swapchain CREATED with variant v1 (STRETCH/IGNORE)\0",
        272 => b"cw: swapchain CREATED with variant v2 (NONE/PREMULT)\0",
        280 => b"offscreen: CreateTexture2D\0",
        281 => b"offscreen: CreateRenderTargetView\0",
        282 => b"offscreen: CreateDirect3D11SurfaceFromDXGISurface\0",
        283 => b"offscreen: CreateDirect3D11DeviceFromDXGIDevice\0",
        _ => b"(unknown tick)\0",
    };
    s.as_ptr()
}

// ===========================================================================
// the original probe, kept so the C# side can still ask the platform what it has
// ===========================================================================
//
// `wp81_d3d11_rs_probe` / `wp81_d3d11_rs_report` are defined (and `#[no_mangle]`) in
// `direct.rs`; re-exporting them here as wrappers would define the same symbol twice, so the
// crate simply surfaces them by path.

// ===========================================================================
// CRT-free DLL entry point
// ===========================================================================
//
// WP8.1 native DLLs must not import KERNEL32.dll, and the desktop ARM CRT's startup objects do
// exactly that, so the link runs with /NODEFAULTLIB and this supplies the entry point instead.
// (`direct.rs` also defines these; the linker resolves the duplicate with /FORCE:MULTIPLE, but
// only one may be `#[no_mangle]`-exported, so they live in one place - here - and `direct.rs`'s
// copies are `pub` for the same crate.)

/// Any code that touches floating point references this; the CRT would normally define it.
#[no_mangle]
pub static _fltused: i32 = 0;

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    _module: *mut c_void,
    _reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    // Deliberately NOT calling DisableThreadLibraryCalls: Rust std's thread-locals want the
    // thread attach/detach notifications to arrive.
    1
}

#[no_mangle]
pub unsafe extern "system" fn _DllMainCRTStartup(
    module: *mut c_void,
    reason: u32,
    reserved: *mut c_void,
) -> i32 {
    DllMain(module, reason, reserved)
}
