//! Stage 1 of Path A: the D3D11 renderer core, owned entirely by Rust.
//!
//! This module turns the proven binding (`direct.rs`) into something that draws. It covers the
//! whole spine a renderer needs:
//!
//!   1. **Device + immediate context** - reuse the adapter path the probe proved (enumerate via
//!      `IDXGIFactory1`, skip the SOFTWARE adapter, create with `D3D_DRIVER_TYPE_UNKNOWN`).
//!   2. **Swapchain** - created *for composition* (`CreateSwapChainForComposition`) and handed to
//!      XAML by `ISwapChainPanelNative::SetSwapChain`, so the UI thread owns layout and Rust only
//!      owns pixels. A `CoreWindow`-based fallback exists because the composition route is the one
//!      unknown left.
//!   3. **Shaders** - compiled from embedded HLSL at runtime via `D3DCompile`.
//!   4. **Pipelines** - input layout for the macroquad vertex format, a dynamic vertex buffer, an
//!      alpha blend state, a rasteriser state with culling off (sprites are two-sided), and a
//!      linear/clamp sampler.
//!   5. **One draw** - a single textured quad, which is the entire primitive the whole of prpr's
//!      scene is built from.
//!
//! ## The vertex format is macroquad's, unchanged
//!
//! `core/note.rs` builds `Vertex::new(x, y, z, u, v, color)` and pushes `[Vertex; 4]` per sprite.
//! That layout is `pos: float3, uv: float2, color: float4` = **36 bytes**. Keeping the producer
//! untouched and matching the stride here is what makes Stage 3 a swap of the consumer only.
//!
//! ## Feature level 9.3
//!
//! The device reports 9.3, so shaders are `vs_4_0_level_9_3` / `ps_4_0_level_9_3`. That profile
//! has no compute/geometry stages and a smaller constant-buffer budget, none of which matters
//! here. It *is* limited in texture formats and in `SampleLevel`, so the shader sticks to a plain
//! `Sample`.

#![allow(non_snake_case, dead_code)]

use crate::direct::*;
use core::ffi::c_void;

/// macroquad's vertex: `pos.xyz`, `uv`, `rgba`. 9 floats = 36 bytes, matching `note.rs`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub u: f32,
    pub v: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const VERTEX_STRIDE: u32 = 36;

/// Max sprites per frame. prpr can put a few thousand notes on screen; this is a ceiling, not an
/// allocation policy - the buffer is created once and `Map(WRITE_DISCARD)` per frame.
pub const MAX_VERTICES: usize = 65536;

// ===========================================================================
// shaders (HLSL, level 9.3)
// ===========================================================================

/// The whole scene shader: a transformed, tinted, textured quad.
///
/// `u_proj` is fed in **already transposed** as far as the maths is concerned - HLSL's default
/// packing is column-major, and macroquad's `Mat3` (which the C# ports reuse verbatim) is
/// row-major for row vectors, so `mul(v, M)` with the HLSL default matches `v * M` in Rust. The
/// projection itself is the one already proven in the C# head:
///   `screenX = (x + 1) / 2 * W`, `screenY = (1 - y * aspect) / 2 * H`
/// expressed as an orthographic matrix so the GPU does it.
pub const QUAD_HLSL: &[u8] = b"
cbuffer Transform : register(b0)
{
    float4x4 u_proj;
};

struct VSIn
{
    float3 pos : POSITION;
    float2 uv  : TEXCOORD0;
    float4 col : COLOR0;
};

struct VSOut
{
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
    float4 col : COLOR0;
};

VSOut vs_main(VSIn i)
{
    VSOut o;
    o.pos = mul(float4(i.pos, 1.0), u_proj);
    o.uv  = i.uv;
    o.col = i.col;
    return o;
}

Texture2D    u_tex  : register(t0);
SamplerState u_samp : register(s0);

float4 ps_main(VSOut i) : SV_Target
{
    return u_tex.Sample(u_samp, i.uv) * i.col;
}
\0";

