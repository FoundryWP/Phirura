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

/// A *typeless* `DXGI_FORMAT` cannot back a view, which is why a NULL-descriptor
/// `CreateRenderTargetView` on such a resource answers `E_INVALIDARG`: D3D11 would have to invent a
/// typed format. Map the two families XAML composites in onto their UNORM form; anything already
/// typed passes through untouched.
///
/// Values from the kit's `dxgiformat.h`, not from memory (`R8G8B8A8_TYPELESS` is 27, not 1).
fn typed_view_format(fmt: i32) -> u32 {
    match fmt {
        27 => 28, // DXGI_FORMAT_R8G8B8A8_TYPELESS     -> DXGI_FORMAT_R8G8B8A8_UNORM
        90 => 87, // DXGI_FORMAT_B8G8R8A8_TYPELESS     -> DXGI_FORMAT_B8G8R8A8_UNORM
        92 => 88, // DXGI_FORMAT_B8G8R8X8_TYPELESS     -> DXGI_FORMAT_B8G8R8X8_UNORM
        other => other as u32,
    }
}

/// Number of entries in [`SurfaceChain::diag`], and their state before anything has been asked.
///
/// `[10]` (which device made the texture) and `[15]` (whether the two interface pointers are the
/// same address) start at `-1` rather than `0`: each has a `0` meaning that reads as a *finding*,
/// and a finding nobody measured is worse than no finding at all. `[11]` starts at `-99` for the
/// same reason — `0` there means "drew directly into XAML's surface", which no frame has done yet.
const DIAG_LEN: usize = 16;
const DIAG_INIT: [i32; DIAG_LEN] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, -99, 0, 0, 0, -1];

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
    /// That same surface QI'd to `ID3D11Texture2D` — the pointer everything actually wants.
    ///
    /// `BeginDraw` hands back an `IDXGISurface*`, and **an interface pointer may be used as another
    /// interface of the same object only when the two are pointer-identical**: true for single
    /// inheritance, false as soon as the implementation has more than one base. Nothing has
    /// established identity here, and passing an `IDXGISurface*` where an `ID3D11Resource*` is
    /// expected is exactly the kind of wrong that is *quiet* — `CreateRenderTargetView` answers
    /// `E_INVALIDARG` for a pointer it does not recognise, and `CopyResource` returns `void`, so a
    /// refused copy is indistinguishable from one that ran. QI is the only sanctioned way to get
    /// the resource, so the frame works from this pointer and from nothing else.
    pub frame_tex: ComPtr,
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
    /// Our own render target, used when XAML's surface refuses a view (see [`Self::build_rtv`]).
    ///
    /// It is made to match the surface's description exactly — same width, height and format —
    /// because `CopyResource` allows no mismatch, and is kept across frames: reallocating ~4 MB
    /// per frame to work around a refused view would be a second bug stacked on the first.
    pub own_texture: ComPtr,
    /// `own_texture`'s `[width, height, format]`, so a changed surface rebuilds it instead of
    /// copying between two resources that no longer match.
    pub own_key: [i32; 3],
    /// `true` when this frame drew into `own_texture` and must copy it into `frame_surface`
    /// before `EndDraw`.
    pub need_copy: bool,
    /// What the target surface *is*, and what the two render-target-view attempts answered.
    ///
    /// `[0]` format (`DXGI_FORMAT_*`), `[1]` `BindFlags`, `[2]` width, `[3]` height,
    /// `[4]` `SampleDesc.Count`, `[5]` `MiscFlags`, `[6]` `QI(ID3D11Texture2D)` HRESULT,
    /// `[7]` `CreateRenderTargetView(NULL desc)` HRESULT, `[8]` the explicit-descriptor retry's
    /// HRESULT, `[9]` the format the retry used,
    /// `[10]` whether the texture's `GetDevice` is *our* device (1 same, 0 other, -1 unknown),
    /// `[11]` which path the frame took (0 direct, 1 own-texture+copy, -1/-2/-3 refused),
    /// `[12]` the HRESULT that refused the own-texture path,
    /// `[13]`/`[14]` `BeginDraw`'s offset of this frame inside the source's atlas,
    /// `[15]` whether `IDXGISurface*` and `ID3D11Texture2D*` are the same address (1 same,
    ///        0 different, -1 not asked yet).
    ///
    /// Measured on the Lumia 925: `[7] = E_INVALIDARG` where `[0]`/`[1]` decide the fix. The
    /// surface is XAML's, so nothing about its shape can be inspected off-device - this array is
    /// the only way the phone log can say *why* a view of it was refused.
    pub diag: [i32; DIAG_LEN],
    /// A CPU-readable copy of the frame, made only by [`Self::probe_pixels`] and kept across them.
    ///
    /// `CopyResource` allows no mismatch, so this is keyed like `own_texture`: a surface that
    /// changes size rebuilds it instead of copying into a stale one.
    pub stage_tex: ComPtr,
    /// `stage_tex`'s `[width, height, format]`.
    pub stage_key: [i32; 3],
    /// Four pixels read back out of the frame, packed `0xAARRGGBB`, plus a fifth under the finger.
    ///
    /// Nothing in a `draw` call reports whether anything rasterized — `Draw` returns `void`, and
    /// `ClearRenderTargetView` succeeding says nothing about the fragments after it. These values
    /// are the only evidence the phone log can carry about what the driver actually painted.
    ///
    /// The fifth slot exists only once a touch has moved the quad, and is all zero until then: the
    /// four original points answer "did the draw happen", this one answers "did it follow the
    /// finger" — `vdbg[6]` says how many were actually read, so a `00000000` there is never
    /// mistaken for a measured zero.
    pub pixels: [u32; 5],
    /// The probe's sequence number from 0, or `-1` while nothing has ever been measured — which
    /// is what separates "measured a zero" from "measured nothing", the two a default value
    /// would merge.
    pub pixel_seq: i32,
    /// The viewport in force when those pixels were read, plus the probe's own status.
    ///
    /// `[0..2]` `TopLeftX`/`TopLeftY` in hundredths of a pixel, `[2..4]` `Width`/`Height` the
    /// same, `[4]`/`[5]` `MinDepth`/`MaxDepth` in thousandths, `[6]` how many probes ran,
    /// `[7]` the status (0 = read, negative = the HRESULT of the call that could not run).
    ///
    /// Recorded because a viewport of `(0, 0, 0, 0)` or a depth range of `(1, 1)` clears perfectly
    /// and clips every fragment, and no other field of the log would show it.
    pub vdbg: [i32; 8],
}

