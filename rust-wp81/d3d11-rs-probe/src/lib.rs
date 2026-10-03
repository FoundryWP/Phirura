//! Feasibility probe for "Rust renders itself" (Path A) on Windows Phone 8.1.
//!
//! WP8.1 has no OpenGL, so `prpr`'s GL renderer cannot run as written. The only GPU API is D3D11
//! (measured on device: Adreno 225, feature level 9.3). The question this file answers is narrower
//! than "does the port work": *can Rust bind D3D11 at all, and does that binding import anything
//! the phone does not have?*
//!
//! The functions below are chosen to be the smallest set that still touches every layer a real
//! renderer needs: device creation, resource creation, view creation, and a draw call. Once this
//! links, `tools/check_imports.py` on the result says whether the binding is shippable.
//!
//! Nothing here is called. It only has to compile and link, so the linker keeps the import
//! descriptors and the import table can be read.

#![allow(non_snake_case)]

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_9_3, D3D_PRIMITIVE_TOPOLOGY,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1};

const LEVELS: [D3D_FEATURE_LEVEL; 1] = [D3D_FEATURE_LEVEL_9_3];

/// Create a device on an explicit adapter with the driver type D3D requires when one is supplied.
///
/// `D3D_DRIVER_TYPE_UNKNOWN` is not a fallback here, it is mandatory: passing HARDWARE together
/// with a real adapter is an error, and passing a null adapter with HARDWARE measured as WARP on
/// this device (the reason the C++ probe enumerates adapters instead of taking the shortcut).
unsafe fn create_device(adapter: &IDXGIAdapter1) -> windows::core::Result<ID3D11Device> {
    let mut device: Option<ID3D11Device> = None;
    let mut level = D3D_FEATURE_LEVEL(0);
    let mut context: Option<ID3D11DeviceContext> = None;
    D3D11CreateDevice(
        adapter,
        D3D_DRIVER_TYPE_UNKNOWN,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_FLAG(0),
        Some(&LEVELS),
        D3D11_SDK_VERSION,
        Some(&mut device),
        Some(&mut level),
        Some(&mut context),
    )?;
    Ok(device.unwrap())
}

/// Every stage of the renderer that binds a D3D entry point, in one function so nothing is pruned.
///
/// The order is the order a real renderer uses them: enumerate a GPU, create the device, make a
/// texture and an RTV, make a vertex buffer, upload a shader, set up the pipeline, draw, present.
pub unsafe fn touch_everything() -> windows::core::Result<()> {
    // ---- DXGI: adapter enumeration (a NULL adapter with HARDWARE gave WARP on this phone) ----
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0)?;
    let _desc = adapter.GetDesc1()?;

    // ---- device + immediate context ----
    let device = create_device(&adapter)?;
    let context = device.GetImmediateContext()?;

    // ---- a render target ----
    let desc = D3D11_TEXTURE2D_DESC {
        Width: 64,
        Height: 64,
        MipLevels: 1,
        ArraySize: 1,
        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut target: Option<ID3D11Texture2D> = None;
    device.CreateTexture2D(&desc, None, Some(&mut target))?;
    let target = target.unwrap();
    // `pprtview` is an `Option<*mut Option<...>>` out-parameter, not an `Option`, so the call
    // needs the address; the windows 0.62 signature is the same shape as the raw COM call.
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    device.CreateRenderTargetView(&target, None, Some(&mut rtv))?;
    let rtv = rtv.unwrap();
    context.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);

    // ---- a vertex buffer and a shader: the "draw" half of the pipeline ----
    #[repr(C)]
    struct Vertex {
        pos: [f32; 2],
    }
    let vertices = [Vertex { pos: [0.0, 1.0] }, Vertex { pos: [1.0, -1.0] }];
    let vbuf_desc = D3D11_BUFFER_DESC {
        ByteWidth: (vertices.len() * core::mem::size_of::<Vertex>()) as u32,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: vertices.as_ptr() as *const _,
        SysMemPitch: 0,
        SysMemSlicePitch: 0,
    };
    let mut vbuf: Option<ID3D11Buffer> = None;
    device.CreateBuffer(&vbuf_desc, Some(&data), Some(&mut vbuf))?;

    context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY(4));
    context.Draw(2, 0);
    context.Flush();

    Ok(())
}

/// A `#[no_mangle]` entry point, so the linker cannot drop the whole probe as unreachable when
/// this is linked into a DLL with an explicit export list.
#[no_mangle]
pub extern "C" fn wp81_d3d11_rs_touch() -> i32 {
    unsafe {
        match touch_everything() {
            Ok(()) => 0,
            Err(e) => e.code().0,
        }
    }
}