const VS_ENTRY: &[u8] = b"vs_main\0";
const PS_ENTRY: &[u8] = b"ps_main\0";
const VS_TARGET: &[u8] = b"vs_4_0_level_9_3\0";
const PS_TARGET: &[u8] = b"ps_4_0_level_9_3\0";

// D3DCOMPILE_* flags (d3dcompiler.h)
const D3DCOMPILE_ENABLE_STRICTNESS: UINT = 1 << 11;
const D3DCOMPILE_OPTIMIZATION_LEVEL3: UINT = 1 << 15;

// ===========================================================================
// pipeline state
// ===========================================================================

/// A compiled shader pair plus everything bound with it.
pub struct Pipeline {
    pub device: ComPtr,
    pub context: ComPtr,
    pub vs: ComPtr,
    pub ps: ComPtr,
    pub input_layout: ComPtr,
    pub vb: ComPtr,
    pub cb: ComPtr,
    pub sampler: ComPtr,
    pub blend: ComPtr,
    pub raster: ComPtr,
}

/// Create the device and immediate context on the first non-software adapter.
///
/// Returns `(device, context, feature_level, adapter_description)`.
pub unsafe fn create_device() -> Option<(ComPtr, ComPtr, u32, [u16; 128])> {
    let mut factory = ComPtr::NULL;
    if CreateDXGIFactory1(
        &IID_IDXGIFACTORY1 as *const Guid,
        &mut factory.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
        || factory.is_null()
    {
        return None;
    }

    let enum_adapters1: extern "system" fn(*mut c_void, UINT, *mut *mut c_void) -> HRESULT =
        factory.method(IDXGIFACTORY1_ENUMADAPTERS1);
    let mut chosen = ComPtr::NULL;
    let mut chosen_desc = DXGI_ADAPTER_DESC1::default();
    let mut i = 0u32;
    loop {
        let mut ad = ComPtr::NULL;
        if enum_adapters1(factory.vtbl as *mut c_void, i, &mut ad.vtbl as *mut _ as *mut *mut c_void) < 0
            || ad.is_null()
        {
            break;
        }
        let get_desc1: extern "system" fn(*mut c_void, *mut DXGI_ADAPTER_DESC1) -> HRESULT =
            ad.method(IDXGIADAPTER1_GETDESC1);
        let mut desc = DXGI_ADAPTER_DESC1::default();
        if get_desc1(ad.vtbl as *mut c_void, &mut desc) >= 0
            && (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) == 0
        {
            chosen = ad;
            chosen_desc = desc;
            break;
        }
        ad.release();
        i += 1;
    }
    factory.release();
    if chosen.is_null() {
        return None;
    }

    let levels = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
    ];
    let mut device = ComPtr::NULL;
    let mut context = ComPtr::NULL;
    let mut level = 0u32;
    let hr = D3D11CreateDevice(
        chosen.vtbl as *mut c_void,
        D3D_DRIVER_TYPE_UNKNOWN,
        core::ptr::null_mut(),
        0,
        levels.as_ptr(),
        levels.len() as u32,
        D3D11_SDK_VERSION,
        &mut device.vtbl as *mut _ as *mut *mut c_void,
        &mut level,
        &mut context.vtbl as *mut _ as *mut *mut c_void,
    );
    chosen.release();
    if hr < 0 || device.is_null() {
        return None;
    }
    Some((device, context, level, chosen_desc.Description))
}

/// Create a composition swapchain for a XAML `SwapChainPanel` and attach it.
///
/// `panel` is the `IInspectable*` of the `SwapChainPanel`; the native side is obtained with
/// `QueryInterface(IID_ISwapChainPanelNative)`.
///
/// `trace` receives a (step, hresult) pair at each failure point, so the phone's log can say
/// *which* call failed instead of a bare "stage 2". On-device this is the only debugger there is.
pub unsafe fn create_swapchain_for_panel(
    device: ComPtr,
    panel: *mut c_void,
    width: u32,
    height: u32,
) -> Option<ComPtr> {
    create_swapchain_for_panel_traced(device, panel, width, height, core::ptr::null_mut())
}

