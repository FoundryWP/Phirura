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

/// Create the device and immediate context.
///
/// Returns `(device, context, feature_level, adapter_description)`.
///
/// **Device shape matters, and this one was chosen by measurement, not by preference.** Two shapes
/// were tried on the device and they fail *differently*:
///
/// | shape | `CreateSwapChainForComposition` |
/// | --- | --- |
/// | enumerated adapter + `D3D_DRIVER_TYPE_UNKNOWN` (this one) | returns `DXGI_ERROR_INVALID_CALL` |
/// | default adapter + `D3D_DRIVER_TYPE_HARDWARE` (the WP8.1 sample's) | **hard-faults the process** |
///
/// A clean error is worth far more than a crash when there is no debugger, so the probe keeps the
/// enumerated-adapter shape until the swapchain question is settled. The sample's shape stays
/// available behind [`create_device_default_adapter`] for a re-test once the attach is proven.
///
/// `adapter_desc` comes from the chosen adapter's own `IDXGIAdapter1::GetDesc1`.
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
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];
    let mut device = ComPtr::NULL;
    let mut context = ComPtr::NULL;
    let mut level = 0u32;
    let hr = D3D11CreateDevice(
        chosen.vtbl as *mut c_void,
        D3D_DRIVER_TYPE_UNKNOWN,
        core::ptr::null_mut(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
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

/// The WP8.1 sample's device shape: default adapter, `D3D_DRIVER_TYPE_HARDWARE`, `BGRA_SUPPORT`.
///
/// Kept as a named alternative (not dead code) because the on-device comparison between this and
/// [`create_device`] is the evidence behind the table in that function's doc comment. Currently
/// unused by the bring-up path.
pub unsafe fn create_device_default_adapter() -> Option<(ComPtr, ComPtr, u32, [u16; 128])> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];
    let mut device = ComPtr::NULL;
    let mut context = ComPtr::NULL;
    let mut level = 0u32;
    let hr = D3D11CreateDevice(
        core::ptr::null_mut(),
        D3D_DRIVER_TYPE_HARDWARE,
        core::ptr::null_mut(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        levels.as_ptr(),
        levels.len() as u32,
        D3D11_SDK_VERSION,
        &mut device.vtbl as *mut _ as *mut *mut c_void,
        &mut level,
        &mut context.vtbl as *mut _ as *mut *mut c_void,
    );
    if hr < 0 || device.is_null() {
        return None;
    }
    Some((device, context, level, [0u16; 128]))
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
    b"DISCARD x1 IGNORE (FL9.3 control)\0",
    b"SEQUENTIAL x1 IGNORE (control)\0",
    b"FLIP_SEQUENTIAL x2 IGNORE (official WP8.1 sample)\0",
    b"FLIP_SEQUENTIAL x2 PREMULTIPLIED\0",
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

    // The `limit = 19` bisect stop wants the *other* factory source: a factory created directly by
    // `CreateDXGIFactory1`, not one derived from the device's adapter. If `CreateSwapChainForComposition`
    // succeeds on this factory but fails on the `GetParent`-derived one, the derived object was the
    // problem all along; if both fail identically, the factory is exonerated.
    if crate::limit_is(19) {
        crate::set_tick(19);
        let mut direct = ComPtr::NULL;
        if CreateDXGIFactory1(
            &IID_IDXGIFACTORY2 as *const Guid,
            &mut direct.vtbl as *mut _ as *mut *mut c_void,
        ) >= 0
            && !direct.is_null()
        {
            crate::dump_vtable(direct);
            factory.release();
            factory = direct;
        }
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
    //
    // `limit = 19` is deliberately NOT stopped here: it wants the *direct* factory above, then the
    // real `CreateSwapChainForComposition` call so its HRESULT lands in the `sc-hr` table, and only
    // then stops.
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
        // DISCARD first, deliberately. On the Adreno 225 / FL 9.3 the flip model
        // (`FLIP_SEQUENTIAL`) does not merely fail - it HARD-FAULTS this driver, and because the
        // loop tries v0 first, a fault there means v1..v3 never run and their HRESULTs stay "n/a"
        // forever. The flip model is a feature-level-10+ concept and this device is 9.3, so the
        // non-flip controls are the shapes actually worth measuring first. The spec says
        // composition requires `FLIP_SEQUENTIAL`, but a driver that faults on the spec answer is a
        // driver whose real answer has to be found by trying the alternatives.
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_DISCARD,
            buffer_count: 1,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            name: b"DISCARD x1 ignore-alpha (FL9.3 control)\0",
        },
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_SEQUENTIAL,
            buffer_count: 1,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            name: b"SEQUENTIAL x1 ignore-alpha (control)\0",
        },
        // The official WP8.1 sample's shape. Kept, but no longer first: it is the one that faults.
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            buffer_count: 2,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            name: b"FLIP_SEQUENTIAL x2 ignore-alpha (official WP8.1 sample)\0",
        },
        Variant {
            swap_effect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            buffer_count: 2,
            alpha_mode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            name: b"FLIP_SEQUENTIAL x2 premultiplied\0",
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
        // `limit = 19` wanted exactly this: the create call attempted against the direct factory,
        // with its HRESULT recorded. Report it as an intentional stop so the bisect's "rc=0 = the
        // step did not fault" invariant holds and C# logs the table.
        if crate::limit_is(19) {
            return None;
        }
        return None;    }

    // Surface which variant worked, through the same trace slot the failure path uses.
    if !trace.is_null() {
        *trace = SC_CREATE_OK;
        *trace.add(1) = chosen as i32;
    }


    // Hand the swapchain to XAML.
    //
    // This is deferred to its own function because on WP8.1 the attach is **the UI thread's job**:
    // Microsoft's own sample does `m_swapChainPanel->Dispatcher->RunAsync(CoreDispatcherPriority::High,
    // … panelNative->SetSwapChain(m_swapChain) …)`. Touching a XAML object from a pool thread is not
    // guaranteed to work, so C# calls the exported `wp81_rs_attach_panel` from the UI thread after a
    // successful create. Attaching here as well keeps this function usable standalone.
    if !attach_swapchain_to_panel(swapchain, panel) {
        swapchain.release();
        note(trace, SC_PANEL_QI, -1);
        return None;
    }
    crate::set_tick(16); // done
    Some(swapchain)
}

