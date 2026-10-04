//! Desktop verification harness for the Windows Phone 8.1 D3D11 binding.
//!
//! **Host-only.** This binary must never be built for the ARM/WP8.1 target: it links `user32` and
//! D3D11/DXGI through the desktop SDK, none of which applies on the phone. `build.ps1` therefore
//! passes `--lib`; and the whole file is behind `#![cfg(...)]` as a second line of defence.
//!
//! The ARM DLL cannot execute on this machine, but every vtable slot, IID, struct layout and
//! shader in `direct.rs` / `renderer.rs` is *identical* to what the phone's runtime will validate.
//! This binary compiles those exact modules for `x86_64-pc-windows-msvc` and runs the whole Stage 1
//! path against the real desktop D3D11 runtime:
//!
//!   create_device -> swapchain (hidden HWND) -> RTV -> build_pipeline -> clear -> quad -> present
//!
//! Any wrong slot, wrong IID, bad `D3D11_INPUT_ELEMENT_DESC`, or shader that fails to compile shows
//! up here as a hard failure *before* it can ever reach the phone.
//!
//! Build + run:
//!   cargo run --bin hostcheck
//!
//! Only the swap-chain creation differs from the phone (a real `HWND` instead of
//! `ISwapChainPanelNative`); `renderer::create_swapchain_for_hwnd` provides it and shares the
//! `IDXGIDevice` -> adapter -> factory walk with the panel version.

#![cfg(all(windows, not(target_arch = "arm")))]
#![allow(non_snake_case, non_camel_case_types, dead_code)]

#[path = "../direct.rs"]
mod direct;
#[path = "../renderer.rs"]
mod renderer;

use core::ffi::c_void;
use direct::*;
use renderer::*;

/// The renderer module reports its progress through `crate::set_tick`. On the phone that lives in
/// `lib.rs` next to the exported `wp81_rs_tick`; this harness has no exports, so the same counter
/// is kept in its own static and simply ignored. It must exist for `renderer.rs` to compile here.
static mut HOST_TICK: i32 = 0;

pub(crate) unsafe fn set_tick(n: i32) {
    HOST_TICK = n;
}

/// `renderer.rs` also snapshots the factory vtable on the phone (a diagnostic for the composition
/// swapchain fault). The harness's HWND path does not need it, so this is a no-op stub.
pub(crate) unsafe fn dump_vtable(_obj: direct::ComPtr) {}

/// The bisect limit is a phone-only concept; the harness always runs to completion.
pub(crate) unsafe fn limit_is(_n: i32) -> bool {
    false
}

/// The factory soundness probe is phone-only too; the harness's HWND path succeeds and needs none.
pub(crate) unsafe fn record_factory_probe(_is_current: i32, _qi_hr: i32) {}

/// Which XAML panel-interop interface answered is only meaningful on the phone; the harness uses a
/// real `HWND` and no panel at all, so this records nothing.
pub(crate) unsafe fn record_panel_iface(_which: i32) {}

/// Per-variant HRESULT recording is a phone-only diagnostic; the harness's single HWND shape either
/// works or the run stops at step 2.
pub(crate) unsafe fn record_sc_hr(_i: usize, _hr: i32) {}

/// The CoreWindow pointer probe belongs to the `CreateSwapChainForCoreWindow` route, which the
/// harness never takes (it creates an HWND swapchain against a hidden window). No-op here, but the
/// statics must exist so `renderer.rs` references resolve.
pub(crate) unsafe fn record_corewindow(_vtbl: u32, _obj: u32) {}
pub(crate) unsafe fn record_corewindow_qi(_hr: i32) {}
pub(crate) unsafe fn record_cw_factory_stereo(_v: i32) {}

// --------------------------------------------------------------------------
// minimal user32 binding: just enough for a hidden window
// --------------------------------------------------------------------------

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(cls: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        inst: *mut c_void,
        param: *mut c_void,
    ) -> *mut c_void;
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wp: usize, lp: isize) -> isize;
    fn DestroyWindow(hwnd: *mut c_void) -> i32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

#[repr(C)]
struct WNDCLASSW {
    style: u32,
    lpfnWndProc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: *mut c_void,
    hIcon: *mut c_void,
    hCursor: *mut c_void,
    hbrBackground: *mut c_void,
    lpszMenuName: *const u16,
    lpszClassName: *const u16,
}

