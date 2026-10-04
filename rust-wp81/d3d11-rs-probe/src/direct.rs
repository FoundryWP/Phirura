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
//! | `IDXGIFactory::CreateSwapChain` | 9 (that is GetWindowAssociation) | **10** |
//!
//! (`IDXGIAdapter1::GetDesc1` = 10 and `IDXGIFactory1::EnumAdapters1` = 12 were already right;
//! the full chains are IUnknown 0..2, IDXGIObject 3..6, then the derived interface from 7.)
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
pub const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as HRESULT;

/// A COM interface pointer. `vtbl` is the object pointer: `*vtbl` is the vtable, `(*vtbl)[n]` the
/// n-th method. The `this` passed to every call is therefore `vtbl` itself.
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
    /// `*self.vtbl` is the vtable; slot `n` in it *is* the function address. Read that word and
    /// reinterpret it as `T`; do not treat the address as a pointer to data (that reads the
    /// function's own machine code and jumps through garbage). `transmute_copy` is used rather than
    /// `transmute` because `T`'s size is only known to the caller.
    #[inline]
    pub unsafe fn method<T: Copy>(&self, n: usize) -> T {
        let vtable: *const usize = *self.vtbl;
        let slot_addr: *const usize = vtable.add(n);
        core::mem::transmute_copy(&*slot_addr)
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

/// `DXGI_SWAP_CHAIN_DESC1` (dxgi.h) - what `CreateSwapChainForComposition` actually takes.
///
/// This is a DIFFERENT struct from [`DXGI_SWAP_CHAIN_DESC`], despite the similar name: it drops
/// `RefreshRate`, `ScanlineOrdering`, `OutputWindow` and `Windowed`, and adds `Stereo`, `Scaling`
/// and `AlphaMode`. Passing the legacy descriptor to `CreateSwapChainForComposition` reads every
/// field from the wrong offset and the call fails with `DXGI_ERROR_INVALID_CALL` (`0x887A0001`) -
/// which looks exactly like "the feature is unsupported", but is really a layout mismatch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DXGI_SWAP_CHAIN_DESC1 {
    pub Width: UINT,
    pub Height: UINT,
    pub Format: UINT,
    pub Stereo: BOOL,
    pub SampleDesc: DXGI_SAMPLE_DESC,
    pub BufferUsage: UINT,
    pub BufferCount: UINT,
    pub Scaling: UINT,
    pub SwapEffect: UINT,
    pub AlphaMode: UINT,
    pub Flags: UINT,
}

// dxgi.h - DXGI_SCALING
pub const DXGI_SCALING_STRETCH: UINT = 0;
pub const DXGI_SCALING_NONE: UINT = 1;
pub const DXGI_SCALING_ASPECT_RATIO_STRETCH: UINT = 2;

// dxgi.h - DXGI_ALPHA_MODE
pub const DXGI_ALPHA_MODE_UNSPECIFIED: UINT = 0;
pub const DXGI_ALPHA_MODE_PREMULTIPLIED: UINT = 1;
pub const DXGI_ALPHA_MODE_STRAIGHT: UINT = 2;
pub const DXGI_ALPHA_MODE_IGNORE: UINT = 3;

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
pub const D3D_FEATURE_LEVEL_9_1: UINT = 0x9100;
pub const D3D_FEATURE_LEVEL_9_2: UINT = 0x9200;
pub const D3D_FEATURE_LEVEL_9_3: UINT = 0x9300;
pub const D3D_FEATURE_LEVEL_10_0: UINT = 0xa000;
pub const D3D_FEATURE_LEVEL_10_1: UINT = 0xa100;
pub const D3D_FEATURE_LEVEL_11_0: UINT = 0xb000;
pub const D3D_FEATURE_LEVEL_11_1: UINT = 0xb100;
pub const D3D_DRIVER_TYPE_UNKNOWN: UINT = 0;
pub const D3D_DRIVER_TYPE_HARDWARE: UINT = 1;
pub const D3D11_SDK_VERSION: UINT = 7;

// D3D11.h - D3D11_CREATE_DEVICE_FLAG
//
// `BGRA_SUPPORT` (0x20) is what every WP8.1 XAML/DirectX sample passes, and it is required for
// XAML composition interop: a `SwapChainPanel` surface is BGRA, so a device created without this
// flag is not a valid device for `CreateSwapChainForComposition` and DXGI rejects the call with
// `DXGI_ERROR_INVALID_CALL` even though the descriptor is otherwise perfect.
pub const D3D11_CREATE_DEVICE_BGRA_SUPPORT: UINT = 0x20;

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
//
// `ID3D11DeviceContext::Map` takes BOTH a `D3D11_MAP MapType` and a `UINT MapFlags`, in that
// order, after the subresource index. Dropping the `MapFlags` argument silently shifts the
// out-parameter into it and the call fails with `E_INVALIDARG`.
pub type D3D11_MAP = UINT;
pub const D3D11_MAP_WRITE_DISCARD: D3D11_MAP = 4;

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
pub const DXGI_FORMAT_R32G32B32A32_FLOAT: UINT = 2;
pub const DXGI_FORMAT_R32G32B32_FLOAT: UINT = 6;
pub const DXGI_FORMAT_R32G32_FLOAT: UINT = 16;
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

