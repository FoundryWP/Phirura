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
#[path = "../scene.rs"]
mod scene;

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

// ---------------------------------------------------------------------------
// Stage 3 helper: decode + structurally validate + build one fixture
// ---------------------------------------------------------------------------
//
// Lives outside `main` because the sequence is identical for both fixtures, and because the
// *order is a contract*: `scene::decode` fills fixed shared arenas (no per-frame allocation is
// the whole design - see scene.rs), so loading a second fixture invalidates the first one's
// `Frame`. Hence: finish everything with fixture A before calling this for B, and replay only
// the last one built (the arenas hold a single frame at a time - the same single-threaded,
// one-frame-drain rule the shim's own PORT_* buffers follow).
//
// What is asserted here is *structural only*: the stream must decode without panicking, the
// counts must agree with the header (decode's exact-size check), no NaN anywhere, every index
// must address its own batch, and the build must account for every command exactly once.
// "Coordinate is on screen" is deliberately absent - see scene.rs's module doc: the fixture on
// disk predates the pixel-space projection, so a range assertion would fail for reasons that
// have nothing to do with the renderer.
//
// Returns the build plus the frame's recorded screen size (the replay sets its viewport from
// those numbers, not from the harness's idea of the window).
fn stage3_load(bytes: &[u8], tag: &str) -> (scene::Build, f32, f32) {
    let f = match scene::decode(bytes) {
        Ok(f) => f,
        Err(e) => fail(
            10,
            &format!("[hostcheck] fixture {}: {}", tag, e.name()),
        ),
    };
    let r = scene::validate(&f);
    // The four counts printed next to `ok` are the whole "file length == header counts" proof:
    // decode only fills those slices after checking `HEADER + cmds*56 + verts*32 + idx*2 +
    // texs*16 == bytes.len()`, so a header/record disagreement surfaces as `Fail::SizeMismatch`
    // above rather than as a silent truncation.
    println!(
        "[hostcheck] 9 {} frame={} screen={}x{} cmds={} verts={} idx={} texs={} \
kinds[tex,rect,line,circle,geo]={:?} flipped={} stray_bits={} unknown_kind={} \
nan={} verts_nan={} vtx_oob={} idx_oob={} local_idx_oob={} mode_bad={} no_cmds={}",
        tag,
        f.frame_id,
        f.screen_w,
        f.screen_h,
        r.cmds,
        r.verts,
        r.idx,
        r.texs,
        r.kinds,
        r.flipped,
        r.stray_bits,
        r.unknown_kind,
        r.nan,
        r.verts_nan,
        r.vtx_oob,
        r.idx_oob,
        r.local_idx_oob,
        r.mode_bad,
        r.empty
    );
    if !r.ok() {
        fail(
            10,
            &format!("[hostcheck] fixture {}: structural validation failed", tag),
        );
    }

    let b = scene::build(&f);
    println!(
        "[hostcheck] 9 {} build: draws={} empty={} skipped={} verts={} idx={}",
        tag, b.draws, b.empty, b.skipped, b.nverts, b.nidx
    );
    // `skipped` is the one that must be zero: it counts every command the builder refused to
    // turn into a draw (unknown tag, bad DrawMode, out-of-range arena slice, no room left).
    // A valid frame never trips any of those, and a non-zero value here means the replay would
    // silently draw *less* than the recording contains.
    if b.skipped != 0 {
        fail(
            10,
            &format!(
                "[hostcheck] fixture {}: build skipped {} of {} commands",
                tag, b.skipped, r.cmds
            ),
        );
    }
    // Every command lands in exactly one bucket - drawn, geometry-with-no-indices, or refused -
    // so the three counters must add back up to the decoded command count. This is what catches
    // a command that vanished between decode and build (the identity cannot drift, because
    // `build` walks `f.cmds` one match arm per command).
    if b.draws + b.empty + b.skipped != r.cmds {
        fail(
            10,
            &format!(
                "[hostcheck] fixture {}: build accounted for {} of {} commands \
(draws={} empty={} skipped={})",
                tag,
                b.draws + b.empty + b.skipped,
                r.cmds,
                b.draws,
                b.empty,
                b.skipped
            ),
        );
    }
    (b, f.screen_w, f.screen_h)
}

