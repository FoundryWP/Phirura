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
    width: u32,
    height: u32,
    vertex_count: usize,
}

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
    width: 0,
    height: 0,
    vertex_count: 0,
};

#[inline]
unsafe fn st() -> &'static mut State {
    &mut *core::ptr::addr_of_mut!(STATE)
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
    let s = st();
    s.width = width;
    s.height = height;

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

    let swapchain = match create_swapchain_for_panel(device, panel, width, height) {
        Some(sc) => sc,
        None => {
            s.stage = 2;
            return 2;
        }
    };
    s.swapchain = swapchain;

    // Back buffer -> render target view.
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
    // ... and the 1x1 white texture the test quad samples.
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
    set_viewports(context.vtbl as *mut c_void, 1, &vp);

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
    let present: extern "system" fn(*mut c_void, UINT, UINT) -> HRESULT =
        s.swapchain.method(IDXGISWAPCHAIN_PRESENT);
    let hr = present(s.swapchain.vtbl as *mut c_void, 1, 0);
    s.last_hr = hr;
    hr
}

#[no_mangle]
pub unsafe extern "C" fn wp81_rs_stop() {
    let s = st();
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
    s.stage = -1;
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

/// Vertices drawn by the last test quad (a sanity check the C# side can read).
#[no_mangle]
pub unsafe extern "C" fn wp81_rs_vertex_count() -> u32 {
    st().vertex_count as u32
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
