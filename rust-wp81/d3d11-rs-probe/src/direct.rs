//! Direct D3D11 binding probe - the *shippable* form of Path A.
//!
//! The other probe (`../src/lib.rs`) proved `windows` 0.62.2 compiles for this target, but it also
//! showed that the full crate drags in import descriptors for `kernel32.dll`, `oleaut32.dll`,
//! `combase.dll`, `rpcrt4.dll`, `user32.dll` and `ntdll.dll` - libraries WP8.1 does not have. Those
//! come from `windows-core`'s `link!` uses for ABI plumbing (and from third-party crates' own
//! raw-dylib bindings), not from the D3D11 entry points themselves.
//!
//! This crate strips the question down to the one that matters: **if Rust binds the D3D11 and DXGI
//! entry points directly, with WP8.1's own module names, does the import table stay clean?**
//!
//! It declares the functions by hand and binds them to:
//!   * `d3d11.dll` - present on WP8.1 (the C++ probe already enumerated the GPU through it)
//!   * `dxgi.dll`  - present on WP8.1
//!
//! No `windows` crate, no `windows-core`, no `windows-link`. Just `extern "system"` plus
//! `#[link(name = "d3d11.dll")]`. That is the whole binding surface a renderer needs, and it is
//! small enough to write out.
//!
//! The COM vtable calls are done by hand: a `#[repr(C)]` struct whose first field is `*const
//! *const usize` (the vtable), and a helper that fetches method N. This is exactly what the
//! `windows` crate does internally, minus the ABI plumbing it brings with it.

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::ffi::c_void;

// ===========================================================================
// types (hand-rolled - `windows` cannot be used, see the module comment)
// ===========================================================================

pub type HRESULT = i32;
pub type UINT = u32;
pub type ULONG = u32;
pub type BOOL = i32;
pub type LUID = u64;

pub const S_OK: HRESULT = 0;
pub const E_INVALIDARG: HRESULT = 0x8007_0057u32 as HRESULT;

/// A COM interface pointer. `vtbl` is the first word of the object; every method is a slot in it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ComPtr {
    pub vtbl: *const *const usize,
}

impl ComPtr {
    pub const NULL: ComPtr = ComPtr { vtbl: core::ptr::null() };

    #[inline]
    pub fn is_null(&self) -> bool {
        self.vtbl.is_null()
    }

    /// Fetch method slot `n` as a typed function pointer.
    ///
    /// Slots 0..2 are `QueryInterface`, `AddRef`, `Release` on every COM interface, so a real
    /// method starts at slot 3. This is the same arithmetic the `windows` crate does.
    ///
    /// `transmute_copy` rather than `transmute`: `T` is a function pointer whose size is only
    /// known to the caller, so it is read out of the vtable slot with `transmute_copy`, which does
    /// not require the two types to have equal size at the call site.
    #[inline]
    pub unsafe fn method<T: Copy>(&self, n: usize) -> T {
        let slot: *const T = *self.vtbl.add(n) as *const T;
        core::mem::transmute_copy(&slot)
    }

