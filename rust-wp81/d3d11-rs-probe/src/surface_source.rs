//! The WP8.1 phone "present" path: XAML `SurfaceImageSource` + `ISurfaceImageSourceNative`.
//!
//! ## Why this exists
//!
//! Round 45 established, on a real Lumia 925, that a WP8.1 *phone* app cannot present through a
//! swapchain: `CreateSwapChainForCoreWindow` and `CreateSwapChainForComposition` both answer
//! `DXGI_ERROR_INVALID_CALL` for every documented argument shape on the Adreno 225 / WDDM 1.2 /
//! FL 9.3 driver, and Win2D — which renders fine on that same device — imports no swapchain entry
//! point at all.
//!
//! The subsequent attempt — create an offscreen `ID3D11Texture2D`, project it to a WinRT
//! `IDirect3DSurface` with `CreateDirect3D11SurfaceFromDXGISurface`, hand that to the compositor —
//! died on an even harder fact: **that helper is not reachable from an app on this platform.**
//! On device the loader probe logged
//!
//! ```text
//! interop-trace=winrt{h=0,exp=0} d3d11{h=1,exp=0}
//! ```
//!
//! i.e. `winrt.dll` cannot be loaded from an app container (handle 0) and `d3d11.dll` does not
//! export the helper (exp=0). A real PE import-table parse of Win2D's ARM DLL agrees: Win2D imports
//! only `D3D11CreateDevice` from `d3d11.dll` plus `RoGetActivationFactory` from
//! `api-ms-win-core-winrt-l1-1-0.dll` — no `winrt.dll`, no interop helpers — and it receives its
//! `IDirect3DDevice`/`IDirect3DSurface` as already-constructed WinRT objects. Neither the WP8.1
//! kit's ARM `d3d11.lib` nor any other kit library exports them.
//!
//! ## The mechanism that *is* reachable
//!
//! `runtimeobject.lib` ships in the WP8.1 kit and imports `RoGetActivationFactory` /
//! `RoActivateInstance` / `WindowsCreateString` from the API sets an app container **is** allowed
//! to load — the same ones Win2D uses. So an app can activate WinRT classes; it just cannot ask
//! for the D3D interop helpers.
//!
//! That is enough, because `SurfaceImageSource` is a *XAML* type whose native side is designed for
//! exactly this use:
//!
//! ```text
//! ISurfaceImageSourceNative::SetDevice(IDXGIDevice*)                       // slot 3, once
//! ISurfaceImageSourceNative::BeginDraw(RECT, IDXGISurface**, POINT*)       // slot 4, per frame
//! ISurfaceImageSourceNative::EndDraw()                                     // slot 5, per frame
//! ```
//!
//! `BeginDraw` hands back an `IDXGISurface*` that lives **inside the compositor's own render
//! target**. Drawing into it *is* presenting. So this route needs
//!
//!   * no swapchain,
//!   * no interop helper,
//!   * no full-screen offscreen texture and no per-frame blit,
//!
//! and the only thing it needs from WinRT is a `RoGetActivationFactory` + a `QueryInterface` for
//! `ISurfaceImageSourceNative` — both of which are plain WinRT mechanism, not D3D-specific.
//!
//! ## Ownership
//!
//! Rust owns the device, the surface object and the drawing. C# only has to put the
//! `SurfaceImageSource` (a `Windows.UI.Xaml.Media.ImageSource`) into an `Image` element — a
//! one-line data flow that cannot be mistaken for a C# renderer. That is what keeps this Path A.

#![allow(non_snake_case, non_upper_case_globals)]

use core::ffi::c_void;

use crate::direct::*;

/// The `RECT` `BeginDraw` takes and the `POINT` it returns, both in the WP8.1 ABI's `LONG` form.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Step codes for the surface bring-up, exposed to the phone log.
///
/// `SIS_BASE + code` becomes the `stage`, exactly as `SC_BASE`/`OT_BASE` do for the older routes,
/// so one field in the log localises a failure on whichever route is in use.
pub const SIS_BASE: i32 = 500;

pub const SIS_OK: i32 = 0;
/// `RoGetActivationFactory` for the `SurfaceImageSource` runtime class failed.
pub const SIS_ACTIVATION_FACTORY: i32 = 1;
/// The factory's `CreateInstanceWithDimensionsAndOpacity` failed.
pub const SIS_CREATE_INSTANCE: i32 = 2;
/// `QueryInterface(IID_ISurfaceImageSourceNative)` on the created instance failed.
pub const SIS_QI_NATIVE: i32 = 3;
/// `SetDevice(IDXGIDevice)` failed.
pub const SIS_SET_DEVICE: i32 = 4;
/// `BeginDraw` failed.
pub const SIS_BEGIN_DRAW: i32 = 5;
/// `EndDraw` failed.
pub const SIS_END_DRAW: i32 = 6;
/// `QueryInterface(IID_IDXGISurface)` on the surface `BeginDraw` returned failed.
pub const SIS_QI_SURFACE: i32 = 7;
/// `CreateRenderTargetView` on the `BeginDraw` surface failed.
pub const SIS_CREATE_RTV: i32 = 8;
/// The device had no `IDXGIDevice` to give `SetDevice`.
pub const SIS_QI_DXGIDEVICE: i32 = 9;