/// Step codes for [`create_swapchain_for_panel_traced`], surfaced through `wp81_rs_stage`.
pub const SC_QI_DXGIDEVICE: i32 = 20;
pub const SC_GET_ADAPTER: i32 = 21;
pub const SC_GET_PARENT: i32 = 22;
pub const SC_CREATE: i32 = 23;
pub const SC_PANEL_QI: i32 = 24;
pub const SC_SET_SWAPCHAIN: i32 = 25;
/// Not a failure - reports which descriptor variant was accepted (see `create_swapchain_for_panel_traced`).
pub const SC_CREATE_OK: i32 = 26;

/// The descriptor shapes tried by [`create_swapchain_for_panel_traced`], in order. Index into this
/// with `wp81_rs_sc_variant()` to name the one the device accepted.
///
/// Variant 0 is Microsoft's own Windows Phone 8.1 sample shape; the rest are fallbacks.
pub static SC_VARIANT_NAMES: [&[u8]; 4] = [
    b"FLIP_SEQUENTIAL x2 IGNORE (official WP8.1 sample)\0",
    b"FLIP_SEQUENTIAL x2 PREMULTIPLIED\0",
    b"DISCARD x1 IGNORE (control)\0",
    b"SEQUENTIAL x1 IGNORE (control)\0",
];

