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

use core::ffi::c_void;
use direct::*;
use renderer::*;

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
    /// The white 1x1 texture the Stage 1 acceptance test samples.
    test_tex: ComPtr,
    test_srv: ComPtr,
    pipeline: Option<Pipeline>,
    stage: i32,
    last_hr: i32,
    /// (step, hresult) from the swapchain-attachment path, so a failure names its call.
    sc_trace: [i32; 2],
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
    test_tex: ComPtr::NULL,
    test_srv: ComPtr::NULL,
    pipeline: None,
    stage: -1,
    last_hr: 0,
    sc_trace: [0, 0],
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
    release_all(s);
    reset_sc_hrs();
    s.width = width;
    s.height = height;
    s.limit = 0;
    s.reached = 0;

    bump(s, 2);
    let (device, context, _level, _desc) = match create_device() {
        Some(v) => v,
        None => {
            s.stage = 1;
            return 1;
        }
    };
    s.device = device;
    s.context = context;

    bump(s, 10);
    // Reuse the tail of the composition path by wrapping the CoreWindow swapchain behind the same
    // "create + RTV + pipeline + quad" sequence. The swapchain itself comes from the other function.
    let swapchain = match create_swapchain_for_corewindow_traced(
        device,
        core_window,
        width,
        height,
        s.sc_trace.as_mut_ptr(),
    ) {
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
/// buffer, shaders, sampler, blend/raster state and constant buffer, uploads the projection,
/// and issues exactly one `Draw(4, 0)`. If this shows a quadrilateral on the phone, every piece
/// Stage 3 needs is already proven.
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_draw_test_quad() -> i32 {
    let s = st();
    let p = match s.pipeline.as_ref() {
        Some(p) => p,
        None => return -1,
    };
    let ctx = s.context;
    // `set_tick` (not `bump(s, ..)`) because `p` holds an immutable borrow of `s.pipeline` here.
    set_tick(60); // frame: Map(constant buffer) + Map(vertex buffer) + bind + Draw(4)

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
    let proj = projection(aspect);
    core::ptr::copy_nonoverlapping(
        proj.as_ptr() as *const u8,
        mapped.pData as *mut u8,
        64,
    );
    let unmap: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_UNMAP);
    unmap(ctx.vtbl as *mut c_void, p.cb.vtbl as *mut c_void, 0);

    // ---- upload the quad's four vertices ----
    let quad: [Vertex; 4] = [
        Vertex { x: -0.5, y: -0.5, z: 0.0, u: 0.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        Vertex { x: 0.5, y: -0.5, z: 0.0, u: 1.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
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

    // ---- bind ----
    let set_rt: extern "system" fn(*mut c_void, UINT, *const *mut c_void, *mut c_void) =
        ctx.method(D3D11CONTEXT_OMSETRENDERTARGETS);
    let rt = s.rtv.vtbl as *mut c_void;
    set_rt(ctx.vtbl as *mut c_void, 1, &rt, core::ptr::null_mut());

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

    let set_vs: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_VSSETSHADER);
    set_vs(ctx.vtbl as *mut c_void, p.vs.vtbl as *mut c_void, 0);
    let set_ps: extern "system" fn(*mut c_void, *mut c_void, UINT) =
        ctx.method(D3D11CONTEXT_PSSETSHADER);
    set_ps(ctx.vtbl as *mut c_void, p.ps.vtbl as *mut c_void, 0);

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
    set_blend(ctx.vtbl as *mut c_void, p.blend.vtbl as *mut c_void, core::ptr::null(), 0xffffffff);

    let set_raster: extern "system" fn(*mut c_void, *mut c_void) =
        ctx.method(D3D11CONTEXT_RSSETSTATE);
    set_raster(ctx.vtbl as *mut c_void, p.raster.vtbl as *mut c_void);

    // ---- one triangle list of four vertices ----
    let draw: extern "system" fn(*mut c_void, UINT, UINT) = ctx.method(D3D11CONTEXT_DRAW);
    draw(ctx.vtbl as *mut c_void, 4, 0);
    s.vertex_count = 4;
    0
}

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_present() -> i32 {
    let s = st();
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
    }
    s.test_srv.release();
    s.test_tex.release();
    s.rtv.release();
    s.swapchain.release();
    s.context.release();
    s.device.release();
    s.test_srv = ComPtr::NULL;
    s.test_tex = ComPtr::NULL;
    s.rtv = ComPtr::NULL;
    s.swapchain = ComPtr::NULL;
    s.context = ComPtr::NULL;
    s.device = ComPtr::NULL;
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
/// 60 = frame: Map constant buffer / Map vertex buffer / bind / Draw(4)
/// 70 = IDXGISwapChain::Present
/// 99 = fully up
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_tick_name() -> *const u8 {
    let t = st().tick;
    let s: &[u8] = match t {
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
        60 => b"frame: Map/bind/Draw(4)\0",
        61 => b"frame: done\0",
        70 => b"IDXGISwapChain::Present\0",
        99 => b"fully up\0",
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