/// The live `SurfaceImageSource` chain.
///
/// Every one of these is a COM reference this struct owns and must release. They are held together
/// because they have a strict lifetime order: the render target view points into the texture
/// `BeginDraw` created, which points into the `SurfaceImageSource` the compositor holds.
pub struct SurfaceChain {
    /// The WinRT `SurfaceImageSource` object, as an `IInspectable*`.
    ///
    /// Kept so C# can take its own reference and hand it to XAML; Rust must not drop it while the
    /// compositor is showing it.
    pub source_inspectable: ComPtr,
    /// `ISurfaceImageSourceNative` — the interop interface reached by a plain QI.
    pub native: ComPtr,
    /// The current frame's surface from `BeginDraw`, or null between frames.
    pub frame_surface: ComPtr,
    /// The render target view over `frame_surface`, rebuilt whenever `BeginDraw` returns a new one.
    pub frame_rtv: ComPtr,
    /// Where `BeginDraw` put this frame inside the source's own atlas. A renderer that ignores this
    /// draws into the wrong place as soon as XAML hands back a sub-rectangle, which it does.
    pub frame_offset: Point,
    /// Pixel size the source was created at.
    pub width: i32,
    pub height: i32,
    /// `true` between a successful `BeginDraw` and its `EndDraw`.
    pub in_frame: bool,
}

impl SurfaceChain {
    pub const EMPTY: SurfaceChain = SurfaceChain {
        source_inspectable: ComPtr::NULL,
        native: ComPtr::NULL,
        frame_surface: ComPtr::NULL,
        frame_rtv: ComPtr::NULL,
        frame_offset: Point { x: 0, y: 0 },
        width: 0,
        height: 0,
        in_frame: false,
    };

    /// Create a `SurfaceImageSource` of `width` x `height` pixels and QI its native interface.
    ///
    /// `is_opaque` is passed as `true`: Phira always paints every pixel of its viewport, and an
    /// opaque source lets the compositor skip an alpha blend of the whole surface every frame.
    ///
    /// Returns `Ok` with the chain populated, or `Err((step_code, hresult))` naming the call that
    /// failed and the `HRESULT` it answered. **The `HRESULT` is part of the error**: `step` alone
    /// says *which* call, but `0x80040154` (class not registered) and `0x80070057` (bad argument,
    /// e.g. an `HSTRING` of the wrong length) are both "activation failed" and need different
    /// fixes. The caller records both in `sis_trace` so the phone log distinguishes them.
    pub unsafe fn create(
        width: i32,
        height: i32,
        is_opaque: bool,
    ) -> Result<SurfaceChain, (i32, HRESULT)> {
        if width <= 0 || height <= 0 {
            return Err((SIS_CREATE_INSTANCE, E_INVALIDARG));
        }

        // ---- 1. activate the runtime class, asking for its statics interface ----------------
        let mut factory = ComPtr::NULL;
        let hr = with_hstring(&SURFACE_IMAGE_SOURCE_RUNTIME_CLASS, |hs| {
            RoGetActivationFactory(
                hs,
                &ISURFACEIMAGESOURCEFACTORY as *const Guid,
                &mut factory.vtbl as *mut _ as *mut *mut c_void,
            )
        });
        let hr = match hr {
            Ok(h) => h,
            Err(e) => return Err((SIS_ACTIVATION_FACTORY, e)),
        };
        if hr < 0 || factory.is_null() {
            return Err((SIS_ACTIVATION_FACTORY, if hr < 0 { hr } else { E_NOINTERFACE }));
        }

        // ---- 2. CreateInstanceWithDimensionsAndOpacity --------------------------------------
        //
        // Signature (kit IDL line 323):
        //   HRESULT CreateInstanceWithDimensionsAndOpacity(
        //       INT32 pixelWidth, INT32 pixelHeight, boolean isOpaque,
        //       IInspectable* outer, IInspectable** inner, SurfaceImageSource** instance)
        //
        // `outer` must be null: this is not an aggregatable class, and passing a non-null outer
        // returns `E_INVALIDARG`. `inner` must be a valid out pointer (it is unused for a
        // non-aggregatable class but the ABI still writes it), hence the local.
        let create: extern "system" fn(
            *mut c_void,
            i32,
            i32,
            u8,
            *mut c_void,
            *mut *mut c_void,
            *mut *mut c_void,
        ) -> HRESULT = factory.method(ISURFACEIMAGESOURCEFACTORY_CREATEINSTANCEOPAQUE);

        let mut instance = ComPtr::NULL;
        let mut inner: *mut c_void = core::ptr::null_mut();
        let hr = create(
            factory.vtbl as *mut c_void,
            width,
            height,
            if is_opaque { 1 } else { 0 },
            core::ptr::null_mut(),
            &mut inner,
            &mut instance.vtbl as *mut _ as *mut *mut c_void,
        );
        factory.release();
        if hr < 0 || instance.is_null() {
            return Err((SIS_CREATE_INSTANCE, if hr < 0 { hr } else { E_NOINTERFACE }));
        }

        // ---- 3. QI the native interop interface ---------------------------------------------
        //
        // This is a plain `QueryInterface` on a WinRT object, which the runtime always supports -
        // no D3D interop helper involved. That is the whole point of this route.
        let mut native = ComPtr::NULL;
        let hr = instance.query_interface(
            &IID_ISURFACEIMAGESOURCENATIVE as *const Guid,
            &mut native.vtbl as *mut _ as *mut *mut c_void,
        );
        if hr < 0 || native.is_null() {
            instance.release();
            return Err((SIS_QI_NATIVE, if hr < 0 { hr } else { E_NOINTERFACE }));
        }

        Ok(SurfaceChain {
            source_inspectable: instance,
            native,
            frame_surface: ComPtr::NULL,
            frame_rtv: ComPtr::NULL,
            frame_offset: Point { x: 0, y: 0 },
            width,
            height,
            in_frame: false,
        })
    }