pub unsafe fn create_swapchain_for_panel_traced(
    device: ComPtr,
    panel: *mut c_void,
    width: u32,
    height: u32,
    trace: *mut i32,
) -> Option<ComPtr> {
    // A plain fn rather than a closure: the closure form borrows `trace`, which then conflicts
    // with the direct writes further down (E0506).
    unsafe fn note(trace: *mut i32, step: i32, hr: i32) {
        if !trace.is_null() {
            *trace = step;
            *trace.add(1) = hr;
        }
    }

    if panel.is_null() {
        note(trace, SC_PANEL_QI, -1);
        return None;
    }

    // IDXGIDevice is needed to get the adapter's factory.
    crate::set_tick(10); // QI IDXGIDevice
    let mut dxgi_device = ComPtr::NULL;
    let hr = device.query_interface(&IID_IDXGIDEVICE, &mut dxgi_device.vtbl as *mut _ as *mut *mut c_void);
    if hr < 0 || dxgi_device.is_null() {
        note(trace, SC_QI_DXGIDEVICE, hr);
        return None;
    }
    let mut adapter = ComPtr::NULL;
    // IDXGIDevice::GetAdapter is slot 7 (IUnknown 0..2, IDXGIObject 3..6).
    let get_adapter: extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = dxgi_device.method(7);
    crate::set_tick(11); // IDXGIDevice::GetAdapter
    let hr = get_adapter(dxgi_device.vtbl as *mut c_void, &mut adapter.vtbl as *mut _ as *mut *mut c_void);
    if hr < 0 || adapter.is_null() {
        dxgi_device.release();
        note(trace, SC_GET_ADAPTER, hr);
        return None;
    }
    let mut factory = ComPtr::NULL;
    // IDXGIAdapter::GetParent(IID_IDXGIFactory2) is slot 6.
    let get_parent: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
        adapter.method(6);
    crate::set_tick(12); // IDXGIAdapter::GetParent
    let hr = get_parent(
        adapter.vtbl as *mut c_void,
        &IID_IDXGIFACTORY2,
        &mut factory.vtbl as *mut _ as *mut *mut c_void,
    );
    adapter.release();
    dxgi_device.release();
    if hr < 0 || factory.is_null() {
        note(trace, SC_GET_PARENT, hr);
        return None;
    }

    // The phone's DXGI is not the desktop's: `GetParent(IID_IDXGIFactory2)` can succeed yet hand
    // back an object whose vtable is SHORTER than `IDXGIFactory2` (i.e. it is really an
    // `IDXGIFactory1` that answered the QI). Reading slot 24 from a short vtable returns whatever
    // happens to follow it and jumping there is an instant hard fault - exactly the observed
    // symptom (GetParent OK, next call dies). Snapshot the slots so the log can show how long the
    // table actually is and whether slot 24 looks like code.
    crate::dump_vtable(factory);

    // Two bisect stops live at the factory stage, and which one is used decides whether the
    // soundness probe runs:
    //
    //   limit = 13  -> run the probe, then stop before slot 24.  If THIS run dies, the probe
    //                  (`IsCurrent`, slot 13) is the faulting call and slot 24 is innocent.
    //   limit = 18  -> **skip** the probe, then stop before slot 24. If 13 dies but 18 is clean,
    //                  the probe is exonerated and `CreateSwapChainForComposition` is confirmed.
    //
    // The probe initially came back `IsCurrent = -1`, which is not a valid BOOL, so a mis-signed
    // slot 13 that smashes the frame slot 24 then trips over is a real possibility. These two runs
    // separate the two candidates without a debugger.
    if !crate::limit_is(18) {
        crate::set_tick(17); // IsCurrent soundness probe - own tick so the watchdog names it exactly
        let is_current: extern "system" fn(*mut c_void) -> BOOL = factory.method(13);
        let current = is_current(factory.vtbl as *mut c_void);
        let mut round_trip = ComPtr::NULL;
        let hr_qi = factory.query_interface(
            &IID_IDXGIFACTORY2,
            &mut round_trip.vtbl as *mut _ as *mut *mut c_void,
        );
        round_trip.release();
        crate::record_factory_probe(current, hr_qi);
    }

    // Honour a bisect stop at the factory stage. This is the only way to read the vtable (and the
    // probe results) on the phone, because the very next call is the one that dies.
    if crate::limit_is(13) || crate::limit_is(18) {
        crate::set_tick(13);
        return None;
    }

    // The descriptor shapes tried, in order. Variant 0 is the **exact** combination from
    // Microsoft's own Windows Phone 8.1 "XAML DirectX 3D shooting game" sample's `DeviceResources`
    // (`microsoftarchive/msdn-code-gallery-microsoft`, `[C++]-Windows Phone 8.1 samples`):
    //
    //     B8G8R8A8_UNORM / Stereo=IsWindowedStereoEnabled() / MSAA off /
    //     BufferCount=2 / FLIP_SEQUENTIAL / SCALING_STRETCH / ALPHA_MODE_IGNORE / Flags=0
    //
    // That sample calls `CreateSwapChainForComposition` and attaches via
    // `ISwapChainPanelNative::SetSwapChain` **on the UI thread**, then
    // `IDXGIDevice3::SetMaximumFrameLatency(1)`. It is the only authority for what this platform
    // accepts, so its shape goes first; the rest are fallbacks kept for their HRESULTs.
    //
    // All four previously came back `DXGI_ERROR_INVALID_CALL` (0x887A0001) in a run that also had
    // four bring-ups racing on one global STATE - i.e. the calls were probably handed a device the
    // other run had already released. The race is fixed on the C# side; these shapes are re-tried now
    // that only one run can be in flight.
    struct Variant {
        swap_effect: UINT,
        buffer_count: UINT,
        alpha_mode: UINT,
        name: &'static [u8],
    }
    let variants: [Variant; 4] = [
        // The official WP8.1 sample's shape.
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            buffer_count: 2,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            name: b"FLIP_SEQUENTIAL x2 ignore-alpha (official WP8.1 sample)\0",
        },
        // The same, but with premultiplied alpha (the Microsoft doc for `SwapChainPanel` forbids
        // this, but it is the shape that appeared to hard-fault first, so its HRESULT is worth seeing).
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            buffer_count: 2,
            alpha_mode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            name: b"FLIP_SEQUENTIAL x2 premultiplied\0",
        },
        // Non-flip effects: illegal for `CreateSwapChainForComposition` by spec, so these are the
        // controls - if one of them *succeeds*, the spec is not being enforced and the flip model is
        // genuinely unimplemented on this driver.
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_DISCARD,
            buffer_count: 1,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            name: b"DISCARD x1 ignore-alpha (control)\0",
        },
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_SEQUENTIAL,
            buffer_count: 1,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            name: b"SEQUENTIAL x1 ignore-alpha (control)\0",
        },
    ];

    let create_for_comp: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const DXGI_SWAP_CHAIN_DESC1,
        *mut *mut c_void,
    ) -> HRESULT = factory.method(IDXGIFACTORY2_CREATESWAPCHAINFORCOMPOSITION);

    let mut swapchain = ComPtr::NULL;
    let mut hr = E_INVALIDARG;
    let mut chosen: u32 = 0xFFFF_FFFF;
    crate::set_tick(13); // CreateSwapChainForComposition
    for (i, v) in variants.iter().enumerate() {
        let scd = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            // The official WP8.1 sample passes `IsWindowedStereoEnabled()` here; on this phone that
            // is FALSE, which is the same as 0.
            Stereo: 0,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: v.buffer_count,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: v.swap_effect,
            AlphaMode: v.alpha_mode,
            Flags: 0,
        };
        let mut sc = ComPtr::NULL;
        hr = create_for_comp(
            factory.vtbl as *mut c_void,
            device.vtbl as *mut c_void,
            &scd,
            &mut sc.vtbl as *mut _ as *mut *mut c_void,
        );
        // Record every variant's HRESULT, not just the last. With one run in flight this is what
        // says whether *any* shape is accepted, and whether the flip model errors differently from
        // the controls. `wp81_rs_sc_hr(i)` reads it back.
        crate::record_sc_hr(i, hr);
        if hr >= 0 && !sc.is_null() {
            swapchain = sc;
            chosen = i as u32;
            break;
        }
        if !sc.is_null() {
            sc.release();
        }
    }
    factory.release();

    if hr < 0 || swapchain.is_null() {
        if !trace.is_null() {
            *trace = SC_CREATE;
            *trace.add(1) = hr;
        }
        let _ = chosen;
        return None;
    }

    // Surface which variant worked, through the same trace slot the failure path uses.
    if !trace.is_null() {
        *trace = SC_CREATE_OK;
        *trace.add(1) = chosen as i32;
    }


    // Hand the swapchain to XAML. Two interop interfaces exist and a given XAML object answers
    // exactly one of them: `SwapChainPanel` -> `ISwapChainPanelNative`, `SwapChainBackgroundPanel`
    // -> `ISwapChainBackgroundPanelNative`. Rather than make the caller pick, try both; whichever
    // QIs is the one this panel is, and both have `SetSwapChain` at slot 3.
    crate::set_tick(14); // panel QI
    let panel_ptr = ComPtr { vtbl: panel as *const *const usize };
    let mut native = ComPtr::NULL;
    let mut hr = panel_ptr.query_interface(
        &IID_ISWAPCHAINPANELNATIVE,
        &mut native.vtbl as *mut _ as *mut *mut c_void,
    );
    if hr >= 0 && !native.is_null() {
        crate::record_panel_iface(0); // ISwapChainPanelNative
    } else {
        if !native.is_null() {
            native.release();
            native = ComPtr::NULL;
        }
        hr = panel_ptr.query_interface(
            &IID_ISWAPCHAINBACKGROUNDPANELNATIVE,
            &mut native.vtbl as *mut _ as *mut *mut c_void,
        );
        if hr >= 0 && !native.is_null() {
            crate::record_panel_iface(1); // ISwapChainBackgroundPanelNative
        } else {
            swapchain.release();
            note(trace, SC_PANEL_QI, hr);
            return None;
        }
    }
    let set_swapchain: extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
        native.method(ISWAPCHAINPANELNATIVE_SETSWAPCHAIN);
    crate::set_tick(15); // SetSwapChain
    let hr = set_swapchain(native.vtbl as *mut c_void, swapchain.vtbl as *mut c_void);
    native.release();
    if hr < 0 {
        swapchain.release();
        note(trace, SC_SET_SWAPCHAIN, hr);
        return None;
    }
    crate::set_tick(16); // done
    Some(swapchain)
}

