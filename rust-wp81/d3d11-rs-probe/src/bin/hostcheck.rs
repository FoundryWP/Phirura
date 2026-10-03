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
        set_blend(
            context.vtbl as *mut c_void,
            p.blend.vtbl as *mut c_void,
            core::ptr::null(),
            0xffff_ffff,
        );
        let set_raster: extern "system" fn(*mut c_void, *mut c_void) =
            context.method(D3D11CONTEXT_RSSETSTATE);
        set_raster(context.vtbl as *mut c_void, p.raster.vtbl as *mut c_void);
        let draw: extern "system" fn(*mut c_void, UINT, UINT) = context.method(D3D11CONTEXT_DRAW);
        draw(context.vtbl as *mut c_void, 4, 0);
        println!("[hostcheck] 7 draw ok (Draw(4,0) through every bound slot)");

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