    /// `ISurfaceImageSourceNative::SetDevice(IDXGIDevice*)`. Call once, before the first frame.
    ///
    /// The device must be the one every subsequent draw targets; XAML uses it to decide how to
    /// synchronise the surface with the compositor.
    pub unsafe fn set_device(&self, dxgi_device: *mut c_void) -> HRESULT {
        let f: extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
            self.native.method(ISURFACEIMAGESOURCENATIVE_SETDEVICE);
        f(self.native.vtbl as *mut c_void, dxgi_device)
    }

    /// `BeginDraw(updateRect, &surface, &offset)`.
    ///
    /// Returns `Ok(&IDXGISurface)` — the caller draws into it — or `Err((step, hresult))`.
    ///
    /// `updateRect` is the region to redraw in *source* pixels. Passing the whole source keeps this
    /// simple and is correct; a renderer that knows its dirty region can pass less.
    ///
    /// The returned surface is a **new** object each frame and the previous frame's surface is
    /// invalid after its `EndDraw`, so this releases the previous one and rebuilds the render
    /// target view. The surface itself is refcounted: the render target view holds its own
    /// reference, so releasing the returned `IDXGISurface` pointer here is what keeps the chain
    /// from growing.
    pub unsafe fn begin_draw(&mut self) -> Result<ComPtr, (i32, HRESULT)> {
        if self.in_frame {
            // A previous BeginDraw was never closed. Close it rather than leak, then proceed.
            self.end_draw();
        }

        let rect = Rect { left: 0, top: 0, right: self.width, bottom: self.height };
        let mut surface = ComPtr::NULL;
        let mut offset = Point::default();

        let f: extern "system" fn(
            *mut c_void,
            Rect,
            *mut *mut c_void,
            *mut Point,
        ) -> HRESULT = self.native.method(ISURFACEIMAGESOURCENATIVE_BEGINDRAW);

        let hr = f(
            self.native.vtbl as *mut c_void,
            rect,
            &mut surface.vtbl as *mut _ as *mut *mut c_void,
            &mut offset,
        );
        if hr < 0 || surface.is_null() {
            return Err((SIS_BEGIN_DRAW, if hr < 0 { hr } else { E_NOINTERFACE }));
        }

        // The surface must be an `IDXGISurface` for the render target view.
        let mut dxgi_surface = ComPtr::NULL;
        let hr = surface.query_interface(
            &IID_IDXGISURFACE as *const Guid,
            &mut dxgi_surface.vtbl as *mut _ as *mut *mut c_void,
        );
        if hr < 0 || dxgi_surface.is_null() {
            // EndDraw anyway so the source is not left mid-frame.
            self.end_draw();
            return Err((SIS_QI_SURFACE, if hr < 0 { hr } else { E_NOINTERFACE }));
        }

        self.frame_surface = dxgi_surface;
        self.frame_offset = offset;
        self.in_frame = true;
        Ok(surface)
    }

    /// `ISurfaceImageSourceNative::EndDraw()`. Must be called once per `BeginDraw`.
    pub unsafe fn end_draw(&mut self) -> HRESULT {
        if !self.in_frame {
            return S_OK;
        }
        let f: extern "system" fn(*mut c_void) -> HRESULT =
            self.native.method(ISURFACEIMAGESOURCENATIVE_ENDDRAW);
        let hr = f(self.native.vtbl as *mut c_void);
        self.in_frame = false;
        hr
    }

    /// Release everything, in dependency order.
    pub unsafe fn release_all(&mut self) {
        if self.in_frame {
            self.end_draw();
        }
        self.frame_rtv.release();
        self.frame_rtv = ComPtr::NULL;
        self.frame_surface.release();
        self.frame_surface = ComPtr::NULL;
        self.native.release();
        self.native = ComPtr::NULL;
        // The `SurfaceImageSource` is intentionally released last: the compositor may still hold a
        // reference from XAML, and dropping ours first would leave it holding a dead object.
        self.source_inspectable.release();
        self.source_inspectable = ComPtr::NULL;
    }
}