/// Create a classic `HWND`-bound swapchain - the desktop verification path.
///
/// The phone has no `HWND`, so this is *not* used on WP8.1; it exists so `src/bin/hostcheck.rs`
/// can drive `create_device` + `build_pipeline` against the real D3D11 runtime on the build
/// machine. Everything it shares with `create_swapchain_for_panel` (the `IDXGIDevice` -> adapter ->
/// factory walk) is deliberately the same code, so the slots and IIDs the phone will use are the
/// ones being exercised.
pub unsafe fn create_swapchain_for_hwnd(
    device: ComPtr,
    hwnd: *mut c_void,
    width: u32,
    height: u32,
) -> Option<ComPtr> {
    let mut dxgi_device = ComPtr::NULL;
    if device.query_interface(&IID_IDXGIDEVICE, &mut dxgi_device.vtbl as *mut _ as *mut *mut c_void) < 0
        || dxgi_device.is_null()
    {
        return None;
    }
    let mut adapter = ComPtr::NULL;
    let get_adapter: extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = dxgi_device.method(7);
    if get_adapter(dxgi_device.vtbl as *mut c_void, &mut adapter.vtbl as *mut _ as *mut *mut c_void) < 0
        || adapter.is_null()
    {
        dxgi_device.release();
        return None;
    }
    let mut factory = ComPtr::NULL;
    let get_parent: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
        adapter.method(6);
    get_parent(
        adapter.vtbl as *mut c_void,
        &IID_IDXGIFACTORY1,
        &mut factory.vtbl as *mut _ as *mut *mut c_void,
    );
    adapter.release();
    dxgi_device.release();
    if factory.is_null() {
        return None;
    }

    let mut scd = DXGI_SWAP_CHAIN_DESC {
        BufferDesc_Width: width,
        BufferDesc_Height: height,
        BufferDesc_RefreshRate_Numerator: 60,
        BufferDesc_RefreshRate_Denominator: 1,
        BufferDesc_Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        BufferDesc_ScanlineOrdering: 0,
        BufferDesc_Scaling: 0,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        OutputWindow: hwnd,
        Windowed: 1,
        SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
        Flags: 0,
    };
    let create_sc: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const DXGI_SWAP_CHAIN_DESC,
        *mut *mut c_void,
    ) -> HRESULT = factory.method(IDXGIFACTORY_CREATESWAPCHAIN);
    let mut swapchain = ComPtr::NULL;
    let hr = create_sc(
        factory.vtbl as *mut c_void,
        device.vtbl as *mut c_void,
        &mut scd,
        &mut swapchain.vtbl as *mut _ as *mut *mut c_void,
    );
    factory.release();
    if hr < 0 || swapchain.is_null() {
        return None;
    }
    Some(swapchain)
}