impl SurfaceChain {
    pub const EMPTY: SurfaceChain = SurfaceChain {
        source_inspectable: ComPtr::NULL,
        native: ComPtr::NULL,
        frame_surface: ComPtr::NULL,
        frame_tex: ComPtr::NULL,
        frame_rtv: ComPtr::NULL,
        frame_offset: Point { x: 0, y: 0 },
        width: 0,
        height: 0,
        in_frame: false,
        own_texture: ComPtr::NULL,
        own_key: [0; 3],
        need_copy: false,
        // `[10]`/`[15]` start *unknown*, not "0 = another device" / "0 = different address": a
        // default that means something specific would be believed before anything had asked.
        diag: DIAG_INIT,
        stage_tex: ComPtr::NULL,
        stage_key: [0; 3],
        pixels: [0; 5],
        pixel_seq: -1,
        vdbg: [0; 8],
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
            frame_tex: ComPtr::NULL,
            frame_rtv: ComPtr::NULL,
            frame_offset: Point { x: 0, y: 0 },
            width,
            height,
            in_frame: false,
            diag: DIAG_INIT,
            own_texture: ComPtr::NULL,
            own_key: [0; 3],
            need_copy: false,
            stage_tex: ComPtr::NULL,
            stage_key: [0; 3],
            pixels: [0; 5],
            pixel_seq: -1,
            vdbg: [0; 8],
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
    /// Returns `Ok` — the caller draws into [`SurfaceChain::frame_rtv`] — or `Err((step, hresult))`.
    ///
    /// `updateRect` is the region to redraw in *source* pixels. Passing the whole source keeps this
    /// simple and is correct; a renderer that knows its dirty region can pass less.
    ///
    /// **Both the previous frame's view and its surface are dropped here, before the new one is
    /// taken.** `BeginDraw` hands back a *new* `IDXGISurface` each frame and the old one is invalid
    /// after its `EndDraw`, so keeping the old render target view would mean drawing frame N into
    /// frame N-1's texture while `EndDraw` publishes frame N's — a frame loop that "runs" and shows
    /// nothing. `ComPtr` has no `Drop` (releases are explicit, see `direct.rs`), so skipping this
    /// also leaked one surface reference per frame.
    pub unsafe fn begin_draw(&mut self) -> Result<(), (i32, HRESULT)> {
        if self.in_frame {
            // A previous BeginDraw was never closed. Close it rather than leak, then proceed.
            self.end_draw();
        }
        // The view points into the surface, so it goes first (reversing the dependency order).
        if !self.frame_rtv.is_null() {
            self.frame_rtv.release();
            self.frame_rtv = ComPtr::NULL;
        }
        if !self.frame_surface.is_null() {
            self.frame_surface.release();
            self.frame_surface = ComPtr::NULL;
        }
        // The QI'd texture belongs to the same frame as the surface, so it goes with it.
        if !self.frame_tex.is_null() {
            self.frame_tex.release();
            self.frame_tex = ComPtr::NULL;
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
            surface.release();
            self.end_draw();
            return Err((SIS_QI_SURFACE, if hr < 0 { hr } else { E_NOINTERFACE }));
        }
        // One reference per frame is what `frame_surface` keeps; the pointer `BeginDraw` gave us is
        // a second one on the same object (QI identity), and dropping it here is what stops the
        // chain from growing by one reference every frame.
        surface.release();

        self.frame_surface = dxgi_surface;
        self.frame_offset = offset;
        self.in_frame = true;
        Ok(())
    }

    /// Read the target surface's `D3D11_TEXTURE2D_DESC` into [`SurfaceChain::diag`], and ask the
    /// texture which device made it.
    ///
    /// The surface belongs to XAML, so its shape cannot be known off-device: `CreateRenderTargetView`
    /// answering `E_INVALIDARG` on it does not say which argument was wrong. Format, `BindFlags`
    /// and sample count between them do — a typeless format means the NULL-descriptor form cannot
    /// work (a view needs a typed format) and has to be given explicitly, while a missing
    /// `D3D11_BIND_RENDER_TARGET` means no view is possible at all and the frame has to reach the
    /// surface some other way.
    ///
    /// `device` is ours, for the third possibility: a resource created by a *different* device is
    /// refused the same way, and `SetDevice` only asks XAML to use our device — it does not prove
    /// it did. `diag[10]` is the answer, straight from `GetDevice` pointer identity.
    pub unsafe fn inspect_surface(&mut self, device: &ComPtr) {
        self.diag[6] = E_NOINTERFACE;
        self.diag[10] = -1; // not known, unless the QI below gets far enough to ask
        self.diag[15] = -1; // likewise: no texture pointer yet, so no identity to compare
        if self.frame_surface.is_null() {
            return;
        }
        let mut tex = ComPtr::NULL;
        let hr = self.frame_surface.query_interface(
            &IID_ID3D11TEXTURE2D as *const Guid,
            &mut tex.vtbl as *mut _ as *mut *mut c_void,
        );
        self.diag[6] = hr;
        if hr < 0 || tex.is_null() {
            // Not an `ID3D11Texture2D` at all - say so rather than leave stale numbers behind.
            for slot in self.diag.iter_mut().take(6) {
                *slot = 0;
            }
            return;
        }
        let get_desc: extern "system" fn(*mut c_void, *mut D3D11_TEXTURE2D_DESC) =
            tex.method(ID3D11TEXTURE2D_GETDESC);
        let mut d = D3D11_TEXTURE2D_DESC::default();
        get_desc(tex.vtbl as *mut c_void, &mut d);

        // Which device owns it? `GetDevice` AddRefs, so the reference is dropped here rather than
        // left for someone who cannot see where it came from.
        let get_dev: extern "system" fn(*mut c_void, *mut *mut c_void) =
            tex.method(ID3D11DEVICECHILD_GETDEVICE);
        let mut owner = ComPtr::NULL;
        get_dev(
            tex.vtbl as *mut c_void,
            &mut owner.vtbl as *mut _ as *mut *mut c_void,
        );
        self.diag[10] = if owner.is_null() {
            -1
        } else if owner.vtbl == device.vtbl {
            1
        } else {
            0
        };
        owner.release();

        // Keep the QI'd texture: everything that must reach this resource as a *resource* (the
        // render target view, `CopyResource`) uses this pointer rather than the `IDXGISurface*`
        // `BeginDraw` handed back. Both are recorded, because whether they are the same address
        // is the question that decides which of those two answers `E_INVALIDARG`.
        self.diag[15] = if tex.vtbl == self.frame_surface.vtbl { 1 } else { 0 };
        if !self.frame_tex.is_null() {
            self.frame_tex.release();
        }
        self.frame_tex = tex;

        self.diag[0] = d.Format as i32;
        self.diag[1] = d.BindFlags as i32;
        self.diag[2] = d.Width as i32;
        self.diag[3] = d.Height as i32;
        self.diag[4] = d.SampleDesc.Count as i32;
        self.diag[5] = d.MiscFlags as i32;
        // Where in XAML's atlas this frame sits, for the log: a viewport built from a garbage
        // offset rasterizes into empty space, which looks exactly like a failed draw.
        self.diag[13] = self.frame_offset.x;
        self.diag[14] = self.frame_offset.y;
    }

    /// Read the target surface's shape *before the first frame* — `BeginDraw` + `GetDesc` +
    /// `EndDraw`, no view and no draws.
    ///
    /// The whole point is ordering: on the Adreno 225 building a view of this surface is what took
    /// the process down, and a description that would only be logged *after* `draw_test_quad`
    /// returns arrives too late to be any use. Running the read at attach time means C# logs
    /// `fmt`/`BindFlags` while the renderer is still demonstrably healthy, so the next failure is
    /// decidable from the log instead of guessed at.
    ///
    /// Not fatal on failure — the frame will hit the same wall and `sis_trace` names it — because
    /// losing the probe must not cost the bring-up log line that follows it.
    pub unsafe fn probe(&mut self, device: &ComPtr) -> Result<(), (i32, HRESULT)> {
        if self.native.is_null() {
            return Err((SIS_BEGIN_DRAW, E_NOINTERFACE));
        }
        self.begin_draw()?;
        self.inspect_surface(device);
        self.end_draw();
        Ok(())
    }

    /// Build `frame_rtv` over the current frame's surface, trying the documented form first and an
    /// explicit descriptor second — but only where the explicit descriptor can be the fix.
    ///
    /// Attempt 1 passes a NULL descriptor and lets D3D11 derive the view from the resource — what
    /// the offscreen route does successfully. On the Lumia 925 the `SurfaceImageSource` surface
    /// refuses it (`E_INVALIDARG`, step [`SIS_CREATE_RTV`]).
    ///
    /// **Attempt 2 is the dangerous one.** It exists solely for a *typeless* format, where D3D11
    /// cannot derive a view because a view must be typed. Handing an explicit descriptor to a
    /// surface whose format is already typed (or that was never bound as a render target, or that
    /// could not even be QI'd as a texture) asks the driver to redo a derivation it has already
    /// refused — measured on device, that call is where the process died. So it now runs only when
    /// all three preconditions hold, and otherwise returns attempt 1's failure immediately, which
    /// costs a black frame but keeps the app alive long enough to log why.
    ///
    /// `diag[7]` = attempt 1's HRESULT, `diag[8]` = attempt 2's (0 = skipped, with `diag[9]` giving
    /// the reason: -1 QI failed, -2 no `D3D11_BIND_RENDER_TARGET`, -3 format already typed),
    /// `diag[9]` = the format attempt 2 used when it ran.
    pub unsafe fn build_rtv(&mut self, device: &ComPtr) -> HRESULT {
        if self.frame_surface.is_null() {
            return E_NOINTERFACE;
        }
        // Stale attempt results from a previous frame/run would be read as this one's.
        self.diag[7] = 0;
        self.diag[8] = 0;
        self.diag[9] = 0;
        let create: extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            *mut *mut c_void,
        ) -> HRESULT = device.method(D3D11DEVICE_CREATERENDERTARGETVIEW);
        let this = device.vtbl as *mut c_void;
        // The *resource*, QI'd — not the `IDXGISurface*` `BeginDraw` returned. A view API takes
        // `ID3D11Resource*`, and passing a different interface of an object whose identity we have
        // never checked is precisely how you get `E_INVALIDARG` out of a description that is
        // otherwise correct. `diag[15]` records whether the two addresses matched after all.
        let res = if self.frame_tex.is_null() {
            self.frame_surface.vtbl as *mut c_void
        } else {
            self.frame_tex.vtbl as *mut c_void
        };

        // ---- attempt 1: NULL descriptor ----
        let mut rtv = ComPtr::NULL;
        let hr1 = create(this, res, core::ptr::null(), &mut rtv.vtbl as *mut _ as *mut *mut c_void);
        self.diag[7] = hr1;
        crate::set_tick(73); // NULL-descriptor view attempt returned
        if hr1 >= 0 && !rtv.is_null() {
            self.frame_rtv = rtv;
            self.need_copy = false;
            self.diag[11] = 0; // drew straight into XAML's surface
            return 0;
        }
        if !rtv.is_null() {
            rtv.release();
        }

        // ---- decide whether attempt 2 can possibly be the fix ----
        let qi_ok = self.diag[6] >= 0;
        let fmt_raw = self.diag[0];
        let is_typeless = fmt_raw == 27 || fmt_raw == 90 || fmt_raw == 92;
        let bind_ok = (self.diag[1] as u32 & D3D11_BIND_RENDER_TARGET) != 0;
        if !qi_ok || !is_typeless || !bind_ok {
            self.diag[8] = 0; // not attempted
            self.diag[9] = if !qi_ok {
                -1 // QI(ID3D11Texture2D) failed: nothing is known about the surface
            } else if !bind_ok {
                -2 // never created as a render target: no view is possible on this resource
            } else {
                -3 // format already typed: the NULL descriptor failed for some other reason
            };
            return self.build_own_target(device, hr1);
        }

        // ---- attempt 2: explicit descriptor, typed format ----
        //
        // `D3D11_RENDER_TARGET_VIEW_DESC` is `Format(4) + ViewDimension(4) + union(12)`; for both
        // TEXTURE2D and TEXTURE2DMS the union's first field is a MipSlice/unused UINT, so five
        // little-endian u32s is the whole struct with the rest correctly zero.
        let fmt = typed_view_format(fmt_raw);
        let dim = if self.diag[4] > 1 {
            D3D11_RTV_DIM_TEXTURE2DMS
        } else {
            D3D11_RTV_DIM_TEXTURE2D
        };
        let desc: [u32; 5] = [fmt, dim, 0, 0, 0];
        let mut rtv2 = ComPtr::NULL;
        crate::set_tick(74); // about to hand the driver an explicit descriptor
        let hr2 = create(
            this,
            res,
            desc.as_ptr() as *const c_void,
            &mut rtv2.vtbl as *mut _ as *mut *mut c_void,
        );
        self.diag[8] = hr2;
        self.diag[9] = fmt as i32;
        if hr2 >= 0 && !rtv2.is_null() {
            self.frame_rtv = rtv2;
            self.need_copy = false;
            self.diag[11] = 0;
            return 0;
        }
        if !rtv2.is_null() {
            rtv2.release();
        }
        // Neither form worked: stop asking this resource for a view and draw somewhere viewable.
        self.build_own_target(device, if hr1 < 0 { hr1 } else { hr2 })
    }