// --------------------------------------------------------------------------
// Stage 3 fixtures: two frames recorded by `host_m1::dump_fixture`
// --------------------------------------------------------------------------
//
// Embedded with `include_bytes!` rather than read at runtime: this crate does no file I/O
// anywhere (`std::fs` is off the table for the port), the paths are compile-time and relative
// to *this* file so `cargo run` works from any directory, and the harness stays a single
// artifact that cannot fail because a fixture moved. `.last` is the recording's final frame -
// same stream shape hours apart is what `dump_fixture`'s two-fixture design exists to prove.
static FIXTURE: &[u8] = include_bytes!("../../../fixtures/190.bin");
static FIXTURE_LAST: &[u8] = include_bytes!("../../../fixtures/190.bin.last");

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
        // Stage 3 replays a recorded frame whose screen is 768x1229 (Lumia 925 portrait),
        // so the backbuffer must match: the replay draws in screen pixels into the current
        // viewport, and viewport == backbuffer here. Every Stage 1/2 check below is written
        // in terms of w/h, so resizing costs those checks nothing.
        let (w, h) = (768u32, 1229u32);
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

        // Six vertices, two triangles. A `Draw(4, 0)` over a TRIANGLELIST only ever forms *one*
        // triangle - the fourth vertex is dropped, and `IAtris` reported exactly that - which is
        // why the quad kept rasterizing as half a quad whose diagonal ran straight through the
        // centre pixel the readback tests. The two triangles are (BL,BR,TR) and (BL,TR,TL).
        let quad: [Vertex; 6] = [
            Vertex { x: -0.5, y: -0.5, z: 0.0, u: 0.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
            Vertex { x: 0.5, y: -0.5, z: 0.0, u: 1.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
            Vertex { x: 0.5, y: 0.5, z: 0.0, u: 1.0, v: 0.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
            Vertex { x: -0.5, y: -0.5, z: 0.0, u: 0.0, v: 1.0, r: 1.0, g: 0.4, b: 0.2, a: 1.0 },
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
        // Three parameters, per the header's own declaration: `VSSetShader(pVertexShader,
        // ppClassInstances, NumClassInstances)`. Declaring only `(shader, 0)` leaves the count
        // reading a stray argument register, and a non-zero count with a null instance array is
        // a call the runtime silently discards - which is how the draw ended up with no vertex
        // shader while every HRESULT stayed green.
        let set_vs: extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, UINT) =
            context.method(D3D11CONTEXT_VSSETSHADER);
        set_vs(
            context.vtbl as *mut c_void,
            p.vs.vtbl as *mut c_void,
            core::ptr::null(),
            0,
        );
        let set_ps: extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, UINT) =
            context.method(D3D11CONTEXT_PSSETSHADER);
        set_ps(
            context.vtbl as *mut c_void,
            p.ps.vtbl as *mut c_void,
            core::ptr::null(),
            0,
        );
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

        // Stage 1 has no depth buffer: bind a state that says so instead of inheriting the
        // default, which has `DepthEnable = TRUE` and would run a depth test against a
        // depth-stencil view this path never binds. The phone binds the same object.
        let set_depth: extern "system" fn(*mut c_void, *mut c_void, UINT) =
            context.method(D3D11CONTEXT_OMSETDEPTHSTENCILSTATE);
        set_depth(
            context.vtbl as *mut c_void,
            p.depth.vtbl as *mut c_void,
            0,
        );
        println!("[hostcheck] 7a depth-stencil state bound (DepthEnable=FALSE)");

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
        set_vs(
            context.vtbl as *mut c_void,
            p.ps.vtbl as *mut c_void,
            core::ptr::null(),
            0,
        );
        let mut vs_probe = core::ptr::null_mut();
        get_vs(context.vtbl as *mut c_void, &mut vs_probe, &mut ci, &mut nci);
        println!(
            "[hostcheck] 7e VSSetShader(p.ps={:p}) -> VSGetShader={:p} (setter works if equal)",
            p.ps.vtbl, vs_probe
        );
        set_vs(
            context.vtbl as *mut c_void,
            p.vs.vtbl as *mut c_void,
            core::ptr::null(),
            0,
        );
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
        draw(context.vtbl as *mut c_void, 6, 0);
        println!("[hostcheck] 7 draw ok (Draw(6,0) through every bound slot)");

        let end: extern "system" fn(*mut c_void, *mut c_void) = context.method(D3D11CONTEXT_END);
        if have_q {
            end(context.vtbl as *mut c_void, q.vtbl as *mut c_void);
            // The result is read *after* the pixel readback below, not here: `GetData` never
            // blocks, it returns `S_FALSE` while the GPU is still working, and asking for it in a
            // tight loop on a machine that has not yet run the draw hands back whatever the
            // freshly created query contains - zeros, which read as "the draw fed no vertices"
            // and send the investigation down entirely the wrong path. The staging-texture `Map`
            // in the readback is what actually waits for the GPU, so the counters are honest
            // once they are read on the far side of it.
        }

        // ---- 7b. read the pixels back: the only check that says the quad *rasterized* --------
        //
        // Every check above answers an HRESULT, and `Draw` returns `void`, so this harness could
        // print "ALL STAGE 1 CHECKS PASSED" over a frame that produced no fragments at all 鈥?which
        // is how the Lumia spent several rounds showing a perfect clear colour and no quad while
        // every gate stayed green. Reading the backbuffer turns the acceptance test into a pixel:
        // the centre must be the quad (white test texture x vertex colour) and a corner must still
        // be the clear colour. The draw path here is the same code the phone runs 鈥?same shaders,
        // same vertices, same projection, same bind order 鈥?so a failure is reproducible without a
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
        // `D3D11_MAP_READ` is 1, not 3 鈥?3 is `READ_WRITE`, which a read-only staging texture
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
        // Where did it actually land? The centre pixel answers for one point only, and this
        // round has already been through several wrong models of where the quad is; a bounding
        // box of everything that is neither the clear colour nor an untouched pixel answers for
        // all of them at once.
        let (mut min_x, mut max_x, mut min_y, mut max_y, mut hits) = (u32::MAX, 0u32, u32::MAX, 0u32, 0u64);
        let mut y = 0u32;
        while y < h {
            let mut x = 0u32;
            while x < w {
                let px = read(x, y);
                let (r, g, b) = ((px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
                let clear = (r as i32 - 13).abs() <= 2 && (g as i32 - 13).abs() <= 2 && (b as i32 - 26).abs() <= 3;
                if px != 0 && !clear {
                    hits += 1;
                    if x < min_x { min_x = x; }
                    if x > max_x { max_x = x; }
                    if y < min_y { min_y = y; }
                    if y > max_y { max_y = y; }
                }
                x += 2;
            }
            y += 2;
        }
        if hits > 0 {
            println!(
                "[hostcheck] 7g non-clear pixels={} bbox x=[{}..{}] y=[{}..{}] (viewport {}x{})",
                hits, min_x, max_x, min_y, max_y, w, h
            );
        } else {
            println!("[hostcheck] 7g no non-clear pixel anywhere in {}x{}", w, h);
        }
        unmap(context.vtbl as *mut c_void, staging.vtbl as *mut c_void, 0);
        staging.release();
        bb2.release();

        // The GPU has finished by now - the staging `Map` above blocked until it did - so the
        // query brackets around the draw can be read for what they counted.
        if have_q {
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

        // The quad is white texture x (1.0, 0.4, 0.2, 1.0) alpha-blended over a clear of
        // (0.05, 0.05, 0.1) 鈥?opaque, so 0xFFFF6633 exactly.
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

        // ==================================================================
        // Stage 3: decode + structurally validate + GPU-replay recorded frames
        // ==================================================================
        println!("[hostcheck] --- Stage 3 recorded-frame replay ---");

        // ---- 9. decode / validate / build both fixtures ----
        // Order matters and is documented on `stage3_load`: each `decode` refills the one shared
        // arena set, so frame 0's `Frame` is dead the moment `.last` is decoded. Nothing below
        // uses `b0`'s arenas - it is loaded first purely so *both* recordings are validated, and
        // `b0` itself survives as an ordinary value (counts only) because `Build` copies out.
        let (b0, sw0, sh0) = stage3_load(FIXTURE, "190");
        println!(
            "[hostcheck] 9a frame 0: {} draws / {} verts / {} idx, screen {}x{}",
            b0.draws, b0.nverts, b0.nidx, sw0, sh0
        );
        let (b1, sw1, sh1) = stage3_load(FIXTURE_LAST, "190.last");
        println!(
            "[hostcheck] 9b frame last: {} draws / {} verts / {} idx, screen {}x{}",
            b1.draws, b1.nverts, b1.nidx, sw1, sh1
        );
        // The replay below draws into the harness's backbuffer, which was sized in step 2 to
        // match the recording (768x1229). A mismatch is not a scene.rs bug - the projection and
        // viewport are built from the *frame's* screen size, so they stay self-consistent - but
        // it would mean step 2 drifted from the fixture and every pixel check below measures
        // two different screens at once, so it fails loudly instead of being quietly clipped.
        if sw1 != w as f32 || sh1 != h as f32 {
            fail(
                10,
                &format!(
                    "[hostcheck] fixture screen {}x{} != swapchain {}x{} (step 2 drifted)",
                    sw1, sh1, w, h
                ),
            );
        }

        // ---- 10. replay the last frame through the real pipeline ----
        // Clear to a colour no recorded command can produce, so the readback below can say what
        // the *replay* drew rather than what Stage 1 left behind. The frame's own coordinates
        // decide what lands where; because the on-disk fixture predates the pixel-space
        // projection (scene.rs module doc) the *count* is checked, never the position - and the
        // count is only reported, not asserted, until a re-recorded fixture exists.
        clear(
            context.vtbl as *mut c_void,
            rtv.vtbl as *mut c_void,
            [0.0, 0.0, 0.0, 1.0].as_ptr(),
        );

        // The GPU's own account of the replay. `DrawIndexed` returns void like every other draw
        // call, so "the draws were issued" and "geometry reached the input assembler" are only
        // distinguishable through these counters - and that distinction is exactly what a
        // mis-decoded command stream (bad index range, zero-count item, wrong topology) would
        // break, which is why Stage 3 needs its own query rather than reusing Stage 1's.
        let mut q3 = ComPtr::NULL;
        let qhr3 = create_query(
            device.vtbl as *mut c_void,
            &QueryDesc {
                query: D3D11_QUERY_PIPELINE_STATISTICS,
                misc_flags: 0,
            },
            &mut q3.vtbl as *mut _ as *mut *mut c_void,
        );
        let have_q3 = qhr3 >= 0 && !q3.is_null();
        if !have_q3 {
            println!(
                "[hostcheck] 10 CreateQuery hr=0x{:08X} - no pipeline stats for the replay",
                qhr3 as u32
            );
        }
        if have_q3 {
            begin(context.vtbl as *mut c_void, q3.vtbl as *mut c_void);
        }

        // Bind set is entirely inside scene::draw; `srv` is the 1x1 white stand-in every texture
        // id resolves to (fixture pixels are omitted by design - see scene.rs's module doc).
        let dr = scene::draw(&p, rtv, srv, sw1, sh1, &b1);
        if dr != 0 {
            fail(
                11,
                &format!(
                    "[hostcheck] scene::draw step {} failed, LAST_HR=0x{:08X}",
                    dr,
                    scene::LAST_HR as u32
                ),
            );
        }
        println!(
            "[hostcheck] 10 scene::draw ok ({} DrawIndexed calls issued)",
            b1.draws
        );
        if have_q3 {
            end(context.vtbl as *mut c_void, q3.vtbl as *mut c_void);
        }

        // ---- 10b. wait for the GPU, then read the replay's pixels back ----
        // Same two-step as Stage 1's 7b/7c: the staging `Map` blocks until the GPU is done (and
        // is therefore also what makes the query result trustworthy), *then* `GetData` runs. The
        // backbuffer desc/format (`d`, `rgba`) come from step 7b and still describe this target.
        let mut bb3 = ComPtr::NULL;
        if get_buffer2(
            swapchain.vtbl as *mut c_void,
            0,
            &IID_ID3D11TEXTURE2D as *const Guid,
            &mut bb3.vtbl as *mut _ as *mut *mut c_void,
        ) < 0
            || bb3.is_null()
        {
            fail(11, "GetBuffer (replay readback)");
        }
        let mut st3 = ComPtr::NULL;
        if create_tex(
            device.vtbl as *mut c_void,
            &sd,
            core::ptr::null(),
            &mut st3.vtbl as *mut _ as *mut *mut c_void,
        ) < 0
            || st3.is_null()
        {
            fail(11, "CreateTexture2D staging (replay readback)");
        }
        copy(
            context.vtbl as *mut c_void,
            st3.vtbl as *mut c_void,
            bb3.vtbl as *mut c_void,
        );
        let mut m3 = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        if map(
            context.vtbl as *mut c_void,
            st3.vtbl as *mut c_void,
            0,
            D3D11_MAP_READ,
            0,
            &mut m3,
        ) < 0
        {
            fail(11, "Map staging (replay readback)");
        }
        // Fresh closure over `m3`: the Stage 1 `read` closure points at `m2`'s unmapped region,
        // and dereferencing that would be use-after-unmap, not a stale value.
        let read3 = |x: u32, y: u32| -> u32 {
            let base = m3.pData as *const u8;
            let p = base.add(y as usize * m3.RowPitch as usize + x as usize * 4);
            let (r, g, b, a) = if rgba {
                (*p, *p.add(1), *p.add(2), *p.add(3))
            } else {
                (*p.add(2), *p.add(1), *p, *p.add(3))
            };
            ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        };
        let (mut hits3, mut minx, mut maxx, mut miny, mut maxy) =
            (0u64, u32::MAX, 0u32, u32::MAX, 0u32);
        let mut y3 = 0u32;
        while y3 < h {
            let mut x3 = 0u32;
            while x3 < w {
                if read3(x3, y3) != 0 {
                    hits3 += 1;
                    if x3 < minx { minx = x3; }
                    if x3 > maxx { maxx = x3; }
                    if y3 < miny { miny = y3; }
                    if y3 > maxy { maxy = y3; }
                }
                x3 += 2;
            }
            y3 += 2;
        }
        unmap(context.vtbl as *mut c_void, st3.vtbl as *mut c_void, 0);
        if hits3 > 0 {
            println!(
                "[hostcheck] 10b non-black pixels={} bbox x=[{}..{}] y=[{}..{}] (info only: \
fixture coords predate the pixel-space projection)",
                hits3, minx, maxx, miny, maxy
            );
        } else {
            println!(
                "[hostcheck] 10b no non-black pixel anywhere in {}x{} (info only - the stale \
fixture carries world-space coordinates, so where it lands is exactly what a re-record will \
settle)",
                w, h
            );
        }
        st3.release();
        bb3.release();

        // ---- 10c. the GPU's account of the replay ----
        // Now that the staging `Map` above has blocked, `GetData` reads real counters.
        if have_q3 {
            let get_data: extern "system" fn(*mut c_void, *mut c_void, *mut c_void, usize, UINT) ->
                HRESULT = context.method(D3D11CONTEXT_GETDATA);
            let mut stq = PipeStats::default();
            let mut tries = 0u32;
            let mut ghr = -1i32;
            while tries < 200 {
                ghr = get_data(
                    context.vtbl as *mut c_void,
                    q3.vtbl as *mut c_void,
                    &mut stq as *mut PipeStats as *mut c_void,
                    core::mem::size_of::<PipeStats>(),
                    0,
                );
                if ghr == 0 {
                    break;
                }
                tries += 1;
            }
            println!(
                "[hostcheck] 10c stats IAverts={} IAtris={} VSinv={} PSinv={} (want IAverts={} \
== nidx of the build, hr=0x{:08X} tries={})",
                stq.ia_vertices, stq.ia_primitives, stq.vs_invocations, stq.ps_invocations,
                b1.nidx, ghr as u32, tries
            );
            // The one hard GPU assertion of Stage 3. Every draw item is DrawIndexed over u16
            // indices, so the input assembler must have consumed exactly `b1.nidx` vertices -
            // > 0 proves geometry reached the IA at all, and == proves the index ranges the
            // builder emitted are the ranges the draw side issued (a rebasing or range bug
            // shows up here as a mismatch rather than as a silently mangled frame).
            if stq.ia_vertices == 0 {
                fail(11, "replay drew nothing: IA consumed 0 vertices (draw items not issued?)");
            }
            if stq.ia_vertices != b1.nidx as u64 {
                fail(
                    11,
                    &format!(
                        "[hostcheck] IA vertices {} != build idx count {}",
                        stq.ia_vertices, b1.nidx
                    ),
                );
            }
            q3.release();
        }

        // ---- teardown ----
        scene::release();
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
        p.depth.release();
        context.release();
        device.release();
        DestroyWindow(hwnd);

        println!("[hostcheck] === ALL STAGE 1 CHECKS PASSED ===");
        println!("[hostcheck] === ALL STAGE 3 REPLAY CHECKS PASSED ===");
    }
}