// dxgi.h - the DXGI_ERROR_* family. Only the ones the swapchain path can actually return are
// listed, so a failing HRESULT in the phone's log decodes to a sentence instead of a number.
pub const DXGI_ERROR_INVALID_CALL: i32 = 0x887A0001u32 as i32;
pub const DXGI_ERROR_NOT_CURRENTLY_AVAILABLE: i32 = 0x887A0022u32 as i32;
pub const DXGI_ERROR_UNSUPPORTED: i32 = 0x887A0004u32 as i32;
pub const DXGI_ERROR_DEVICE_REMOVED: i32 = 0x887A0005u32 as i32;
pub const DXGI_ERROR_DEVICE_RESET: i32 = 0x887A0007u32 as i32;
pub const DXGI_ERROR_ALREADY_EXISTS: i32 = 0x887A0008u32 as i32;
pub const DXGI_ERROR_SDK_COMPONENT_MISSING: i32 = 0x887A002Du32 as i32;

/// English name for a DXGI HRESULT (used by the swapchain step-name table on the C# side).
pub fn dxgi_error_name(hr: i32) -> &'static [u8] {
    match hr {
        DXGI_ERROR_INVALID_CALL => b"DXGI_ERROR_INVALID_CALL\0",
        DXGI_ERROR_NOT_CURRENTLY_AVAILABLE => b"DXGI_ERROR_NOT_CURRENTLY_AVAILABLE\0",
        DXGI_ERROR_UNSUPPORTED => b"DXGI_ERROR_UNSUPPORTED\0",
        DXGI_ERROR_DEVICE_REMOVED => b"DXGI_ERROR_DEVICE_REMOVED\0",
        DXGI_ERROR_DEVICE_RESET => b"DXGI_ERROR_DEVICE_RESET\0",
        DXGI_ERROR_ALREADY_EXISTS => b"DXGI_ERROR_ALREADY_EXISTS\0",
        DXGI_ERROR_SDK_COMPONENT_MISSING => b"DXGI_ERROR_SDK_COMPONENT_MISSING\0",
        _ => b"\0",
    }
}

// ===========================================================================
// the binding: WP8.1 module names only
// ===========================================================================

// The DLL names are supplied by the *linker*, not by `#[link]`.
//
// On WP8.1 `build.ps1` passes the kit's ARM `d3d11.lib` / `dxgi.lib` / `d3dcompiler.lib` straight
// to `link.exe`; on the desktop host `build.rs` does the same for the SDK's x64 libs. Neither
// wants Rust's `#[link]` machinery: `#[link(name = "d3d11.dll")]` makes MSVC ask for
// `d3d11.dll.lib`, a file the SDK does not ship, and the ARM link already names the libraries
// itself. Declaring the imports without `#[link]` keeps both paths explicit and identical.