    /// Give up on viewing XAML's surface and make a render target of our own to draw into.
    ///
    /// The surface answers `E_INVALIDARG` to a view that its own `GetDesc` says should be legal
    /// (`fmt=87`, `BindFlags=0x20`, one sample — every precondition of the documented call), so
    /// the reason is not in the description and cannot be repaired by changing the arguments:
    /// measured on the Lumia 925, guessing at it took the process down. Rather than keep asking,
    /// draw into a texture **we** create — which is what the offscreen route already does there —
    /// and let [`Self::need_copy`] move the result into the surface before `EndDraw`.
    ///
    /// The texture is copied only when its shape matches (`own_key`), and it must: `CopyResource`
    /// allows no mismatch, so a surface that changes size has to rebuild it rather than silently
    /// copy into a stale one.
    ///
    /// `refused` is the HRESULT the view attempts answered, kept for the log. Returns 0 with
    /// `diag[11] = 1` on success; a negative `diag[11]` (`-1` texture, `-2` view) with the failing
    /// call's HRESULT in `diag[12]` means even the fallback could not be built, which is the case
    /// that says something about the *device* rather than about XAML's surface.
    unsafe fn build_own_target(&mut self, device: &ComPtr, refused: HRESULT) -> HRESULT {
        let w = self.diag[2] as u32;
        let h = self.diag[3] as u32;
        // Without a description there is nothing to match, and a view cannot be built blind.
        if self.diag[6] < 0 || w == 0 || h == 0 {
            self.diag[11] = -2;
            self.diag[12] = refused;
            return refused;
        }
        // The *exact* surface format, not a typed version of it: `CopyResource` compares formats
        // literally, so a typeless surface would need a typeless copy source.
        let surface_fmt = self.diag[0] as u32;
        let key = [w as i32, h as i32, surface_fmt as i32];
        if self.own_texture.is_null() || self.own_key != key {
            if !self.own_texture.is_null() {
                self.own_texture.release();
                self.own_texture = ComPtr::NULL;
            }
            let td = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: surface_fmt,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: self.diag[4].max(1) as u32,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let create_tex: extern "system" fn(
                *mut c_void,
                *const D3D11_TEXTURE2D_DESC,
                *const c_void,
                *mut *mut c_void,
            ) -> HRESULT = device.method(D3D11DEVICE_CREATETEXTURE2D);
            let mut tex = ComPtr::NULL;
            let hr = create_tex(
                device.vtbl as *mut c_void,
                &td,
                core::ptr::null(),
                &mut tex.vtbl as *mut _ as *mut *mut c_void,
            );
            if hr < 0 || tex.is_null() {
                self.diag[11] = -1;
                self.diag[12] = if hr < 0 { hr } else { E_NOINTERFACE };
                return self.diag[12];
            }
            self.own_texture = tex;
            self.own_key = key;
        }

        // An explicit descriptor, not the NULL form: on *our* texture that is not a guess — we
        // chose the format, the dimension and `D3D11_BIND_RENDER_TARGET` ourselves, and for a
        // typeless copy source it is the only way to a typed view.
        let fmt = typed_view_format(self.diag[0]);
        let dim = if self.diag[4] > 1 {
            D3D11_RTV_DIM_TEXTURE2DMS
        } else {
            D3D11_RTV_DIM_TEXTURE2D
        };
        let desc: [u32; 5] = [fmt, dim, 0, 0, 0];
        let create_rtv: extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            *mut *mut c_void,
        ) -> HRESULT = device.method(D3D11DEVICE_CREATERENDERTARGETVIEW);
        let mut rtv = ComPtr::NULL;
        let hr = create_rtv(
            device.vtbl as *mut c_void,
            self.own_texture.vtbl as *mut c_void,
            desc.as_ptr() as *const c_void,
            &mut rtv.vtbl as *mut _ as *mut *mut c_void,
        );
        if hr < 0 || rtv.is_null() {
            self.diag[11] = -2;
            self.diag[12] = if hr < 0 { hr } else { E_NOINTERFACE };
            return self.diag[12];
        }
        self.frame_rtv = rtv;
        self.need_copy = true;
        self.diag[11] = 1;
        self.diag[12] = 0;
        0
    }

    /// Copy the finished frame into a CPU-readable texture and read four pixels out of it.
    ///
    /// Nothing above this call can fail *visibly*: `Draw` returns `void`, and a clear that lands
    /// proves only that the view is bound — not that a single fragment was rasterized. Static
    /// analysis did eventually name the two faults (`VSSetShader` called with the wrong arity, and
    /// `Draw(4, 0)` over a triangle list forming a single triangle), but the readback stays: it is
    /// what turned each of them from a guess into a line the phone could print, and it is the only
    /// thing that can say a *later* change broke the draw.
    ///
    /// The four points separate the explanations that are still open:
    ///
    ///   * `(2, 0)` — the top row, *outside* the viewport. `ClearRenderTargetView` ignores the
    ///     viewport, so this says whether the clear reached the surface the compositor shows.
    ///   * `(w/2, h/2)` — the centre of the viewport, where the quad is at rest: did the draw
    ///     rasterize?
    ///   * `(w/2, h/4)`, `(w/2, h-8)` — inside the viewport, away from the quad: the clear colour
    ///     itself, so a centre pixel matching them means something rather than merely being blue.
    ///
    /// A fifth point is read only once a finger has moved the quad: the spot the quad came to rest
    /// in. `vdbg[6]` reports how many were actually read, so the empty fifth slot of the startup
    /// window can never be read as a measured zero.
    ///
    /// `pixels` is packed `0xAARRGGBB`; `pixel_seq` numbers the completed probes from 0 (and stays
    /// at `-1` until one succeeds, which is what separates "measured a zero" from "measured
    /// nothing"); `vdbg[7]` carries the status (0 = read, negative = the HRESULT that stopped it).
    pub unsafe fn probe_pixels(&mut self, ctx: ComPtr) -> i32 {
        self.vdbg[6] = 0;
        self.vdbg[7] = -1;
        if self.frame_tex.is_null() {
            return -1;
        }

        // ---- what is being copied out of --------------------------------------------------
        let get_desc: extern "system" fn(*mut c_void, *mut D3D11_TEXTURE2D_DESC) =
            self.frame_tex.method(ID3D11TEXTURE2D_GETDESC);
        let mut d = D3D11_TEXTURE2D_DESC::default();
        get_desc(self.frame_tex.vtbl as *mut c_void, &mut d);
        if d.Width == 0 || d.Height == 0 {
            self.vdbg[7] = E_INVALIDARG;
            return E_INVALIDARG;
        }
        // A multisampled resource cannot be a staging one and cannot be mapped, so there is no
        // readback at all rather than a readback that returns the wrong pixels.
        if d.SampleDesc.Count > 1 {
            self.vdbg[7] = -3;
            return -3;
        }

        // ---- the device that has to allocate the staging texture --------------------------
        // `GetDevice` returns `void` and AddRefs, so the reference is dropped right here.
        let get_dev: extern "system" fn(*mut c_void, *mut *mut c_void) =
            self.frame_tex.method(ID3D11DEVICECHILD_GETDEVICE);
        let mut dev = ComPtr::NULL;
        get_dev(
            self.frame_tex.vtbl as *mut c_void,
            &mut dev.vtbl as *mut _ as *mut *mut c_void,
        );
        if dev.is_null() {
            self.vdbg[7] = -2;
            return -2;
        }

        // ---- the staging texture, rebuilt whenever the surface changes ---------------------
        let key = [d.Width as i32, d.Height as i32, d.Format as i32];
        if self.stage_tex.is_null() || self.stage_key != key {
            if !self.stage_tex.is_null() {
                self.stage_tex.release();
                self.stage_tex = ComPtr::NULL;
            }
            // Same size and format as the source, because `CopyResource` allows no mismatch —
            // everything else stripped: a staging resource binds nothing and is CPU-readable.
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
            let create: extern "system" fn(
                *mut c_void,
                *const D3D11_TEXTURE2D_DESC,
                *const c_void,
                *mut *mut c_void,
            ) -> HRESULT = dev.method(D3D11DEVICE_CREATETEXTURE2D);
            let mut st = ComPtr::NULL;
            let hr = create(
                dev.vtbl as *mut c_void,
                &sd,
                core::ptr::null(),
                &mut st.vtbl as *mut _ as *mut *mut c_void,
            );
            dev.release();
            if hr < 0 || st.is_null() {
                if !st.is_null() {
                    st.release();
                }
                self.vdbg[7] = if hr < 0 { hr } else { E_NOINTERFACE };
                return self.vdbg[7];
            }
            self.stage_tex = st;
            self.stage_key = key;
        } else {
            dev.release();
        }

        // ---- copy, then map for reading ---------------------------------------------------
        let copy: extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
            ctx.method(D3D11CONTEXT_COPYRESOURCE);
        copy(
            ctx.vtbl as *mut c_void,
            self.stage_tex.vtbl as *mut c_void,
            self.frame_tex.vtbl as *mut c_void,
        );
        let mut mapped = D3D11_MAPPED_SUBRESOURCE {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        let map: extern "system" fn(
            *mut c_void,
            *mut c_void,
            UINT,
            D3D11_MAP,
            UINT,
            *mut D3D11_MAPPED_SUBRESOURCE,
        ) -> HRESULT = ctx.method(D3D11CONTEXT_MAP);
        let hr = map(
            ctx.vtbl as *mut c_void,
            self.stage_tex.vtbl as *mut c_void,
            0,
            D3D11_MAP_READ,
            0,
            &mut mapped,
        );
        if hr < 0 || mapped.pData.is_null() {
            self.vdbg[7] = if hr < 0 { hr } else { E_NOINTERFACE };
            return self.vdbg[7];
        }

        // ---- the four points ---------------------------------------------------------------
        let w = d.Width as i32;
        let h = d.Height as i32;
        // Four fixed points, plus a fifth once the quad has been moved by a finger. `pts[1]` is
        // where the quad sits *at rest*, so a touch probe reading the clear colour there and
        // `FFFF6633` at `pts[4]` is the log's proof that the draw followed the finger rather than
        // merely disappearing from the middle of the screen.
        let mut pts: [(i32, i32); 5] = [(0, 0); 5];
        pts[0] = (2, 0); // above the viewport: the clear, unconstrained by the viewport
        pts[1] = (w / 2, h / 2); // viewport centre: the quad at rest
        pts[2] = (w / 2, h / 4); // viewport, away from the quad: the clear colour
        pts[3] = (w / 2, h - 8); // viewport bottom
        let mut n = 4;
        if let Some((tx, ty)) = crate::touch_rest() {
            // The touch is in *viewport* pixels; these coordinates are in the surface, which the
            // compositor may have parked a couple of pixels down (`frame_offset`).
            pts[4] = (
                (tx + self.frame_offset.x as f32) as i32,
                (ty + self.frame_offset.y as f32) as i32,
            );
            n = 5;
        }
        let base = mapped.pData as *const u8;
        let pitch = mapped.RowPitch as usize;
        // `DXGI_FORMAT_R8G8B8A8_UNORM` (28) is R,G,B,A; BGRA8 is the other way round, and reading
        // a BGRA surface as RGBA reports the clear colour's blue in the red channel — a wrong
        // answer with every field populated, which is worse than no answer.
        let rgba = d.Format == 28;
        for (i, &(x, y)) in pts[..n].iter().enumerate() {
            let x = x.max(0).min(w - 1);
            let y = y.max(0).min(h - 1);
            let p = base.add(y as usize * pitch + x as usize * 4);
            let (r, g, b, a) = if rgba {
                (*p, *p.add(1), *p.add(2), *p.add(3))
            } else {
                (*p.add(2), *p.add(1), *p, *p.add(3))
            };
            self.pixels[i] =
                ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        }

        let unmap: extern "system" fn(*mut c_void, *mut c_void, UINT) = ctx.method(D3D11CONTEXT_UNMAP);
        unmap(ctx.vtbl as *mut c_void, self.stage_tex.vtbl as *mut c_void, 0);
        self.pixel_seq = if self.pixel_seq < 0 {
            0
        } else {
            self.pixel_seq + 1
        };
        self.vdbg[6] = n as i32;
        self.vdbg[7] = 0;
        0
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
        self.frame_tex.release();
        self.frame_tex = ComPtr::NULL;
        // The fallback render target outlives any one frame by design, so it is released here
        // rather than in `begin_draw`.
        self.own_texture.release();
        self.own_texture = ComPtr::NULL;
        self.own_key = [0; 3];
        self.need_copy = false;
        // The readback texture is the only other long-lived allocation; its pixels go with it, so
        // a later run cannot present this one's measurements as its own.
        self.stage_tex.release();
        self.stage_tex = ComPtr::NULL;
        self.stage_key = [0; 3];
        self.pixels = [0; 5];
        self.pixel_seq = -1;
        self.vdbg = [0, 0, 0, 0, 0, 0, 0, -1];
        self.native.release();
        self.native = ComPtr::NULL;
        // The `SurfaceImageSource` is intentionally released last: the compositor may still hold a
        // reference from XAML, and dropping ours first would leave it holding a dead object.
        self.source_inspectable.release();
        self.source_inspectable = ComPtr::NULL;
    }
}
