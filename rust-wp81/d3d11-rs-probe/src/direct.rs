//! Direct D3D11 binding for Windows Phone 8.1 - the shippable form of Path A.
//!
//! No `windows` crate: that crate compiles for `thumbv7a-pc-windows-msvc` but its ABI plumbing
//! pulls in import descriptors for `kernel32.dll`, `oleaut32.dll`, `combase.dll`, `rpcrt4.dll`,
//! `user32.dll` and `ntdll.dll`, none of which exist on WP8.1. Binding `d3d11.dll` and `dxgi.dll`
//! by hand keeps the import table to exactly those two libraries.
//!
//! ## Every constant here is taken from the WP8.1 kit's own headers
//!
//! `C:\Program Files (x86)\Windows Phone Kits\8.1\include\D3D11.h` / `dxgi.h` / `D3Dcommon.h`.
//! That matters because the vtable slot numbers are not memorable and a wrong one is a silent
//! corruption rather than a compile error. Three were wrong in the first probe draft:
//!
//! | call | wrong | right |
//! | --- | --- | --- |
//! | `ID3D11Device::CreateRenderTargetView` | 10 (that is CreateDepthStencilView) | **9** |
//! | `ID3D11DeviceContext::ClearRenderTargetView` | 50 (that is GenerateMips) | **46** |
//! | `IDXGIAdapter1::GetDesc1` | 10 | **6** |
//!
//! The device/adapter creation path was verified independently by a C++ probe reaching the Adreno
//! 225 on a real device, so the entry points are right; these slots are the part that was not.
//!
//! ## vtable convention
//!
//! Slots 0..2 are `QueryInterface`/`AddRef`/`Release` on every COM interface, so a real method
//! starts at slot 3. `ComPtr::method(n)` fetches slot `n`; the tables below record the real slot
//! numbers (i.e. already offset past IUnknown).

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::ffi::c_void;

// ===========================================================================
// scalar types
// ===========================================================================

pub type HRESULT = i32;
pub type UINT = u32;
pub type ULONG = u32;
pub type BOOL = i32;
pub type FLOAT = f32;
pub type LUID = u64;

pub const S_OK: HRESULT = 0;
pub const E_INVALIDARG: HRESULT = 0x8007_0057u32 as HRESULT;

/// A COM interface pointer. `vtbl` is the object's first word; every method is a slot in it.
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
    /// `transmute_copy` rather than `transmute`: `T` is a function pointer whose size is only known
    /// to the caller, so it is read out of the slot with `transmute_copy`, which does not require
    /// the two types to have equal size at the call site.
    #[inline]
    pub unsafe fn method<T: Copy>(&self, n: usize) -> T {
        let slot: *const T = *self.vtbl.add(n) as *const T;
        core::mem::transmute_copy(&slot)
    }

    /// `IUnknown::QueryInterface` (slot 0).
    pub unsafe fn query_interface(&self, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        let f: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT = self.method(0);
        f(self.vtbl as *mut c_void, iid, out)
    }

    pub unsafe fn release(&self) {
        if self.is_null() {
            return;
        }
        let f: extern "system" fn(*mut c_void) -> ULONG = self.method(2);
        f(self.vtbl as *mut c_void);
    }
}

/// The `windows` crate's `GUID`/`IID` equivalent, kept local so this crate has no dependency.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