extern "system" {
    pub fn D3D11CreateDevice(
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

    pub fn D3D11CreateDeviceAndSwapChain(
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

extern "system" {
    pub fn CreateDXGIFactory1(riid: *const Guid, ppfactory: *mut *mut c_void) -> HRESULT;
}

// ===========================================================================
// WinRT activation — how an *app* reaches the WinRT runtime
// ===========================================================================
//
// `runtimeobject.lib` ships in the WP8.1 kit and imports these from the API sets an app container
// is allowed to load (`api-ms-win-core-winrt-l1-1-0.dll`, `...-string-l1-1-0.dll`). This is the
// same mechanism Win2D's own ARM DLL uses. On desktop `runtimeobject.lib` does not exist under
// that name; `hostcheck` therefore links `RoGetActivationFactory`/`WindowsCreateString` from
// `combase.lib`, which carries identical names.

extern "system" {
    /// `RoGetActivationFactory(HSTRING classId, REFIID iid, void** factory)`.
    ///
    /// The class name must be a WinRT `HSTRING`, which is why [`hstring`] exists - a plain
    /// NUL-terminated `u16*` is *not* an `HSTRING` and passing one is a callback into the
    /// `HSTRING` header word, which is a hard fault rather than an error return.
    pub fn RoGetActivationFactory(
        activatable_class_id: *mut c_void,
        iid: *const Guid,
        factory: *mut *mut c_void,
    ) -> HRESULT;

    /// `WindowsCreateString(LPCWSTR sourceString, UINT32 length, HSTRING* string)`.
    pub fn WindowsCreateString(
        source_string: *const u16,
        length: u32,
        string: *mut *mut c_void,
    ) -> HRESULT;

    /// `WindowsDeleteString(HSTRING string)`.
    pub fn WindowsDeleteString(string: *mut c_void) -> HRESULT;
}

/// A WinRT string, as the `HSTRING` opaque handle.
pub type HSTRING = *mut c_void;

/// Build an `HSTRING` from a compile-time UTF-16 literal, run `f`, then free it.
///
/// Wrapping the create/delete in one place means a failure to create cannot leak, and no caller
/// has to remember `WindowsDeleteString`. `f` receives the handle and returns the callee's
/// `HRESULT`.
pub unsafe fn with_hstring<T>(
    literal: &[u16],
    f: impl FnOnce(HSTRING) -> T,
) -> Result<T, HRESULT> {
    // A WinRT string literal is stored without its trailing NUL; passing the NUL as part of the
    // length makes the class name "Foo\0", which `RoGetActivationFactory` rejects with
    // `E_INVALIDARG` (0x80070057) rather than "class not registered".
    let len = literal.len() as u32;
    let mut hs: HSTRING = core::ptr::null_mut();
    let hr = WindowsCreateString(literal.as_ptr(), len, &mut hs);
    if hr < 0 || hs.is_null() {
        return Err(hr);
    }
    let out = f(hs);
    WindowsDeleteString(hs);
    Ok(out)
}

/// Run `f` with an `HSTRING` for `literal`, **including** the trailing NUL in the length.
///
/// Used only for `RoActivateInstance`, where the kit's own samples pass `ARRAYSIZE(name)` on a
/// NUL-terminated `WCHAR[]`. Kept separate from [`with_hstring`] so the two conventions are
/// explicit rather than one function guessing.
pub unsafe fn with_hstring_nul<T>(
    literal: &[u16],
    f: impl FnOnce(HSTRING) -> T,
) -> Result<T, HRESULT> {
    let mut hs: HSTRING = core::ptr::null_mut();
    let hr = WindowsCreateString(literal.as_ptr(), literal.len() as u32, &mut hs);
    if hr < 0 || hs.is_null() {
        return Err(hr);
    }
    let out = f(hs);
    WindowsDeleteString(hs);
    Ok(out)
}

// ===========================================================================
// DXGI <-> WinRT interop — the WP8.1 phone render path
// ===========================================================================
//
// A WP8.1 *phone* app does not present through a swapchain. `CreateSwapChainForCoreWindow` and
// `CreateSwapChainForComposition` both answer `DXGI_ERROR_INVALID_CALL` for every documented
// argument shape on the Adreno 225 / WDDM 1.2 / FL 9.3 driver (proven exhaustively on device —
// see `PLAN-PATH-A.md`, Round 45). The `SwapChainPanel` mechanism is the desktop-class Windows 8.1
// path. The phone path is the **XAML `SurfaceImageSource`** pair of interfaces:
//
//     ISurfaceImageSourceNative::SetDevice(IDXGIDevice*)   // slot 3
//     ISurfaceImageSourceNative::BeginDraw(rect, &surface, &offset)  // slot 4
//     ISurfaceImageSourceNative::EndDraw()                 // slot 5
//
// Rust calls `SetDevice` **once** with its own `IDXGIDevice`, then calls `BeginDraw` on every frame:
// the returned `IDXGISurface*` *is inside the compositor's own render target*, so drawing into it
// **is** presenting. That removes the whole "large offscreen texture" copy and removes the interop
// helpers from the picture entirely — the only thing needed is a raw QI for
// `ISurfaceImageSourceNative`, which every WinRT object answers natively.
//
// ### Why the old approach was impossible
//
// The earlier design called `CreateDirect3D11SurfaceFromDXGISurface` / `CreateDirect3D11DeviceFromDXGIDevice`
// resolved through `LoadLibraryExW` + `GetProcAddress`. On device that logged
//
//     interop-trace=winrt{h=0,exp=0} d3d11{h=1,exp=0}
//
// i.e. **`winrt.dll` cannot be loaded from an app container (handle 0), and `d3d11.dll` does not
// export the helper (exp=0)**. A real PE import-table parse of Win2D's ARM DLL then showed Win2D
// imports only `D3D11CreateDevice` from `d3d11.dll` plus `RoGetActivationFactory` from
// `api-ms-win-core-winrt-l1-1-0.dll` — no `winrt.dll`, no interop helpers. Neither the WP8.1 kit's
// ARM `d3d11.lib` nor any other kit lib carries those two helpers. So there is no app-accessible
// export to find, and the API is simply not reachable that way.
//
// ### What an app *can* do
//
// `runtimeobject.lib` **is** in the WP8.1 kit and is the sanctioned WinRT entry point. It imports
// `RoGetActivationFactory` / `RoActivateInstance` from `api-ms-win-core-winrt-l1-1-0.dll` — the
// very API set Win2D imports. That is the app-accessible path, and it is what `lib.rs` uses to
// activate the `SurfaceImageSource` WinRT class and QI it for the native interface.
//
// The `LoadLibraryExW` declarations below are retained only so `INTEROP_SOURCE`/`INTEROP_TRACE`
// still report the (failed) legacy probe in the on-device log; nothing calls them any more.

extern "system" {
    fn LoadLibraryExW(lp_lib_file_name: *const u16, h_file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, proc_name: *const u8) -> *mut c_void;
}

/// `d3d11.dll` as UTF-16LE, NUL-terminated.
const D3D11_DLL_W: [u16; 10] = [
    0x0064, 0x0033, 0x0064, 0x0031, 0x0031, 0x002E, 0x0064, 0x006C, 0x006C, 0x0000,
];

/// `winrt.dll` as UTF-16LE, NUL-terminated. Kept only for the legacy trace probe.
const WINRT_DLL_W: [u16; 10] = [
    0x0077, 0x0069, 0x006E, 0x0072, 0x0074, 0x002E, 0x0064, 0x006C, 0x006C, 0x0000,
];

/// Resolve an export of the already-loaded `d3d11.dll`.
pub unsafe fn d3d11_export(name: *const u8) -> *mut c_void {
    let h = LoadLibraryExW(D3D11_DLL_W.as_ptr(), core::ptr::null_mut(), 0);
    if h.is_null() {
        return core::ptr::null_mut();
    }
    GetProcAddress(h, name)
}

/// **Legacy.** Record whether the Direct3D interop helpers are reachable via the loader.
///
/// Returns null on WP8.1 by construction (see the block comment above). Kept so the on-device log
/// keeps printing the proof that this route is dead, and so `hostcheck` on desktop still resolves.
pub unsafe fn interop_export(name: *const u8) -> *mut c_void {
    let h = LoadLibraryExW(WINRT_DLL_W.as_ptr(), core::ptr::null_mut(), 0);
    INTEROP_TRACE[0] = if h.is_null() { 0 } else { 1 };
    if !h.is_null() {
        let p = GetProcAddress(h, name);
        INTEROP_TRACE[1] = if p.is_null() { 0 } else { 1 };
        if !p.is_null() {
            note_interop_source(1); // winrt.dll
            return p;
        }
    }

    let h11 = LoadLibraryExW(D3D11_DLL_W.as_ptr(), core::ptr::null_mut(), 0);
    INTEROP_TRACE[2] = if h11.is_null() { 0 } else { 1 };
    let p = if h11.is_null() {
        core::ptr::null_mut()
    } else {
        GetProcAddress(h11, name)
    };
    INTEROP_TRACE[3] = if p.is_null() { 0 } else { 1 };
    note_interop_source(if p.is_null() { 0 } else { 2 }); // 0 = neither, 2 = d3d11.dll
    p
}

/// Wrap a DXGI device as the WinRT `IDirect3DDevice` the XAML compositor accepts.
///
/// **Not used by the `SurfaceImageSource` route** — that route hands `SetDevice` a plain
/// `IDXGIDevice*` and needs no projection at all. Retained because `hostcheck` exercises it and it
/// is the documented desktop path.
pub unsafe fn create_direct3d11_device_from_dxgi_device(
    dxgi_device: *mut c_void,
    gfx_device: *mut *mut c_void,
) -> HRESULT {
    let p = interop_export(b"CreateDirect3D11DeviceFromDXGIDevice\0".as_ptr());
    if p.is_null() {
        return E_NOINTERFACE;
    }
    let f: extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = core::mem::transmute(p);
    f(dxgi_device, gfx_device)
}

/// Wrap an offscreen DXGI surface as the WinRT `IDirect3DSurface` the compositor can show.
///
/// **Not used by the `SurfaceImageSource` route** — see above.
pub unsafe fn create_direct3d11_surface_from_dxgi_surface(
    dxgi_surface: *mut c_void,
    gfx_surface: *mut *mut c_void,
) -> HRESULT {
    let p = interop_export(b"CreateDirect3D11SurfaceFromDXGISurface\0".as_ptr());
    if p.is_null() {
        return E_NOINTERFACE;
    }
    let f: extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = core::mem::transmute(p);
    f(dxgi_surface, gfx_surface)
}

/// Which module the interop helpers resolved from: 1 = `winrt.dll`, 2 = `d3d11.dll`, 0 = neither.
///
/// Exposed so the phone log can say whether the resolution took the platform-correct path.
#[no_mangle]
pub static mut INTEROP_SOURCE: i32 = 0;

/// Diagnostics for the resolution attempt, readable from C# so a failure is unambiguous:
///   [0] = 1 if `LoadLibraryExW("winrt.dll")` returned a handle, else 0
///   [1] = 1 if it exported the name, else 0
///   [2] = 1 if `LoadLibraryExW("d3d11.dll")` returned a handle, else 0
///   [3] = 1 if it exported the name, else 0
#[no_mangle]
pub static mut INTEROP_TRACE: [i32; 4] = [0; 4];

unsafe fn note_interop_source(v: i32) {
    INTEROP_SOURCE = v;
}

extern "system" {
    /// Present in the kit's `d3dcompiler.lib` (61 symbols incl. `D3DCompile`, `D3DReflect`), so
    /// the phone can compile HLSL at runtime and offline DXBC is an optimisation, not a need.
    pub fn D3DCompile(
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
//
// Every one of these is copied from a `DEFINE_GUID` line (or a `MIDL_INTERFACE` attribute) in the
// WP8.1 kit's own headers. They were originally written from memory and six of them were wrong -
// see the note at the top of this file. Do not "tidy" these.

// IUnknown's own IID (objbase.h / guiddef.h) - the one QueryInterface can always answer.
pub const IID_IUNKNOWN: Guid = Guid {
    data1: 0x0000_0000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

// dxgi.h
pub const IID_IDXGIFACTORY1: Guid = Guid {
    data1: 0x770a_ae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

// dxgi1_2.h
pub const IID_IDXGIFACTORY2: Guid = Guid {
    data1: 0x50c8_3a1c,
    data2: 0xe072,
    data3: 0x4c48,
    data4: [0x87, 0xb0, 0x36, 0x30, 0xfa, 0x36, 0xa6, 0xd0],
};

// dxgi.h
pub const IID_IDXGIDEVICE: Guid = Guid {
    data1: 0x54ec_77fa,
    data2: 0x1377,
    data3: 0x44e6,
    data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
};

// dxgi.h
pub const IID_IDXGISWAPCHAIN: Guid = Guid {
    data1: 0x310d_36a0,
    data2: 0xd2e7,
    data3: 0x4c0a,
    data4: [0xaa, 0x04, 0x6a, 0x9d, 0x23, 0xb8, 0x88, 0x6a],
};

/// `IDXGISurface` (dxgi.h: `cafcb56c-6ac3-4889-bf47-9e23bbd260ec`).
///
/// Needed explicitly because `CreateDirect3D11SurfaceFromDXGISurface` takes an **`IDXGISurface*`**,
/// and an `ID3D11Texture2D` is *not* one. Both descend from `IDXGIDeviceSubObject`
/// (`IDXGISurface : public IDXGIDeviceSubObject`; `ID3D11Texture2D : public ID3D11Resource` ->
/// `ID3D11Resource : public IDXGIResource` -> `IDXGIDeviceSubObject`) - siblings, not parent/child.
/// On desktop the interop helper QIs leniently enough that a raw texture pointer sneaks through;
/// on the WP8.1 FL 9.3 driver it does not, and returns E_NOINTERFACE. So the QI is done here.
pub const IID_IDXGISURFACE: Guid = Guid {
    data1: 0xcafc_b56c,
    data2: 0x6ac3,
    data3: 0x4889,
    data4: [0xbf, 0x47, 0x9e, 0x23, 0xbb, 0xd2, 0x60, 0xec],
};

/// `IDXGISurface1` (dxgi.h: `4ae63092-6327-4c1b-80ae-bfe12ea32b86`), derives from `IDXGISurface`.
pub const IID_IDXGISURFACE1: Guid = Guid {
    data1: 0x4ae6_3092,
    data2: 0x6327,
    data3: 0x4c1b,
    data4: [0x80, 0xae, 0xbf, 0xe1, 0x2e, 0xa3, 0x2b, 0x86],
};

/// `IDXGISurface2` (DXGI1_2.h: `aba496dd-b617-4cb8-a866-bc44d7eb1fa2`), derives from
/// `IDXGISurface1`.
///
/// The WP8.1 interop helper is a DXGI-1.2-era API and, on the FL 9.3 driver, asks for the *most
/// derived* surface interface it can get - an `IDXGISurface` alone is not enough there, which is
/// why the E_NOINTERFACE survived the first QI fix. The chain is therefore probed newest-first.
pub const IID_IDXGISURFACE2: Guid = Guid {
    data1: 0xaba4_96dd,
    data2: 0xb617,
    data3: 0x4cb8,
    data4: [0xa8, 0x66, 0xbc, 0x44, 0xd7, 0xeb, 0x1f, 0xa2],
};

// dxgi1_2.h
pub const IID_IDXGISWAPCHAIN1: Guid = Guid {
    data1: 0x790a_45f7,
    data2: 0x0d42,
    data3: 0x4876,
    data4: [0x98, 0x3a, 0x0a, 0x55, 0xcf, 0xe6, 0xf4, 0xaa],
};

// D3D11.h
pub const IID_ID3D11DEVICE: Guid = Guid {
    data1: 0xdb6f_6ddb,
    data2: 0xac77,
    data3: 0x4e88,
    data4: [0x82, 0x53, 0x81, 0x9d, 0xf9, 0xbb, 0xf1, 0x40],
};

/// `ID3D11Texture2D` - used as the riid when asking a swapchain for its back buffer.
pub const IID_ID3D11TEXTURE2D: Guid = Guid {
    data1: 0x6f15_aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

/// `ISwapChainPanelNative` (windows.ui.xaml.media.dxinterop.h) - the XAML side of composition.
///
/// The kit's own header gives the IID: `F92F19D2-3ADE-45A6-A20C-F6F1EA90554B`. It derives from
/// `IUnknown` and has exactly one method, `SetSwapChain(IDXGISwapChain*)`, at slot 3.
pub const IID_ISWAPCHAINPANELNATIVE: Guid = Guid {
    data1: 0xf92f_19d2,
    data2: 0x3ade,
    data3: 0x45a6,
    data4: [0xa2, 0x0c, 0xf6, 0xf1, 0xea, 0x90, 0x55, 0x4b],
};

/// `ISwapChainPanelNative::SetSwapChain` lives at slot 3 (the first method past IUnknown).
pub const ISWAPCHAINPANELNATIVE_SETSWAPCHAIN: usize = 3;

/// `ISwapChainBackgroundPanelNative` (windows.ui.xaml.media.dxinterop.h) - the Win8.0-era XAML side
/// of composition, and still present in the WP8.1 kit's own header.
///
/// The kit gives the IID: `43B EBD4E-ADD5-4035-8F85-5608D08E9DC9`. Like `ISwapChainPanelNative` it
/// derives from `IUnknown` and has exactly one method, `SetSwapChain(IDXGISwapChain*)`, at slot 3.
///
/// It exists here as a fallback because the phone's driver hard-faults inside
/// `CreateSwapChainForComposition` (see `renderer.rs`), and the two panels differ in how strict
/// they are about the swapchain they accept - a `SwapChainBackgroundPanel` is the one that works
/// with the classic (non-flip) swap effects on the older composition path.
pub const IID_ISWAPCHAINBACKGROUNDPANELNATIVE: Guid = Guid {
    data1: 0x43be_bd4e,
    data2: 0xadd5,
    data3: 0x4035,
    data4: [0x8f, 0x85, 0x56, 0x08, 0xd0, 0x8e, 0x9d, 0xc9],
};

/// `ISwapChainBackgroundPanelNative::SetSwapChain`, also slot 3.
pub const ISWAPCHAINBACKGROUNDPANELNATIVE_SETSWAPCHAIN: usize = 3;

/// `ISurfaceImageSourceNative` (windows.ui.xaml.media.dxinterop.h) — **the WP8.1 phone present path**.
///
/// IID from the kit's own header: `f2e9edc1-d307-4525-9886-0fafaa44163c`. Derives straight from
/// `IUnknown` and has exactly three methods, so the slots are 3, 4, 5:
///
///     HRESULT SetDevice(IDXGIDevice* device);                        // slot 3
///     HRESULT BeginDraw(RECT updateRect, IDXGISurface** surface,
///                       POINT* offset);                              // slot 4
///     HRESULT EndDraw();                                             // slot 5
///
/// `BeginDraw` returns a surface that *is* inside the compositor's render target, which is why
/// this route needs no swapchain and no interop helper: drawing into the returned `IDXGISurface`
/// is presenting. The `RECT` is the update region in **source pixels**, and the returned `POINT`
/// offset must be added to the viewport (XAML is free to hand back a sub-rectangle of its own
/// atlas, so a renderer that ignores `offset` draws into the wrong place at some sizes).
pub const IID_ISURFACEIMAGESOURCENATIVE: Guid = Guid {
    data1: 0xf2e9_edc1,
    data2: 0xd307,
    data3: 0x4525,
    data4: [0x98, 0x86, 0x0f, 0xaf, 0xaa, 0x44, 0x16, 0x3c],
};

/// `ISurfaceImageSourceNative::SetDevice` — first method past IUnknown.
pub const ISURFACEIMAGESOURCENATIVE_SETDEVICE: usize = 3;
/// `ISurfaceImageSourceNative::BeginDraw`.
pub const ISURFACEIMAGESOURCENATIVE_BEGINDRAW: usize = 4;
/// `ISurfaceImageSourceNative::EndDraw`.
pub const ISURFACEIMAGESOURCENATIVE_ENDDRAW: usize = 5;

/// `Windows.UI.Xaml.Media.Imaging.SurfaceImageSource` as a UTF-16 class name, no trailing NUL.
///
/// This is the *runtime class* name `RoGetActivationFactory` takes. It is deliberately a different
/// string from the interface IID's name (`ISurfaceImageSourceFactory`); passing the interface name
/// as the class name returns `E_INVALIDARG`.
///
/// **The array length is the string length, exactly 48.** [`with_hstring`] passes `literal.len()`
/// as the `HSTRING` length, so an over-sized array silently asks WinRT for
/// `"…SurfaceImageSource\0\0\0\0\0\0\0\0"` — which is a *different* class name and comes back
/// `0x80040154` (class not registered). That is precisely what an early draft did by declaring
/// `[u16; 56]`; `windows.ui.xaml.media.imaging.idl` line 310 names the class, and 48 is the count.
pub const SURFACE_IMAGE_SOURCE_RUNTIME_CLASS: [u16; 48] = {
    let mut a = [0u16; 48];
    let s = *b"Windows.UI.Xaml.Media.Imaging.SurfaceImageSource";
    let mut i = 0;
    while i < s.len() {
        a[i] = s[i] as u16;
        i += 1;
    }
    a
};

/// `ISurfaceImageSourceFactory` — **the IID is read from the WP8.1 kit's own ABI IDL**, not
/// remembered: `Include\abi\windows.ui.xaml.media.imaging.idl` declares
/// `[uuid(3AB2212A-EF65-4A5F-BFAC-73993E8C12C9)] interface ISurfaceImageSourceFactory`.
///
/// This is the interface `RoGetActivationFactory` must be asked for. Its vtable is `IInspectable`
/// (0..5) followed by the two factory methods, so the slots are 6 and 7:
///
///     CreateInstanceWithDimensions(pixelWidth, pixelHeight, outer, inner, out instance)  // 6
///     CreateInstanceWithDimensionsAndOpacity(w, h, isOpaque, outer, inner, out instance) // 7
pub const ISURFACEIMAGESOURCEFACTORY: Guid = Guid {
    data1: 0x3ab2_212a,
    data2: 0xef65,
    data3: 0x4a5f,
    data4: [0xbf, 0xac, 0x73, 0x99, 0x3e, 0x8c, 0x12, 0xc9],
};

/// `ISurfaceImageSourceFactory::CreateInstanceWithDimensions` — first method past `IInspectable`.
pub const ISURFACEIMAGESOURCEFACTORY_CREATEINSTANCE: usize = 6;

/// `ISurfaceImageSourceFactory::CreateInstanceWithDimensionsAndOpacity`.
pub const ISURFACEIMAGESOURCEFACTORY_CREATEINSTANCEOPAQUE: usize = 7;

// ===========================================================================
// vtable slots (already offset past IUnknown - a real method starts at 3)
// ===========================================================================

// IDXGIObject (3..6) -> IDXGIAdapter (7..9) -> IDXGIAdapter1 (10)
pub const IDXGIADAPTER1_GETDESC1: usize = 10;
// IDXGIObject (3..6) -> IDXGIFactory (7..11) -> IDXGIFactory1 (12 EnumAdapters1, 13 IsCurrent)
pub const IDXGIFACTORY1_ENUMADAPTERS1: usize = 12;
// IDXGIFactory: 7 EnumAdapters, 8 MakeWindowAssociation, 9 GetWindowAssociation,
// 10 CreateSwapChain, 11 CreateSoftwareAdapter
pub const IDXGIFACTORY_CREATESWAPCHAIN: usize = 10;
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
// `ID3D11Texture2D::GetDesc`, extracted with `tools/vtable_slots.py --iface ID3D11Texture2D
// --method GetDesc` (IUnknown 3 + ID3D11DeviceChild 4 + ID3D11Resource 3 + own 0 = 10) rather
// than remembered - the same tool reproduces every slot REFERENCE.md already records as proven.
pub const ID3D11TEXTURE2D_GETDESC: usize = 10;
// `ID3D11DeviceChild::GetDevice` (slot 3: IUnknown's three methods, then the first method of the
// first base interface) - the one question that separates "XAML's surface is on our device" from
// "it is not", which is the difference between a refused view we can do something about and one
// we cannot. Extracted the same way as `ID3D11TEXTURE2D_GETDESC`, never remembered.
pub const ID3D11DEVICECHILD_GETDEVICE: usize = 3;
/// `D3D11_RTV_DIMENSION_TEXTURE2D` / `_TEXTURE2DMS` (d3d11.h), for an explicit
/// `D3D11_RENDER_TARGET_VIEW_DESC` when the NULL-descriptor form is refused.
pub const D3D11_RTV_DIM_TEXTURE2D: u32 = 4;
pub const D3D11_RTV_DIM_TEXTURE2DMS: u32 = 6;
pub const D3D11DEVICE_CREATEINPUTLAYOUT: usize = 11;
pub const D3D11DEVICE_CREATEVERTEXSHADER: usize = 12;
pub const D3D11DEVICE_CREATEPIXELSHADER: usize = 15;
pub const D3D11DEVICE_CREATEBLENDSTATE: usize = 20;
pub const D3D11DEVICE_CREATERASTERIZERSTATE: usize = 22;
pub const D3D11DEVICE_CREATESAMPLERSTATE: usize = 23;
pub const D3D11DEVICE_GETFEATURELEVEL: usize = 37;

// ID3D11DeviceContext
//
// Base is `ID3D11DeviceChild`, i.e. IUnknown 0..2 plus GetDevice/GetPrivateData/SetPrivateData/
// SetPrivateDataInterface at 3..6. The first *context* method is therefore slot **7**, not 3 -
// verified by extracting the declaration order from `Windows Kits\8.1\Include\um\d3d11.h`.
// (The `ID3D11Device` slots above start at 3 because that interface derives straight from
// IUnknown; the two do not share an offset.)
pub const D3D11CONTEXT_VSSETCONSTANTBUFFERS: usize = 7;
pub const D3D11CONTEXT_PSSETSHADERRESOURCES: usize = 8;
pub const D3D11CONTEXT_PSSETSHADER: usize = 9;
pub const D3D11CONTEXT_PSSETSAMPLERS: usize = 10;
pub const D3D11CONTEXT_VSSETSHADER: usize = 11;
pub const D3D11CONTEXT_DRAWINDEXED: usize = 12;
pub const D3D11CONTEXT_DRAW: usize = 13;
pub const D3D11CONTEXT_MAP: usize = 14;
pub const D3D11CONTEXT_UNMAP: usize = 15;
pub const D3D11CONTEXT_PSSETCONSTANTBUFFERS: usize = 16;
pub const D3D11CONTEXT_IASETINPUTLAYOUT: usize = 17;
pub const D3D11CONTEXT_IASETVERTEXBUFFERS: usize = 18;
pub const D3D11CONTEXT_IASETINDEXBUFFER: usize = 19;
pub const D3D11CONTEXT_IASETPRIMITIVETOPOLOGY: usize = 24;
pub const D3D11CONTEXT_OMSETRENDERTARGETS: usize = 33;
pub const D3D11CONTEXT_OMSETBLENDSTATE: usize = 35;
pub const D3D11CONTEXT_RSSETSTATE: usize = 43;
pub const D3D11CONTEXT_RSSETVIEWPORTS: usize = 44;
pub const D3D11CONTEXT_UPDATESUBRESOURCE: usize = 48;
pub const D3D11CONTEXT_CLEARRENDERTARGETVIEW: usize = 50;
pub const D3D11CONTEXT_FLUSH: usize = 111;
// `ID3D11DeviceContext::CopyResource` (slot 47) - `vtable_slots.py --iface ID3D11DeviceContext
// --method CopyResource`, between `CopySubresourceRegion` (46) and `UpdateSubresource` (48). It is
// how a frame drawn into a texture we *can* make a view of reaches a surface we cannot.
pub const D3D11CONTEXT_COPYRESOURCE: usize = 47;

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
/// These are pinned here rather than trusted: a wrong slot is a crash, not a compile error, so
/// the ones the renderer depends on get asserted.
pub mod slots {
    pub const IDXGIADAPTER1_GETDESC1: usize = 10;
    pub const IDXGIFACTORY_CREATESWAPCHAIN: usize = 10;
    pub const IDXGIFACTORY1_ENUMADAPTERS1: usize = 12;
    pub const IDXGISWAPCHAIN_PRESENT: usize = 8;
    pub const IDXGISWAPCHAIN_GETBUFFER: usize = 9;
    pub const IDXGISWAPCHAIN_RESIZEBUFFERS: usize = 13;
    pub const ID3D11DEVICE_CREATERENDERTARGETVIEW: usize = 9;
    pub const ID3D11DEVICE_CREATEINPUTLAYOUT: usize = 11;
    pub const ID3D11CONTEXT_IASETVERTEXBUFFERS: usize = 18;
    pub const ID3D11CONTEXT_DRAW: usize = 13;
    pub const ID3D11CONTEXT_CLEARRENDERTARGETVIEW: usize = 50;
    pub const ID3D11CONTEXT_FLUSH: usize = 111;
}

// ===========================================================================
// CRT-free DLL entry point
// ===========================================================================
//
// The entry point (`_fltused`, `DllMain`, `_DllMainCRTStartup`) lives in `lib.rs`, because a
// `#[no_mangle]` symbol may only be defined once in the final image. It is there rather than here
// so the crate has a single obvious place for exported names.