/// Compile both stages of `QUAD_HLSL` and build every pipeline object once.
pub unsafe fn build_pipeline(device: ComPtr, context: ComPtr) -> Option<Pipeline> {
    let mut vs_blob = ComPtr::NULL;
    let mut ps_blob = ComPtr::NULL;
    let mut err = ComPtr::NULL;
    let src = QUAD_HLSL.as_ptr() as *const c_void;
    let src_len = QUAD_HLSL.len();

    let hr = D3DCompile(
        src,
        src_len,
        core::ptr::null(),
        core::ptr::null(),
        core::ptr::null_mut(),
        VS_ENTRY.as_ptr(),
        VS_TARGET.as_ptr(),
        D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
        0,
        &mut vs_blob.vtbl as *mut _ as *mut *mut c_void,
        &mut err.vtbl as *mut _ as *mut *mut c_void,
    );
    err.release();
    if hr < 0 || vs_blob.is_null() {
        return None;
    }
    let hr = D3DCompile(
        src,
        src_len,
        core::ptr::null(),
        core::ptr::null(),
        core::ptr::null_mut(),
        PS_ENTRY.as_ptr(),
        PS_TARGET.as_ptr(),
        D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
        0,
        &mut ps_blob.vtbl as *mut _ as *mut *mut c_void,
        &mut err.vtbl as *mut _ as *mut *mut c_void,
    );
    err.release();
    if hr < 0 || ps_blob.is_null() {
        vs_blob.release();
        return None;
    }

    // ID3DBlob::GetBufferPointer (slot 3) / GetBufferSize (slot 4).
    let blob_ptr: extern "system" fn(*mut c_void) -> *const c_void = vs_blob.method(3);
    let blob_len: extern "system" fn(*mut c_void) -> usize = vs_blob.method(4);

    let create_vs: extern "system" fn(
        *mut c_void,
        *const c_void,
        usize,
        *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATEVERTEXSHADER);
    let mut vs = ComPtr::NULL;
    if create_vs(
        device.vtbl as *mut c_void,
        blob_ptr(vs_blob.vtbl as *mut c_void),
        blob_len(vs_blob.vtbl as *mut c_void),
        core::ptr::null_mut(),
        &mut vs.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        vs_blob.release();
        ps_blob.release();
        return None;
    }

    let ps_ptr: extern "system" fn(*mut c_void) -> *const c_void = ps_blob.method(3);
    let ps_len: extern "system" fn(*mut c_void) -> usize = ps_blob.method(4);
    let create_ps: extern "system" fn(
        *mut c_void,
        *const c_void,
        usize,
        *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATEPIXELSHADER);
    let mut ps = ComPtr::NULL;
    if create_ps(
        device.vtbl as *mut c_void,
        ps_ptr(ps_blob.vtbl as *mut c_void),
        ps_len(ps_blob.vtbl as *mut c_void),
        core::ptr::null_mut(),
        &mut ps.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        vs.release();
        vs_blob.release();
        ps_blob.release();
        return None;
    }

    // Input layout matching `Vertex` (POSITION float3 @0, TEXCOORD float2 @12, COLOR float4 @20).
    let sem_pos = b"POSITION\0";
    let sem_uv = b"TEXCOORD\0";
    let sem_col = b"COLOR\0";
    let elems = [
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: sem_pos.as_ptr(),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: sem_uv.as_ptr(),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 12,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: sem_col.as_ptr(),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 20,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ];
    let create_layout: extern "system" fn(
        *mut c_void,
        *const D3D11_INPUT_ELEMENT_DESC,
        UINT,
        *const c_void,
        usize,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATEINPUTLAYOUT);
    let mut input_layout = ComPtr::NULL;
    if create_layout(
        device.vtbl as *mut c_void,
        elems.as_ptr(),
        elems.len() as u32,
        blob_ptr(vs_blob.vtbl as *mut c_void),
        blob_len(vs_blob.vtbl as *mut c_void),
        &mut input_layout.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        vs.release();
        ps.release();
        vs_blob.release();
        ps_blob.release();
        return None;
    }
    vs_blob.release();
    ps_blob.release();

    // Dynamic vertex buffer.
    let create_buffer: extern "system" fn(
        *mut c_void,
        *const D3D11_BUFFER_DESC,
        *const D3D11_SUBRESOURCE_DATA,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATEBUFFER);
    let vb_desc = D3D11_BUFFER_DESC {
        ByteWidth: (MAX_VERTICES * VERTEX_STRIDE as usize) as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_VERTEX_BUFFER,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut vb = ComPtr::NULL;
    if create_buffer(
        device.vtbl as *mut c_void,
        &vb_desc,
        core::ptr::null(),
        &mut vb.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        vs.release();
        ps.release();
        input_layout.release();
        return None;
    }

    // Constant buffer: one float4x4.
    let cb_desc = D3D11_BUFFER_DESC {
        ByteWidth: 64,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut cb = ComPtr::NULL;
    if create_buffer(
        device.vtbl as *mut c_void,
        &cb_desc,
        core::ptr::null(),
        &mut cb.vtbl as *mut _ as *mut *mut c_void,
    ) < 0
    {
        vs.release();
        ps.release();
        input_layout.release();
        vb.release();
        return None;
    }

    // Sampler: linear, clamp (the atlas is a plain 2D sheet; bilinear is what the C# head uses).
    let sd = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_ALWAYS,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let create_sampler: extern "system" fn(
        *mut c_void,
        *const D3D11_SAMPLER_DESC,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATESAMPLERSTATE);
    let mut sampler = ComPtr::NULL;
    if create_sampler(device.vtbl as *mut c_void, &sd, &mut sampler.vtbl as *mut _ as *mut *mut c_void) < 0 {
        vs.release();
        ps.release();
        input_layout.release();
        vb.release();
        cb.release();
        return None;
    }

    // Alpha blend: SRC_ALPHA / INV_SRC_ALPHA, alpha accumulated the same way.
    let mut bd = D3D11_BLEND_DESC {
        AlphaToCoverageEnable: 0,
        IndependentBlendEnable: 0,
        RenderTarget: [D3D11_RENDER_TARGET_BLEND_DESC::default(); 8],
    };
    bd.RenderTarget[0].BlendEnable = 1;
    bd.RenderTarget[0].SrcBlend = D3D11_BLEND_SRC_ALPHA;
    bd.RenderTarget[0].DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
    bd.RenderTarget[0].BlendOp = D3D11_BLEND_OP_ADD;
    bd.RenderTarget[0].SrcBlendAlpha = D3D11_BLEND_ONE;
    bd.RenderTarget[0].DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
    bd.RenderTarget[0].BlendOpAlpha = D3D11_BLEND_OP_ADD;
    bd.RenderTarget[0].RenderTargetWriteMask = 0x0f;
    let create_blend: extern "system" fn(
        *mut c_void,
        *const D3D11_BLEND_DESC,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATEBLENDSTATE);
    let mut blend = ComPtr::NULL;
    if create_blend(device.vtbl as *mut c_void, &bd, &mut blend.vtbl as *mut _ as *mut *mut c_void) < 0 {
        vs.release();
        ps.release();
        input_layout.release();
        vb.release();
        cb.release();
        sampler.release();
        return None;
    }

    // Rasteriser: sprites are two-sided, so no culling; no scissor (Stage 1 has no clipping).
    let rd = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: 0,
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: 1,
        ScissorEnable: 0,
        MultisampleEnable: 0,
        AntialiasedLineEnable: 0,
    };
    let create_raster: extern "system" fn(
        *mut c_void,
        *const D3D11_RASTERIZER_DESC,
        *mut *mut c_void,
    ) -> HRESULT = device.method(D3D11DEVICE_CREATERASTERIZERSTATE);
    let mut raster = ComPtr::NULL;
    if create_raster(device.vtbl as *mut c_void, &rd, &mut raster.vtbl as *mut _ as *mut *mut c_void) < 0 {
        vs.release();
        ps.release();
        input_layout.release();
        vb.release();
        cb.release();
        sampler.release();
        blend.release();
        return None;
    }

    Some(Pipeline {
        device,
        context,
        vs,
        ps,
        input_layout,
        vb,
        cb,
        sampler,
        blend,
        raster,
    })
}

/// Build the projection matrix for a viewport.
///
/// Chart coordinates are macroquad's: both axes run `-1..1` with Y **up**. D3D's clip space has Y
/// **down** and Z in `0..1`. So the only real work is:
///
///   * flip Y (`y' = -y`),
///   * split the aspect ratio the way prpr's camera does (microquad uses `zoom.y = -aspect`, i.e.
///     the Y axis is divided by the aspect ratio - hence `y' = -y * aspect`),
///   * push Z to 0 so the depth test (which is off) never clips.
///
/// Returning it row-major; HLSL's default `mul(v, M)` treats `M` as row-major too, so no transpose
/// is needed at the call site. This matches the C# head's already-proven
/// `screenY = (1 - y * aspect) / 2 * H`.
pub fn projection(aspect: f32) -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0,
        0.0, -aspect, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.5, 1.0,
    ]
}