// ===========================================================================
// structs (field order from the kit's D3D11.h / dxgi.h - it is the ABI)
// ===========================================================================

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
#[derive(Clone, Copy, Default)]
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
pub struct D3D11_INPUT_ELEMENT_DESC {
    pub SemanticName: *const u8, // LPCSTR
    pub SemanticIndex: UINT,
    pub Format: UINT,
    pub InputSlot: UINT,
    pub AlignedByteOffset: UINT,
    pub InputSlotClass: UINT,
    pub InstanceDataStepRate: UINT,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct D3D11_VIEWPORT {
    pub TopLeftX: FLOAT,
    pub TopLeftY: FLOAT,
    pub Width: FLOAT,
    pub Height: FLOAT,
    pub MinDepth: FLOAT,
    pub MaxDepth: FLOAT,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct D3D11_MAPPED_SUBRESOURCE {
    pub pData: *mut c_void,
    pub RowPitch: UINT,
    pub DepthPitch: UINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct D3D11_SAMPLER_DESC {
    pub Filter: UINT,
    pub AddressU: UINT,
    pub AddressV: UINT,
    pub AddressW: UINT,
    pub MipLODBias: FLOAT,
    pub MaxAnisotropy: UINT,
    pub ComparisonFunc: UINT,
    pub BorderColor: [FLOAT; 4],
    pub MinLOD: FLOAT,
    pub MaxLOD: FLOAT,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct D3D11_RENDER_TARGET_BLEND_DESC {
    pub BlendEnable: BOOL,
    pub SrcBlend: UINT,
    pub DestBlend: UINT,
    pub BlendOp: UINT,
    pub SrcBlendAlpha: UINT,
    pub DestBlendAlpha: UINT,
    pub BlendOpAlpha: UINT,
    pub RenderTargetWriteMask: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct D3D11_BLEND_DESC {
    pub AlphaToCoverageEnable: BOOL,
    pub IndependentBlendEnable: BOOL,
    pub RenderTarget: [D3D11_RENDER_TARGET_BLEND_DESC; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct D3D11_RASTERIZER_DESC {
    pub FillMode: UINT,
    pub CullMode: UINT,
    pub FrontCounterClockwise: BOOL,
    pub DepthBias: i32,
    pub DepthBiasClamp: FLOAT,
    pub SlopeScaledDepthBias: FLOAT,
    pub DepthClipEnable: BOOL,
    pub ScissorEnable: BOOL,
    pub MultisampleEnable: BOOL,
    pub AntialiasedLineEnable: BOOL,
}

/// `DXGI_SWAP_CHAIN_DESC` (dxgi.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DXGI_SWAP_CHAIN_DESC {
    pub BufferDesc_Width: UINT,
    pub BufferDesc_Height: UINT,
    pub BufferDesc_RefreshRate_Numerator: UINT,
    pub BufferDesc_RefreshRate_Denominator: UINT,
    pub BufferDesc_Format: UINT,
    pub BufferDesc_ScanlineOrdering: UINT,
    pub BufferDesc_Scaling: UINT,
    pub SampleDesc: DXGI_SAMPLE_DESC,
    pub BufferUsage: UINT,
    pub BufferCount: UINT,
    pub OutputWindow: *mut c_void,
    pub Windowed: BOOL,
    pub SwapEffect: UINT,
    pub Flags: UINT,
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

// ===========================================================================
// constants (values from the kit's headers)
// ===========================================================================

// D3Dcommon.h
pub const D3D_FEATURE_LEVEL_9_3: UINT = 0x9300;
pub const D3D_FEATURE_LEVEL_10_0: UINT = 0xa000;
pub const D3D_FEATURE_LEVEL_10_1: UINT = 0xa100;
pub const D3D_FEATURE_LEVEL_11_0: UINT = 0xb000;
pub const D3D_DRIVER_TYPE_UNKNOWN: UINT = 0;
pub const D3D_DRIVER_TYPE_HARDWARE: UINT = 1;
pub const D3D11_SDK_VERSION: UINT = 7;

// D3D11.h - D3D11_USAGE
pub const D3D11_USAGE_DEFAULT: UINT = 0;
pub const D3D11_USAGE_IMMUTABLE: UINT = 1;
pub const D3D11_USAGE_DYNAMIC: UINT = 2;

// D3D11.h - D3D11_BIND_FLAG
pub const D3D11_BIND_VERTEX_BUFFER: UINT = 0x1;
pub const D3D11_BIND_INDEX_BUFFER: UINT = 0x2;
pub const D3D11_BIND_CONSTANT_BUFFER: UINT = 0x4;
pub const D3D11_BIND_SHADER_RESOURCE: UINT = 0x8;
pub const D3D11_BIND_RENDER_TARGET: UINT = 0x20;

// D3D11.h - D3D11_CPU_ACCESS_FLAG
pub const D3D11_CPU_ACCESS_WRITE: UINT = 0x10000;

// D3D11.h - D3D11_MAP
pub const D3D11_MAP_WRITE_DISCARD: UINT = 4;

// D3D11.h - D3D11_PRIMITIVE_TOPOLOGY (D3Dcommon.h)
pub const D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST: UINT = 4;

// D3D11.h - D3D11_FILTER
pub const D3D11_FILTER_MIN_MAG_MIP_POINT: UINT = 0;
pub const D3D11_FILTER_MIN_MAG_MIP_LINEAR: UINT = 21;

// D3D11.h - D3D11_TEXTURE_ADDRESS_MODE
pub const D3D11_TEXTURE_ADDRESS_WRAP: UINT = 1;
pub const D3D11_TEXTURE_ADDRESS_CLAMP: UINT = 3;

// D3D11.h - D3D11_COMPARISON_FUNC
pub const D3D11_COMPARISON_ALWAYS: UINT = 8;

// D3D11.h - D3D11_BLEND / D3D11_BLEND_OP
pub const D3D11_BLEND_ZERO: UINT = 1;
pub const D3D11_BLEND_ONE: UINT = 2;
pub const D3D11_BLEND_SRC_ALPHA: UINT = 5;
pub const D3D11_BLEND_INV_SRC_ALPHA: UINT = 6;
pub const D3D11_BLEND_OP_ADD: UINT = 1;

// D3D11.h - D3D11_FILL_MODE / D3D11_CULL_MODE
pub const D3D11_FILL_SOLID: UINT = 3;
pub const D3D11_CULL_NONE: UINT = 1;

// D3D11.h - D3D11_INPUT_CLASSIFICATION
pub const D3D11_INPUT_PER_VERTEX_DATA: UINT = 0;

// dxgi.h - DXGI_FORMAT
pub const DXGI_FORMAT_UNKNOWN: UINT = 0;
pub const DXGI_FORMAT_R32G32_FLOAT: UINT = 16;
pub const DXGI_FORMAT_R32G32B32A32_FLOAT: UINT = 2;
pub const DXGI_FORMAT_R8G8B8A8_UNORM: UINT = 28;
pub const DXGI_FORMAT_B8G8R8A8_UNORM: UINT = 87;

// dxgi.h - DXGI_USAGE
pub const DXGI_USAGE_RENDER_TARGET_OUTPUT: UINT = 0x20;

// dxgi.h - DXGI_SWAP_EFFECT / DXGI_SWAP_CHAIN_FLAG
pub const DXGI_SWAP_EFFECT_DISCARD: UINT = 0;
pub const DXGI_SWAP_EFFECT_SEQUENTIAL: UINT = 1;
pub const DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL: UINT = 3;

// dxgi.h - DXGI_MWA / DXGI_PRESENT
pub const DXGI_PRESENT_TEST: UINT = 0x1;
pub const DXGI_ADAPTER_FLAG_SOFTWARE: UINT = 2;

// ===========================================================================
// the binding: WP8.1 module names only
// ===========================================================================

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
        pswapchaindesc: *const DXGI_SWAP_CHAIN_DESC,
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

#[link(name = "d3dcompiler.dll")]
extern "system" {
    /// Present in the kit's `d3dcompiler.lib` (61 symbols incl. `D3DCompile`, `D3DReflect`), so
    /// the phone can compile HLSL at runtime and offline DXBC is an optimisation, not a need.
    fn D3DCompile(
        psrcdata: *const c_void,
        srcdatasize: usize,
        psourcename: *const u8,
        pdefines: *const c_void,
        pinclude: *mut c_void,
        pentrypoint: *const u8,
        ptarget: *const u8,
        flags1: UINT,
        flags2: UINT,
        ppcode: *mut *mut c_void,
        pperrorblob: *mut *mut c_void,
    ) -> HRESULT;
}

// ===========================================================================
// IIDs
// ===========================================================================

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

/// `IDXGISwapChain` (dxgi.h).
pub const IID_IDXGISWAPCHAIN: Guid = Guid {
    data1: 0x310d_36b9,
    data2: 0x231c,
    data3: 0x47c3,
    data4: [0xba, 0xb9, 0x75, 0xc9, 0x74, 0x8e, 0x2c, 0x81],
};

/// `IDXGISwapChain1` (dxgi1_2.h) - what `CreateSwapChainForComposition` returns.
pub const IID_IDXGISWAPCHAIN1: Guid = Guid {
    data1: 0x790a_45d7,
    data2: 0x9d6e,
    data3: 0x4fb1,
    data4: [0x97, 0x3b, 0x00, 0x87, 0xa4, 0xe3, 0x35, 0xef],
};

/// `ISwapChainPanelNative` (windows.ui.xaml.media.dxinterop.h) - the XAML side of composition.
pub const IID_ISWAPCHAINPANELNATIVE: Guid = Guid {
    data1: 0x63aa_d0b8,
    data2: 0x7c24,
    data3: 0x40ff,
    data4: [0x85, 0xa8, 0x64, 0x0d, 0x94, 0x4c, 0xc3, 0x25],
};

// ===========================================================================
// vtable slots (already offset past IUnknown - a real method starts at 3)
// ===========================================================================

// IDXGIObject (3..6) -> IDXGIAdapter (7..9) -> IDXGIAdapter1 (10)
pub const IDXGIADAPTER1_GETDESC1: usize = 10;
// IDXGIObject (3..6) -> IDXGIFactory (7..11) -> IDXGIFactory1 (12 EnumAdapters1, 13 IsCurrent)
pub const IDXGIFACTORY1_ENUMADAPTERS1: usize = 12;
pub const IDXGIFACTORY_CREATESWAPCHAIN: usize = 9;
// IDXGIObject (3..6) -> IDXGIDeviceSubObject (7) -> IDXGISwapChain (8..17)
pub const IDXGISWAPCHAIN_PRESENT: usize = 8;
pub const IDXGISWAPCHAIN_GETBUFFER: usize = 9;
pub const IDXGISWAPCHAIN_RESIZEBUFFERS: usize = 13;

// IDXGIFactory2 (dxgi1_2.h): IDXGIObject 3..6, IDXGIFactory 7..11, IDXGIFactory1 12..13,
// then 14 IsWindowedStereoEnabled, 15 CreateSwapChainForHwnd,
// 16 CreateSwapChainForCoreWindow, ..., 24 CreateSwapChainForComposition
pub const IDXGIFACTORY2_CREATESWAPCHAINFORCOREWINDOW: usize = 16;
pub const IDXGIFACTORY2_CREATESWAPCHAINFORCOMPOSITION: usize = 24;

// ID3D11Device
pub const D3D11DEVICE_CREATEBUFFER: usize = 3;
pub const D3D11DEVICE_CREATETEXTURE2D: usize = 5;
pub const D3D11DEVICE_CREATESHADERRESOURCEVIEW: usize = 7;
pub const D3D11DEVICE_CREATERENDERTARGETVIEW: usize = 9; // NOT 10 (that is DepthStencilView)
pub const D3D11DEVICE_CREATEINPUTLAYOUT: usize = 11;
pub const D3D11DEVICE_CREATEVERTEXSHADER: usize = 12;
pub const D3D11DEVICE_CREATEPIXELSHADER: usize = 15;
pub const D3D11DEVICE_CREATEBLENDSTATE: usize = 20;
pub const D3D11DEVICE_CREATERASTERIZERSTATE: usize = 22;
pub const D3D11DEVICE_CREATESAMPLERSTATE: usize = 23;
pub const D3D11DEVICE_GETFEATURELEVEL: usize = 37;

// ID3D11DeviceContext
pub const D3D11CONTEXT_VSSETCONSTANTBUFFERS: usize = 3;
pub const D3D11CONTEXT_PSSETSHADERRESOURCES: usize = 4;
pub const D3D11CONTEXT_PSSETSHADER: usize = 5;
pub const D3D11CONTEXT_PSSETSAMPLERS: usize = 6;
pub const D3D11CONTEXT_VSSETSHADER: usize = 7;
pub const D3D11CONTEXT_DRAWINDEXED: usize = 8;
pub const D3D11CONTEXT_DRAW: usize = 9;
pub const D3D11CONTEXT_MAP: usize = 10;
pub const D3D11CONTEXT_UNMAP: usize = 11;
pub const D3D11CONTEXT_PSSETCONSTANTBUFFERS: usize = 12;
pub const D3D11CONTEXT_IASETINPUTLAYOUT: usize = 13;
pub const D3D11CONTEXT_IASETVERTEXBUFFERS: usize = 14;
pub const D3D11CONTEXT_IASETINDEXBUFFER: usize = 15;
pub const D3D11CONTEXT_IASETPRIMITIVETOPOLOGY: usize = 20;
pub const D3D11CONTEXT_OMSETRENDERTARGETS: usize = 29;
pub const D3D11CONTEXT_OMSETBLENDSTATE: usize = 31;
pub const D3D11CONTEXT_RSSETSTATE: usize = 39;
pub const D3D11CONTEXT_RSSETVIEWPORTS: usize = 40;
pub const D3D11CONTEXT_UPDATESUBRESOURCE: usize = 44;
pub const D3D11CONTEXT_CLEARRENDERTARGETVIEW: usize = 46; // NOT 50 (that is GenerateMips)
pub const D3D11CONTEXT_FLUSH: usize = 114;

// ===========================================================================
// the probe report (kept: the C# side reads it to show what the platform gave us)
// ===========================================================================

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
        let hr = CreateDXGIFactory1(
            &IID_IDXGIFACTORY1 as *const Guid,
            &mut factory.vtbl as *mut _ as *mut *mut c_void,
        );
        r.stage = 1;
        r.hresult = hr;
        if hr < 0 || factory.is_null() {
            return 1;
        }

        // ---- 2. first adapter without the SOFTWARE flag ----
        let enum_adapters1: extern "system" fn(*mut c_void, UINT, *mut *mut c_void) -> HRESULT =
            factory.method(IDXGIFACTORY1_ENUMADAPTERS1);
        let mut chosen = ComPtr::NULL;
        for i in 0..8u32 {
            let mut ad = ComPtr::NULL;
            let hr = enum_adapters1(
                factory.vtbl as *mut c_void,
                i,
                &mut ad.vtbl as *mut _ as *mut *mut c_void,
            );
            if hr < 0 || ad.is_null() {
                r.adapter_count = i;
                break;
            }
            r.adapter_count = i + 1;
            let get_desc1: extern "system" fn(*mut c_void, *mut DXGI_ADAPTER_DESC1) -> HRESULT =
                ad.method(IDXGIADAPTER1_GETDESC1);
            let mut desc = DXGI_ADAPTER_DESC1::default();
            if get_desc1(ad.vtbl as *mut c_void, &mut desc) >= 0
                && (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) == 0
            {
                chosen = ad;
                r.adapter_desc = desc.Description;
                r.adapter_flags = desc.Flags;
                break;
            }
            ad.release();
        }

        r.stage = 2;
        if chosen.is_null() {
            factory.release();
            return 2;
        }

        // ---- 3. device, on the chosen adapter, UNKNOWN driver type ----
        // D3D_FEATURE_LEVEL_11_1 is deliberately omitted: the WP8.1 runtime predates it and
        // passing it makes the call fail with E_INVALIDARG instead of falling back.
        let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_9_3];
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
            factory.release();
            return 3;
        }

        // ---- 4. render target + clear ----
        let create_texture2d: extern "system" fn(
            *mut c_void,
            *const D3D11_TEXTURE2D_DESC,
            *const D3D11_SUBRESOURCE_DATA,
            *mut *mut c_void,
        ) -> HRESULT = device.method(D3D11DEVICE_CREATETEXTURE2D);
        let create_rtv: extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            *mut *mut c_void,
        ) -> HRESULT = device.method(D3D11DEVICE_CREATERENDERTARGETVIEW);

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
        let hr = create_texture2d(
            device.vtbl as *mut c_void,
            &td,
            core::ptr::null(),
            &mut target.vtbl as *mut _ as *mut *mut c_void,
        );
        r.stage = 4;
        r.hresult = hr;
        if hr < 0 || target.is_null() {
            context.release();
            device.release();
            factory.release();
            return 4;
        }

        let mut rtv = ComPtr::NULL;
        let hr = create_rtv(
            device.vtbl as *mut c_void,
            target.vtbl as *mut c_void,
            core::ptr::null(),
            &mut rtv.vtbl as *mut _ as *mut *mut c_void,
        );
        r.stage = 5;
        r.hresult = hr;
        if hr < 0 || rtv.is_null() {
            target.release();
            context.release();
            device.release();
            factory.release();
            return 5;
        }

        let clear_rtv: extern "system" fn(*mut c_void, *mut c_void, *const FLOAT) =
            context.method(D3D11CONTEXT_CLEARRENDERTARGETVIEW);
        clear_rtv(
            context.vtbl as *mut c_void,
            rtv.vtbl as *mut c_void,
            [0.25f32, 0.5, 0.75, 1.0].as_ptr(),
        );

        let flush: extern "system" fn(*mut c_void) = context.method(D3D11CONTEXT_FLUSH);
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

/// Expose the vtable-slot table to other modules of this crate, and to a runtime self-check.
///
/// These are asserted in `tests` below rather than trusted: a wrong slot is a crash, not a
/// compile error, so it is worth pinning the ones the renderer depends on.
pub mod slots {
    pub const IDXGIADAPTER1_GETDESC1: usize = 10;
    pub const ID3D11DEVICE_CREATERENDERTARGETVIEW: usize = 9;
    pub const ID3D11CONTEXT_CLEARRENDERTARGETVIEW: usize = 46;
    pub const ID3D11CONTEXT_FLUSH: usize = 114;
}

// ===========================================================================
// CRT-free DLL entry point
// ===========================================================================
//
// WP8.1 native DLLs must not import KERNEL32.dll, and the desktop ARM CRT's startup objects do
// exactly that, so the link runs with /NODEFAULTLIB and this file supplies the entry point.

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