unsafe extern "system" fn wndproc(h: *mut c_void, m: u32, w: usize, l: isize) -> isize {
    DefWindowProcW(h, m, w, l)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Create a hidden `WS_OVERLAPPEDWINDOW` (never shown) to hand to DXGI.
unsafe fn make_window(w: u32, h: u32) -> *mut c_void {
    let hinst = GetModuleHandleW(core::ptr::null());
    let cls = wide("PhiruraHostCheck");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: core::ptr::null_mut(),
        hCursor: core::ptr::null_mut(),
        hbrBackground: core::ptr::null_mut(),
        lpszMenuName: core::ptr::null(),
        lpszClassName: cls.as_ptr(),
    };
    RegisterClassW(&wc);
    let title = wide("hostcheck");
    CreateWindowExW(
        0,
        cls.as_ptr(),
        title.as_ptr(),
        0x00CF_0000, // WS_OVERLAPPEDWINDOW; no ShowWindow, so it stays hidden
        0,
        0,
        w as i32,
        h as i32,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        hinst,
        core::ptr::null_mut(),
    )
}

fn fail(code: i32, msg: &str) -> ! {
    eprintln!("[hostcheck] FAIL: {}", msg);
    std::process::exit(code);
}

fn main() {
    unsafe {
        println!("[hostcheck] --- Stage 1 desktop verification ---");

        // ---- 1. device ----
        let (device, context, level, name) = match create_device() {
            Some(v) => v,
            None => fail(1, "create_device (no D3D11 device / adapter walk)"),
        };
        let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        println!(
            "[hostcheck] 1 device ok: feature level 0x{:x}, adapter {:?}",
            level,
            String::from_utf16_lossy(&name[..n])
        );

        // ---- 2. swapchain ----
        let (w, h) = (1280u32, 720u32);
        let hwnd = make_window(w, h);
        if hwnd.is_null() {
            fail(2, "CreateWindowExW");
        }
        let swapchain = match create_swapchain_for_hwnd(device, hwnd, w, h) {
            Some(sc) => sc,
            None => fail(2, "create_swapchain_for_hwnd"),
        };
        println!("[hostcheck] 2 swapchain ok ({}x{})", w, h);

        // ---- 3. backbuffer -> RTV (proves CreateRenderTargetView slot 9) ----
        let get_buffer: extern "system" fn(
            *mut c_void,
            UINT,
            *const Guid,
            *mut *mut c_void,
        ) -> HRESULT = swapchain.method(IDXGISWAPCHAIN_GETBUFFER);
        let mut bb = ComPtr::NULL;
        if get_buffer(
            swapchain.vtbl as *mut c_void,
            0,
            &IID_ID3D11TEXTURE2D as *const Guid,
            &mut bb.vtbl as *mut _ as *mut *mut c_void,
        ) < 0
            || bb.is_null()
        {
            fail(3, "IDXGISwapChain::GetBuffer");
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
            bb.vtbl as *mut c_void,
            core::ptr::null(),
            &mut rtv.vtbl as *mut _ as *mut *mut c_void,
        );
        bb.release();
        if hr < 0 || rtv.is_null() {
            fail(3, "CreateRenderTargetView");
        }
        println!("[hostcheck] 3 rtv ok");

        // ---- 4. pipeline (compiles the HLSL, builds VS/PS/layout/sampler/blend/raster/cb) ----
        let p = match build_pipeline(device, context) {
            Some(p) => p,
            None => fail(4, "build_pipeline - shader compile / input layout / states"),
        };
        println!("[hostcheck] 4 pipeline ok (HLSL compiled + all states created)");

        // ---- 5. 1x1 white test texture ----
        let white: [u8; 4] = [255, 255, 255, 255];
        let td = D3D11_TEXTURE2D_DESC {
            Width: 1,
            Height: 1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
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
        let create_tex: extern "system" fn(
            *mut c_void,
            *const D3D11_TEXTURE2D_DESC,
            *const D3D11_SUBRESOURCE_DATA,
            *mut *mut c_void,
        ) -> HRESULT = device.method(D3D11DEVICE_CREATETEXTURE2D);
        let mut tex = ComPtr::NULL;
        if create_tex(device.vtbl as *mut c_void, &td, &data, &mut tex.vtbl as *mut _ as *mut *mut c_void) < 0 {
            fail(5, "CreateTexture2D");
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
            fail(5, "CreateShaderResourceView");
        }
        println!("[hostcheck] 5 test texture + srv ok");

        // ---- 5b. offscreen target: the WP8.1 phone present path's *verifiable half* ----
        //
        // `create_offscreen_target` does three things: CreateTexture2D, CreateRenderTargetView, and
        // the WinRT `CreateDirect3D11SurfaceFromDXGISurface` interop. The first two are the same
        // calls step 3/5 already make, but with the phone's shape (B8G8R8A8, RENDER_TARGET |
        // SHADER_RESOURCE, USAGE_DEFAULT, explicit size) and they are exactly what breaks silently
        // when a descriptor is malformed - so they are worth running as their own case.
        //
        // The interop call cannot run here: `CreateDirect3D11SurfaceFromDXGISurface` is a WP8.1/
        // WinRT export that desktop `d3d11.dll` does not have, and there is no XAML compositor on
        // this console call to consume the result. `create_offscreen_target` returning `None` with
        // `OT_CREATE_SURFACE` while steps 41/42 passed is therefore the *expected* desktop outcome,
        // and is itself the assertion: it proves the texture and RTV succeeded and only the
        // device-only interop is missing.
        let mut ot_trace = [0i32; 2];
        match create_offscreen_target(device, w, h, ot_trace.as_mut_ptr()) {
            Some((t, r, s)) => {
                println!("[hostcheck] 5b offscreen target ok (texture + rtv + interop surface)");
                t.release();
                r.release();
                s.release();
            }
            None => {
                let step = ot_trace[0];
                if step == OT_CREATE_SURFACE {
                    println!(
                        "[hostcheck] 5b offscreen texture + rtv ok; interop surface unavailable on \
                         desktop (expected - device-only export)"
                    );
                } else {
                    fail(5, &format!(
                        "create_offscreen_target failed before the interop step: step={} hr=0x{:08X}",
                        step, ot_trace[1] as u32
                    ));
                }
            }
        }

        // ---- 5c. the WinRT device wrapper's first half: QI(IDXGIDevice) ----
        //
        // `wrap_device_for_compositor` starts by querying the D3D11 device for `IDXGIDevice`
        // (WinRT wants the DXGI device, not the D3D11 one). That QI is desktop-testable and a wrong
        // IID is a silent `E_NOINTERFACE`, so it earns its own check.
        let mut dxgi_device = ComPtr::NULL;
        let hr = device.query_interface(
            &IID_IDXGIDEVICE as *const Guid,
            &mut dxgi_device.vtbl as *mut _ as *mut *mut c_void,
        );
        if hr < 0 || dxgi_device.is_null() {
            fail(5, &format!("QI(IID_IDXGIDevice) on the D3D11 device hr=0x{:08X}", hr as u32));
        }
        dxgi_device.release();
        println!("[hostcheck] 5c QI(IDXGIDevice) for the compositor device ok");

        // ---- 6. viewport + clear (proves ClearRenderTargetView slot 46) ----
        let vp = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: w as f32,
            Height: h as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let set_viewports: extern "system" fn(*mut c_void, UINT, *const D3D11_VIEWPORT) =
            context.method(D3D11CONTEXT_RSSETVIEWPORTS);
        set_viewports(context.vtbl as *mut c_void, 1, &vp);
        let clear: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT) =
            context.method(D3D11CONTEXT_CLEARRENDERTARGETVIEW);
        clear(
            context.vtbl as *mut c_void,
            rtv.vtbl as *mut c_void,
            [0.05, 0.05, 0.1, 1.0].as_ptr(),
        );
        println!("[hostcheck] 6 clear ok (slot {})", D3D11CONTEXT_CLEARRENDERTARGETVIEW);

        // ---- 7. draw ----
        let map: extern "system" fn(
            *mut c_void,
            *mut c_void,
            UINT,
            D3D11_MAP,
            UINT,
            *mut D3D11_MAPPED_SUBRESOURCE,
        ) -> HRESULT = context.method(D3D11CONTEXT_MAP);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        if map(
            context.vtbl as *mut c_void,
            p.cb.vtbl as *mut c_void,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            &mut mapped,
        ) < 0
        {
            fail(7, "Map constant buffer");
        }
        let proj = projection(h as f32 / w as f32);
        core::ptr::copy_nonoverlapping(proj.as_ptr() as *const u8, mapped.pData as *mut u8, 64);
        let unmap: extern "system" fn(*mut c_void, *mut c_void, UINT) =
            context.method(D3D11CONTEXT_UNMAP);
        unmap(context.vtbl as *mut c_void, p.cb.vtbl as *mut c_void, 0);

        let quad: [Vertex; 4] = [
            Vertex { x: -0.5, y: -0.5, z: 0.0, u: 0.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
            Vertex { x: 0.5, y: -0.5, z: 0.0, u: 1.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
            Vertex { x: 0.5, y: 0.5, z: 0.0, u: 1.0, v: 0.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
            Vertex { x: -0.5, y: 0.5, z: 0.0, u: 0.0, v: 0.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
        ];
        if map(
            context.vtbl as *mut c_void,
            p.vb.vtbl as *mut c_void,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            &mut mapped,
        ) < 0
        {
            fail(7, "Map vertex buffer");
        }
        core::ptr::copy_nonoverlapping(
            quad.as_ptr() as *const u8,
            mapped.pData as *mut u8,
            core::mem::size_of_val(&quad),
        );
        unmap(context.vtbl as *mut c_void, p.vb.vtbl as *mut c_void, 0);

        let set_rt: extern "system" fn(*mut c_void, UINT, *const *mut c_void, *mut c_void) =
            context.method(D3D11CONTEXT_OMSETRENDERTARGETS);
        let rt = rtv.vtbl as *mut c_void;
        set_rt(context.vtbl as *mut c_void, 1, &rt, core::ptr::null_mut());
        let set_layout: extern "system" fn(*mut c_void, *mut c_void) =
            context.method(D3D11CONTEXT_IASETINPUTLAYOUT);
        set_layout(context.vtbl as *mut c_void, p.input_layout.vtbl as *mut c_void);
        let set_vb: extern "system" fn(
            *mut c_void,
            UINT,
            UINT,
            *const *mut c_void,
            *const UINT,
            *const UINT,
        ) = context.method(D3D11CONTEXT_IASETVERTEXBUFFERS);
        let vbv = p.vb.vtbl as *mut c_void;
        let stride = VERTEX_STRIDE;
        let offset = 0u32;
        set_vb(context.vtbl as *mut c_void, 0, 1, &vbv, &stride, &offset);
        let set_topo: extern "system" fn(*mut c_void, UINT) =
            context.method(D3D11CONTEXT_IASETPRIMITIVETOPOLOGY);
        set_topo(context.vtbl as *mut c_void, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        let set_vs: extern "system" fn(*mut c_void, *mut c_void, UINT) =
            context.method(D3D11CONTEXT_VSSETSHADER);
        set_vs(context.vtbl as *mut c_void, p.vs.vtbl as *mut c_void, 0);
        let set_ps: extern "system" fn(*mut c_void, *mut c_void, UINT) =
            context.method(D3D11CONTEXT_PSSETSHADER);
        set_ps(context.vtbl as *mut c_void, p.ps.vtbl as *mut c_void, 0);
        let set_vscb: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
            context.method(D3D11CONTEXT_VSSETCONSTANTBUFFERS);
        let cbv = p.cb.vtbl as *mut c_void;
        set_vscb(context.vtbl as *mut c_void, 0, 1, &cbv);
        let set_srv: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
            context.method(D3D11CONTEXT_PSSETSHADERRESOURCES);
        let srvs = srv.vtbl as *mut c_void;
        set_srv(context.vtbl as *mut c_void, 0, 1, &srvs);
        let set_samp: extern "system" fn(*mut c_void, UINT, UINT, *const *mut c_void) =
            context.method(D3D11CONTEXT_PSSETSAMPLERS);
        let smp = p.sampler.vtbl as *mut c_void;
        set_samp(context.vtbl as *mut c_void, 0, 1, &smp);
        let set_blend: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT, UINT) =
            context.method(D3D11CONTEXT_OMSETBLENDSTATE);
        // EXPERIMENT: bind *no* blend state, which is D3D11's default opaque copy. It splits the
        // two ways the centre can come back as the clear colour: with alpha blending on, a pixel
        // shader that outputs zero leaves the destination untouched exactly as a rasterizer that
        // never fired does, while opaque writes the shader's result outright — black means the
        // texture path is broken, clear means no fragments were produced at all.
        set_blend(
            context.vtbl as *mut c_void,
            core::ptr::null_mut(),
            core::ptr::null(),
            0xffff_ffff,
        );
        let set_raster: extern "system" fn(*mut c_void, *mut c_void) =
            context.method(D3D11CONTEXT_RSSETSTATE);
        set_raster(context.vtbl as *mut c_void, p.raster.vtbl as *mut c_void);

        // Read the bound state back instead of trusting the setters.
        //
        // Every setter in this path returns void, so a wrong slot, a dropped call and a correct
        // call are indistinguishable up to the moment the draw quietly produces nothing - which is
        // exactly what happened, the counters below reporting zero input-assembler vertices. The
        // getters ask the context what it thinks is bound, which turns "the draw did nothing" into
        // a specific missing or wrong piece of state. (Each getter addrefs what it returns; this
        // harness exits immediately, so the references are left to the process teardown.)
        let get_topo: extern "system" fn(*mut c_void, *mut u32) = context.method(83);
        let mut topo = 0u32;
        get_topo(context.vtbl as *mut c_void, &mut topo);
        let get_layout: extern "system" fn(*mut c_void, *mut *mut c_void) = context.method(78);
        let mut layout_out = core::ptr::null_mut();
        get_layout(context.vtbl as *mut c_void, &mut layout_out);
        let get_vb: extern "system" fn(
            *mut c_void,
            UINT,
            UINT,
            *mut *mut c_void,
            *mut UINT,
            *mut UINT,
        ) = context.method(79);
        let mut vb_out = core::ptr::null_mut();
        let mut stride_out = 0u32;
        let mut off_out = 0u32;
        get_vb(
            context.vtbl as *mut c_void,
            0,
            1,
            &mut vb_out,
            &mut stride_out,
            &mut off_out,
        );
        let get_rt: extern "system" fn(*mut c_void, UINT, *mut *mut c_void, *mut *mut c_void) =
            context.method(89);
        let mut rtv_out = core::ptr::null_mut();
        let mut dsv_out = core::ptr::null_mut();
        get_rt(
            context.vtbl as *mut c_void,
            1,
            &mut rtv_out,
            &mut dsv_out,
        );
        let get_vp: extern "system" fn(*mut c_void, *mut UINT, *mut D3D11_VIEWPORT) =
            context.method(95);
        let mut nvp = 1u32;
        let mut vp2 = D3D11_VIEWPORT {
            TopLeftX: -1.0,
            TopLeftY: -1.0,
            Width: 0.0,
            Height: 0.0,
            MinDepth: 0.0,
            MaxDepth: 0.0,
        };
        get_vp(context.vtbl as *mut c_void, &mut nvp, &mut vp2);
        let get_vs: extern "system" fn(*mut c_void, *mut *mut c_void, *mut *mut c_void, *mut UINT) =
            context.method(76);
        let mut vs_out = core::ptr::null_mut();
        // Canonical usage: a one-slot class-instance array alongside the count. Passing a null
        // array with a non-null count is the form the runtime can reject, and a rejected getter
        // would leave `vs_out` untouched - reporting an unbound shader that is in fact bound.
        let mut ci = core::ptr::null_mut();
        let mut nci = 1u32;
        get_vs(
            context.vtbl as *mut c_void,
            &mut vs_out,
            &mut ci,
            &mut nci,
        );
        if !ci.is_null() {
            let rel_ci: extern "system" fn(*mut c_void) -> u32 = unsafe {
                core::mem::transmute(*(ci as *mut *const usize).offset(2))
            };
            rel_ci(ci);
        }
        let get_ps: extern "system" fn(*mut c_void, *mut *mut c_void, *mut *mut c_void, *mut UINT) =
            context.method(74);
        let mut ps_out = core::ptr::null_mut();
        let mut ci2 = core::ptr::null_mut();
        let mut nci2 = 1u32;
        get_ps(
            context.vtbl as *mut c_void,
            &mut ps_out,
            &mut ci2,
            &mut nci2,
        );
        if !ci2.is_null() {
            let rel_ci: extern "system" fn(*mut c_void) -> u32 = unsafe {
                core::mem::transmute(*(ci2 as *mut *const usize).offset(2))
            };
            rel_ci(ci2);
        }
        let get_vscb: extern "system" fn(*mut c_void, UINT, UINT, *mut *mut c_void) =
            context.method(72);
        let mut cb_out = core::ptr::null_mut();
        get_vscb(context.vtbl as *mut c_void, 0, 1, &mut cb_out);
        let get_srv: extern "system" fn(*mut c_void, UINT, UINT, *mut *mut c_void) =
            context.method(73);
        let mut srv_out = core::ptr::null_mut();
        get_srv(context.vtbl as *mut c_void, 0, 1, &mut srv_out);
        println!(
            "[hostcheck] 7d bound: topo={} layout={:p} vs={:p} (classes={}) ps={:p} cb={:p} \
             vb={:p} stride={} off={} rtv={:p} (set={:p}) dsv={:p} vp={}x{}@{},{} srv={:p} | \
             pipeline: vs={:p} ps={:p} layout={:p}",
            topo, layout_out, vs_out, nci, ps_out, cb_out, vb_out, stride_out, off_out,
            rtv_out, rt, dsv_out, vp2.Width, vp2.Height, vp2.TopLeftX, vp2.TopLeftY, srv_out,
            p.vs.vtbl, p.ps.vtbl, p.input_layout.vtbl
        );
        // Isolate the setter from the object.
        //
        // `VSGetShader` reports nothing bound while every other state - including the pixel
        // shader, bound through the identical call - reads back correctly. Feeding the *pixel*
        // shader to `VSSetShader` splits the two remaining explanations: if the getter then
        // reports the PS, the setter works and the fault is in `p.vs` itself; if it still reports
        // nothing, the setter call is the broken link. Both results are followed by restoring the
        // real vertex shader so the draw below stays comparable.
        set_vs(context.vtbl as *mut c_void, p.ps.vtbl as *mut c_void, 0);
        let mut vs_probe = core::ptr::null_mut();
        get_vs(context.vtbl as *mut c_void, &mut vs_probe, &mut ci, &mut nci);
        println!(
            "[hostcheck] 7e VSSetShader(p.ps={:p}) -> VSGetShader={:p} (setter works if equal)",
            p.ps.vtbl, vs_probe
        );
        set_vs(context.vtbl as *mut c_void, p.vs.vtbl as *mut c_void, 0);
        let mut vs_again = core::ptr::null_mut();
        get_vs(context.vtbl as *mut c_void, &mut vs_again, &mut ci, &mut nci);
        println!(
            "[hostcheck] 7e VSSetShader(p.vs={:p}) -> VSGetShader={:p}",
            p.vs.vtbl, vs_again
        );

        // What *is* `p.vs`? `ID3D11VertexShader` and `ID3D11PixelShader` both derive straight
        // from `ID3D11DeviceChild` and declare no methods of their own, so no vtable walk can
        // tell them apart - only `QueryInterface` with the IID read from the kit's header can.
        // This is the question 7e left open: a vertex shader object that `VSSetShader` refuses,
        // and a pixel shader object that `PSSetShader` accepts, is not the same kind of object.
        let qi: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
            p.vs.method(0);
        let mut as_vs = core::ptr::null_mut();
        let hr_v = qi(
            p.vs.vtbl as *mut c_void,
            &IID_ID3D11VERTEXSHADER,
            &mut as_vs,
        );
        let mut as_ps = core::ptr::null_mut();
        let hr_v2 = qi(
            p.vs.vtbl as *mut c_void,
            &IID_ID3D11PIXELSHADER,
            &mut as_ps,
        );
        let qi_ps: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
            p.ps.method(0);
        let mut ps_as_ps = core::ptr::null_mut();
        let hr_p = qi_ps(
            p.ps.vtbl as *mut c_void,
            &IID_ID3D11PIXELSHADER,
            &mut ps_as_ps,
        );
        println!(
            "[hostcheck] 7f QI(p.vs): as_VS=0x{:08X}->{:p} as_PS=0x{:08X}->{:p} | QI(p.ps): as_PS=0x{:08X}->{:p}",
            hr_v as u32, as_vs, hr_v2 as u32, as_ps, hr_p as u32, ps_as_ps
        );

        // The GPU's own account of what this draw does. `Draw` returns void, so an HRESULT can
        // never say whether it rasterized anything; these counters split "nothing happened" into
        // the three different failures it could be: no vertices entered the input assembler, the
        // vertex shader never ran, or the VS ran and every fragment was discarded on the way out.
        #[repr(C)]
        #[derive(Default)]
        struct PipeStats {
            ia_vertices: u64,
            ia_primitives: u64,
            vs_invocations: u64,
            gs_invocations: u64,
            gs_primitives: u64,
            c_invocations: u64,
            c_primitives: u64,
            ps_invocations: u64,
            hs_invocations: u64,
            ds_invocations: u64,
            cs_invocations: u64,
        }
        #[repr(C)]
        struct QueryDesc {
            query: u32,
            misc_flags: u32,
        }
        let create_query: extern "system" fn(*mut c_void, *const QueryDesc, *mut *mut c_void) -> HRESULT =
            device.method(D3D11DEVICE_CREATEQUERY);
        let mut q = ComPtr::NULL;
        let qhr = create_query(
            device.vtbl as *mut c_void,
            &QueryDesc {
                query: D3D11_QUERY_PIPELINE_STATISTICS,
                misc_flags: 0,
            },
            &mut q.vtbl as *mut _ as *mut *mut c_void,
        );
        let have_q = qhr >= 0 && !q.is_null();
        if !have_q {
            println!("[hostcheck] 7c CreateQuery hr=0x{:08X} - no pipeline stats", qhr as u32);
        }
        let begin: extern "system" fn(*mut c_void, *mut c_void) = context.method(D3D11CONTEXT_BEGIN);
        if have_q {
            begin(context.vtbl as *mut c_void, q.vtbl as *mut c_void);
        }

        let draw: extern "system" fn(*mut c_void, UINT, UINT) = context.method(D3D11CONTEXT_DRAW);
        draw(context.vtbl as *mut c_void, 4, 0);
        println!("[hostcheck] 7 draw ok (Draw(4,0) through every bound slot)");

        let end: extern "system" fn(*mut c_void, *mut c_void) = context.method(D3D11CONTEXT_END);
        if have_q {
            end(context.vtbl as *mut c_void, q.vtbl as *mut c_void);
            let get_data: extern "system" fn(*mut c_void, *mut c_void, *mut c_void, usize, UINT) ->
                HRESULT = context.method(D3D11CONTEXT_GETDATA);
            let mut st = PipeStats::default();
            let mut tries = 0u32;
            let mut ghr = -1i32;
            while tries < 200 {
                ghr = get_data(
                    context.vtbl as *mut c_void,
                    q.vtbl as *mut c_void,
                    &mut st as *mut PipeStats as *mut c_void,
                    core::mem::size_of::<PipeStats>(),
                    0,
                );
                if ghr == 0 {
                    break;
                }
                tries += 1;
            }
            println!(
                "[hostcheck] 7c stats IAverts={} IAtris={} VSinv={} PSinv={} (hr=0x{:08X} tries={})",
                st.ia_vertices, st.ia_primitives, st.vs_invocations, st.ps_invocations,
                ghr as u32, tries
            );
            q.release();
        }

        // ---- 7b. read the pixels back: the only check that says the quad *rasterized* --------
        //
        // Every check above answers an HRESULT, and `Draw` returns `void`, so this harness could
        // print "ALL STAGE 1 CHECKS PASSED" over a frame that produced no fragments at all — which
        // is how the Lumia spent several rounds showing a perfect clear colour and no quad while
        // every gate stayed green. Reading the backbuffer turns the acceptance test into a pixel:
        // the centre must be the quad (white test texture x vertex colour) and a corner must still
        // be the clear colour. The draw path here is the same code the phone runs — same shaders,
        // same vertices, same projection, same bind order — so a failure is reproducible without a
        // deploy, and a pass moves the suspicion to what route 3 does differently.
        let get_buffer2: extern "system" fn(
            *mut c_void,
            UINT,
            *const Guid,
            *mut *mut c_void,
        ) -> HRESULT = swapchain.method(IDXGISWAPCHAIN_GETBUFFER);
        let mut bb2 = ComPtr::NULL;
        if get_buffer2(
            swapchain.vtbl as *mut c_void,
            0,
            &IID_ID3D11TEXTURE2D as *const Guid,
            &mut bb2.vtbl as *mut _ as *mut *mut c_void,
        ) < 0
            || bb2.is_null()
        {
            fail(9, "GetBuffer (readback)");
        }
        let get_desc: extern "system" fn(*mut c_void, *mut D3D11_TEXTURE2D_DESC) =
            bb2.method(ID3D11TEXTURE2D_GETDESC);
        let mut d = D3D11_TEXTURE2D_DESC::default();
        get_desc(bb2.vtbl as *mut c_void, &mut d);
        // Identical size and format, everything else stripped: a staging resource binds nothing
        // and is the only usage a CPU read is allowed from.
        let sd = D3D11_TEXTURE2D_DESC {
            Width: d.Width,
            Height: d.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: d.Format,
            SampleDesc: d.SampleDesc,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ,
            MiscFlags: 0,
        };
        let create_tex: extern "system" fn(
            *mut c_void,
            *const D3D11_TEXTURE2D_DESC,
            *const c_void,
            *mut *mut c_void,
        ) -> HRESULT = device.method(D3D11DEVICE_CREATETEXTURE2D);
        let mut staging = ComPtr::NULL;
        if create_tex(
            device.vtbl as *mut c_void,
            &sd,
            core::ptr::null(),
            &mut staging.vtbl as *mut _ as *mut *mut c_void,
        ) < 0
            || staging.is_null()
        {
            fail(9, "CreateTexture2D staging (readback)");
        }
        let copy: extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
            context.method(D3D11CONTEXT_COPYRESOURCE);
        copy(
            context.vtbl as *mut c_void,
            staging.vtbl as *mut c_void,
            bb2.vtbl as *mut c_void,
        );
        let mut m2 = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        // `D3D11_MAP_READ` is 1, not 3 — 3 is `READ_WRITE`, which a read-only staging texture
        // refuses with `E_INVALIDARG`. That single digit cost a deploy to find on the phone.
        if map(
            context.vtbl as *mut c_void,
            staging.vtbl as *mut c_void,
            0,
            D3D11_MAP_READ,
            0,
            &mut m2,
        ) < 0
        {
            fail(9, "Map staging (readback)");
        }
        let rgba = d.Format == 28; // R8G8B8A8_UNORM is R,G,B,A; BGRA8 is the other way round
        let read = |x: u32, y: u32| -> u32 {
            let base = m2.pData as *const u8;
            let p = base.add(y as usize * m2.RowPitch as usize + x as usize * 4);
            let (r, g, b, a) = if rgba {
                (*p, *p.add(1), *p.add(2), *p.add(3))
            } else {
                (*p.add(2), *p.add(1), *p, *p.add(3))
            };
            ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        };
        let center = read(w / 2, h / 2);
        let corner = read(4, 4);
        unmap(context.vtbl as *mut c_void, staging.vtbl as *mut c_void, 0);
        staging.release();
        bb2.release();
        // The quad is white texture x (1.0, 0.4, 0.2, 1.0) alpha-blended over a clear of
        // (0.05, 0.05, 0.1) — opaque, so 0xFFFF6633 exactly.
        println!(
            "[hostcheck] 7b readback center=0x{:08X} corner=0x{:08X} (want center=0xFFFF6633)",
            center, corner
        );
        let (cr, cg, cb_) = (
            (center >> 16) & 0xff,
            (center >> 8) & 0xff,
            center & 0xff,
        );
        if cr < 251 || !(94..=110).contains(&cg) || !(43..=59).contains(&cb_) {
            fail(9, "quad did not rasterize: centre pixel is not the quad colour");
        }

        // ---- 8. present ----
        let present: extern "system" fn(*mut c_void, UINT, UINT) -> HRESULT =
            swapchain.method(IDXGISWAPCHAIN_PRESENT);
        let phr = present(swapchain.vtbl as *mut c_void, 1, 0);
        if phr < 0 {
            fail(8, "Present");
        }
        println!("[hostcheck] 8 present ok hr=0x{:08x}", phr);

        // ---- teardown ----
        srv.release();
        tex.release();
        rtv.release();
        swapchain.release();
        p.vs.release();
        p.ps.release();
        p.input_layout.release();
        p.vb.release();
        p.cb.release();
        p.sampler.release();
        p.blend.release();
        p.raster.release();
        context.release();
        device.release();
        DestroyWindow(hwnd);

        println!("[hostcheck] === ALL STAGE 1 CHECKS PASSED ===");
    }
}