/// Attach an already-created swapchain to a XAML panel, on whatever thread calls this.
///
/// Two interop interfaces exist and a given XAML object answers exactly one of them:
/// `SwapChainPanel` → `ISwapChainPanelNative`, `SwapChainBackgroundPanel` →
/// `ISwapChainBackgroundPanelNative`. Rather than make the caller pick, this tries both; whichever
/// QIs is the one the panel is, and both have `SetSwapChain` at slot 3.
///
/// **Call this on the UI thread** (see the note in [`create_swapchain_for_panel_traced`]).
pub unsafe fn attach_swapchain_to_panel(swapchain: ComPtr, panel: *mut c_void) -> bool {
    if swapchain.is_null() || panel.is_null() {
        return false;
    }
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
            return false;
        }
    }
    let set_swapchain: extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
        native.method(ISWAPCHAINPANELNATIVE_SETSWAPCHAIN);
    crate::set_tick(15); // SetSwapChain
    let hr = set_swapchain(native.vtbl as *mut c_void, swapchain.vtbl as *mut c_void);
    native.release();
    if hr < 0 {
        return false;
    }
    crate::set_tick(16); // done
    true
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

/// The fallback route: `CreateSwapChainForCoreWindow`.
///
/// The plan calls for measuring both attachment routes, and this is the second one. It is
/// structurally simpler than the composition route (no panel interop interface, no `SetSwapChain`),
/// and on WP8.1 the CoreWindow *is* the app's single window, so it is a legitimate target. It lives
/// behind its own entry point so the two routes can be tried independently from C# without
/// rebuilding.
///
/// `core_window` is the `ICoreWindow*` of `Windows.UI.Core.CoreWindow.GetForCurrentThread()`,
/// passed from C# as an `IUnknown*`.
///
/// `trace` receives `(step, hresult)` exactly as [`create_swapchain_for_panel_traced`] does; step
/// codes 30..33 are unique to this path so a log line says which route produced the HRESULT.
pub unsafe fn create_swapchain_for_corewindow_traced(
    device: ComPtr,
    core_window: *mut c_void,
    width: u32,
    height: u32,
    trace: *mut i32,
) -> Option<ComPtr> {
    unsafe fn note(trace: *mut i32, step: i32, hr: i32) {
        if !trace.is_null() {
            *trace = step;
            *trace.add(1) = hr;
        }
    }

    if core_window.is_null() {
        note(trace, CW_NO_WINDOW, -1);
        return None;
    }

    // Same device -> adapter -> factory walk as the panel route.
    let mut dxgi_device = ComPtr::NULL;
    let hr = device.query_interface(&IID_IDXGIDEVICE, &mut dxgi_device.vtbl as *mut _ as *mut *mut c_void);
    if hr < 0 || dxgi_device.is_null() {
        note(trace, CW_QI_DXGIDEVICE, hr);
        return None;
    }
    let mut adapter = ComPtr::NULL;
    let get_adapter: extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = dxgi_device.method(7);
    let hr = get_adapter(dxgi_device.vtbl as *mut c_void, &mut adapter.vtbl as *mut _ as *mut *mut c_void);
    if hr < 0 || adapter.is_null() {
        dxgi_device.release();
        note(trace, CW_GET_ADAPTER, hr);
        return None;
    }
    let mut factory = ComPtr::NULL;
    let get_parent: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
        adapter.method(6);
    let hr = get_parent(
        adapter.vtbl as *mut c_void,
        &IID_IDXGIFACTORY2,
        &mut factory.vtbl as *mut _ as *mut *mut c_void,
    );
    adapter.release();
    dxgi_device.release();
    if hr < 0 || factory.is_null() {
        note(trace, CW_GET_PARENT, hr);
        return None;
    }

    let scd = DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: 0,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: 0,
    };
    let create_for_window: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *const DXGI_SWAP_CHAIN_DESC1,
        *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT = factory.method(IDXGIFACTORY2_CREATESWAPCHAINFORCOREWINDOW);
    let mut swapchain = ComPtr::NULL;
    let hr = create_for_window(
        factory.vtbl as *mut c_void,
        device.vtbl as *mut c_void,
        core_window,
        &scd,
        core::ptr::null_mut(),
        &mut swapchain.vtbl as *mut _ as *mut *mut c_void,
    );
    factory.release();
    if hr < 0 || swapchain.is_null() {
        note(trace, CW_CREATE, hr);
        return None;
    }
    if !trace.is_null() {
        *trace = CW_CREATE_OK;
        *trace.add(1) = 0;
    }
    Some(swapchain)
}

/// Step codes for [`create_swapchain_for_corewindow_traced`] (30..33).
pub const CW_NO_WINDOW: i32 = 30;
pub const CW_QI_DXGIDEVICE: i32 = 31;
pub const CW_GET_ADAPTER: i32 = 32;
pub const CW_GET_PARENT: i32 = 33;
pub const CW_CREATE: i32 = 34;
pub const CW_CREATE_OK: i32 = 35;

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