    pub unsafe fn release(&self) {
        if self.is_null() {
            return;
        }
        let f: extern "system" fn(*mut c_void) -> ULONG = self.method(2);
        f(self.vtbl as *mut c_void);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DXGI_SAMPLE_DESC {
    pub Count: UINT,
    pub Quality: UINT,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct D3D11_TEXTURE2D_DESC {
    pub Width: UINT,
    pub Height: UINT,
    pub MipLevels: UINT,
    pub ArraySize: UINT,
    pub Format: UINT,
    pub SampleDesc: DXGI_SAMPLE_DESC,
    pub Usage: UINT,
    pub BindFlags: UINT,
    pub CPUAccessFlags: UINT,
    pub MiscFlags: UINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct D3D11_SUBRESOURCE_DATA {
    pub pSysMem: *const c_void,
    pub SysMemPitch: UINT,
    pub SysMemSlicePitch: UINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct D3D11_BUFFER_DESC {
    pub ByteWidth: UINT,
    pub Usage: UINT,
    pub BindFlags: UINT,
    pub CPUAccessFlags: UINT,
    pub MiscFlags: UINT,
    pub StructureByteStride: UINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DXGI_ADAPTER_DESC1 {
    pub Description: [u16; 128],
    pub VendorId: UINT,
    pub DeviceId: UINT,
    pub SubSysId: UINT,
    pub Revision: UINT,
    pub DedicatedVideoMemory: usize,
    pub DedicatedSystemMemory: usize,
    pub SharedSystemMemory: usize,
    pub AdapterLuid: LUID,
    pub Flags: UINT,
}

impl Default for DXGI_ADAPTER_DESC1 {
    // `[u16; 128]` is past the array length `Default` is implemented for, so it is written out.
    fn default() -> Self {
        DXGI_ADAPTER_DESC1 {
            Description: [0; 128],
            VendorId: 0,
            DeviceId: 0,
            SubSysId: 0,
            Revision: 0,
            DedicatedVideoMemory: 0,
            DedicatedSystemMemory: 0,
            SharedSystemMemory: 0,
            AdapterLuid: 0,
            Flags: 0,
        }
    }
}

// constants a renderer needs
pub const D3D_FEATURE_LEVEL_9_3: UINT = 0x9300;
pub const D3D_FEATURE_LEVEL_11_0: UINT = 0xb000;
pub const D3D_DRIVER_TYPE_UNKNOWN: UINT = 0;
pub const D3D_DRIVER_TYPE_HARDWARE: UINT = 1;
pub const D3D11_SDK_VERSION: UINT = 7;
pub const D3D11_USAGE_DEFAULT: UINT = 0;
pub const D3D11_USAGE_IMMUTABLE: UINT = 1;
pub const D3D11_BIND_VERTEX_BUFFER: UINT = 0x1;
pub const D3D11_BIND_RENDER_TARGET: UINT = 0x20;
pub const DXGI_FORMAT_R8G8B8A8_UNORM: UINT = 28;
pub const DXGI_ADAPTER_FLAG_SOFTWARE: UINT = 2;

// ===========================================================================
// the binding: WP8.1 module names only
// ===========================================================================
//
// `d3d11.dll` and `dxgi.dll` are the two libraries the phone actually ships (the C++ probe reached
// the GPU through them). Nothing here imports kernel32 / ntdll / oleaut32.

#[link(name = "d3d11.dll")]
extern "system" {
    fn D3D11CreateDevice(
        padapter: *mut c_void,
        drivertype: UINT,
        software: *mut c_void,
        flags: UINT,
        pfeaturelevels: *const UINT,
        featurelevels: UINT,
        sdkversion: UINT,
        ppdevice: *mut *mut c_void,
        pfeaturelevel: *mut UINT,
        ppimmediatecontext: *mut *mut c_void,
    ) -> HRESULT;

    fn D3D11CreateDeviceAndSwapChain(
        padapter: *mut c_void,
        drivertype: UINT,
        software: *mut c_void,
        flags: UINT,
        pfeaturelevels: *const UINT,
        featurelevels: UINT,
        sdkversion: UINT,
        pswapchaindesc: *const c_void,
        ppswapchain: *mut *mut c_void,
        ppdevice: *mut *mut c_void,
        pfeaturelevel: *mut UINT,
        ppimmediatecontext: *mut *mut c_void,
    ) -> HRESULT;
}

#[link(name = "dxgi.dll")]
extern "system" {
    fn CreateDXGIFactory1(riid: *const Guid, ppfactory: *mut *mut c_void) -> HRESULT;
}

/// `windows`'s `GUID`/`IID` equivalent, kept local so this crate has no dependency at all.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const IID_IDXGIFACTORY1: Guid = Guid {
    data1: 0x770a_a070,
    data2: 0x5f86,
    data3: 0x44c6,
    data4: [0xa8, 0x45, 0x8f, 0xc6, 0x53, 0xbc, 0x5c, 0xcb],
};

pub const IID_ID3D11DEVICE: Guid = Guid {
    data1: 0xdb6f_6d30,
    data2: 0x4eb2,
    data3: 0x45de,
    data4: [0x8e, 0x9f, 0xb4, 0xe3, 0x92, 0x8a, 0xa9, 0x9c],
};

/// `IDXGIFactory1` vtable: 0 QueryInterface, 1 AddRef, 2 Release, 3 SetPrivateData, ...,
/// 12 EnumAdapters1 (IDXGIFactory's 7 methods come first, then EnumAdapters1 is slot 12).
pub const VTBL_IDXGIFACTORY1_ENUMADAPTERS1: usize = 12;
/// `IDXGIAdapter1` inherits `IDXGIAdapter`; `GetDesc1` is slot 10 (0..2 IUnknown, 3..8
/// IDXGIObject+IDXGIAdapter, then GetDesc1).
pub const VTBL_IDXGIADAPTER1_GETDESC1: usize = 10;

/// The result of the probe: what the platform gave us.
#[repr(C)]
pub struct D3d11ProbeResult {
    pub stage: i32,
    pub hresult: i32,
    pub adapter_count: u32,
    pub feature_level: u32,
    pub adapter_desc: [u16; 128],
    pub adapter_flags: u32,
}

static mut REPORT: D3d11ProbeResult = D3d11ProbeResult {
    stage: -1,
    hresult: 0,
    adapter_count: 0,
    feature_level: 0,
    adapter_desc: [0; 128],
    adapter_flags: 0,
};

/// The whole probe: enumerate a real adapter, create a device on it, clear a render target.
///
/// Returns a stage code: 0 = success. The details are in `wp81_d3d11_rs_report`.
#[no_mangle]
pub extern "C" fn wp81_d3d11_rs_probe() -> i32 {
    unsafe {
        let r = &mut *core::ptr::addr_of_mut!(REPORT);

        // ---- 1. DXGI factory ----
        let mut factory = ComPtr::NULL;
        let hr = CreateDXGIFactory1(&IID_IDXGIFACTORY1 as *const Guid, &mut factory.vtbl as *mut _ as *mut *mut c_void);
        r.stage = 1;
        r.hresult = hr;
        if hr < 0 || factory.is_null() {
            return 1;
        }

        // ---- 2. first adapter without the SOFTWARE flag (null-adapter HARDWARE gives WARP here) ----
        let enum_adapters1: extern "system" fn(*mut c_void, UINT, *mut *mut c_void) -> HRESULT =
            factory.method(VTBL_IDXGIFACTORY1_ENUMADAPTERS1);
        let mut chosen = ComPtr::NULL;
        for i in 0..8u32 {
            let mut ad = ComPtr::NULL;
            let hr = enum_adapters1(factory.vtbl as *mut c_void, i, &mut ad.vtbl as *mut _ as *mut *mut c_void);
            if hr < 0 || ad.is_null() {
                r.adapter_count = i;
                break;
            }
            r.adapter_count = i + 1;
            let get_desc1: extern "system" fn(*mut c_void, *mut DXGI_ADAPTER_DESC1) -> HRESULT =
                ad.method(VTBL_IDXGIADAPTER1_GETDESC1);
            let mut desc = DXGI_ADAPTER_DESC1::default();
            if get_desc1(ad.vtbl as *mut c_void, &mut desc) >= 0 && (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) == 0 {
                chosen = ad;
                r.adapter_desc = desc.Description;
                r.adapter_flags = desc.Flags;
                break;
            }
            ad.release();
        }

        r.stage = 2;
        if chosen.is_null() {
            return 2;
        }

        // ---- 3. device, on the chosen adapter, UNKNOWN driver type ----
        // D3D_FEATURE_LEVEL_11_0 is passed WITHOUT 11_1: the WP8.1 runtime predates 11.1 and
        // passing it makes the call fail with E_INVALIDARG instead of falling back.
        let levels = [D3D_FEATURE_LEVEL_11_0, 0xa100u32, 0xa000, D3D_FEATURE_LEVEL_9_3];
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
        r.stage = 3;
        r.hresult = hr;
        r.feature_level = level;
        chosen.release();
        if hr < 0 || device.is_null() {
            return 3;
        }

        // ---- 4. render target + clear (slot numbers are the D3D11 vtable order) ----
        // ID3D11Device: 3 CreateBuffer, 4 CreateTexture1D, 5 CreateTexture2D, ...
        let create_texture2d: extern "system" fn(
            *mut c_void, *const D3D11_TEXTURE2D_DESC, *const D3D11_SUBRESOURCE_DATA, *mut *mut c_void,
        ) -> HRESULT = device.method(5);
        // ID3D11Device: 10 CreateRenderTargetView is slot 10 (after Create*Shader block).
        let create_rtv: extern "system" fn(*mut c_void, *mut c_void, *const c_void, *mut *mut c_void) -> HRESULT =
            device.method(10);

        let td = D3D11_TEXTURE2D_DESC {
            Width: 64,
            Height: 64,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut target = ComPtr::NULL;
        let hr = create_texture2d(device.vtbl as *mut c_void, &td, core::ptr::null(), &mut target.vtbl as *mut _ as *mut *mut c_void);
        r.stage = 4;
        r.hresult = hr;
        if hr < 0 || target.is_null() {
            return 4;
        }

        let mut rtv = ComPtr::NULL;
        let hr = create_rtv(device.vtbl as *mut c_void, target.vtbl as *mut c_void, core::ptr::null(), &mut rtv.vtbl as *mut _ as *mut *mut c_void);
        r.stage = 5;
        r.hresult = hr;
        if hr < 0 || rtv.is_null() {
            return 5;
        }

        // ID3D11DeviceContext: 50 ClearRenderTargetView (after the *Set* block; 0..2 IUnknown).
        let clear_rtv: extern "system" fn(*mut c_void, *mut c_void, *const f32) =
            context.method(50);
        clear_rtv(context.vtbl as *mut c_void, rtv.vtbl as *mut c_void, [0.25f32, 0.5, 0.75, 1.0].as_ptr());

        // ID3D11DeviceContext: 114 Flush (the last method on the 11.0 interface).
        let flush: extern "system" fn(*mut c_void) = context.method(114);
        flush(context.vtbl as *mut c_void);

        rtv.release();
        target.release();
        context.release();
        device.release();
        factory.release();

        r.stage = 0;
        0
    }
}

#[no_mangle]
pub extern "C" fn wp81_d3d11_rs_report() -> *const D3d11ProbeResult {
    core::ptr::addr_of!(REPORT)
}

// ===========================================================================
// CRT-free DLL entry point
// ===========================================================================
//
// Same treatment as the real port (`prpr-port` and `link-probe` both define these): WP8.1 native
// DLLs must not import KERNEL32.dll, and the desktop ARM CRT's startup objects do exactly that, so
// the link runs with /NODEFAULTLIB and this file supplies the entry point instead.

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
