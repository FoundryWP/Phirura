using System;
using System.Runtime.InteropServices;
using System.Text;
using Windows.UI.Xaml.Controls;

namespace Phirura
{
    /// <summary>
    /// Bridge to <c>wp81d3d11rs.dll</c> - Path A: <b>Rust owns the D3D11 renderer</b>.
    ///
    /// This is the opposite direction from <see cref="PrprPort"/>. Path B had Rust record draw
    /// calls and C# replay them through Win2D; here Rust creates the device, the swapchain and the
    /// shaders itself, and C# only hands it a window to draw into and drives the frame loop.
    ///
    /// The contract mirrors <c>rust-wp81/d3d11-rs-probe/src/lib.rs</c> exactly:
    ///
    ///   wp81_rs_start(panel, width, height)  - bring up device + swapchain + pipeline; 0 = ok
    ///   wp81_rs_clear(r, g, b, a)            - clear the back buffer
    ///   wp81_rs_draw_test_quad()             - one textured quad (Stage 1 acceptance)
    ///   wp81_rs_present()                    - hand the frame to the compositor
    ///   wp81_rs_stop()                       - release everything
    ///   wp81_rs_stage()                      - how far start-up got (0 = fully up)
    ///   wp81_rs_last_hr()                    - the last HRESULT the Rust side saw
    ///
    /// Calling convention is <c>Cdecl</c>, matching the <c>extern "C"</c> on the Rust side.
    /// On ARM, <c>Cdecl</c> and <c>Stdcall</c> are the same thing, which is why
    /// <see cref="D3D11Probe"/> using StdCall also worked.
    /// </summary>
    public static class RustRenderer
    {
        private const string Dll = "wp81d3d11rs.dll";

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_start(IntPtr panel, uint width, uint height);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_start_upto(IntPtr panel, uint width, uint height, int limit);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_reached();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_rs_vt_count();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_rs_vt_entry(uint i);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_factory_is_current();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_factory_qi_hr();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_panel_iface();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_sc_hr(uint i);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_clear(float r, float g, float b, float a);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_draw_test_quad();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_present();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_rs_stop();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_stage();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_last_hr();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_last_hr_name();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_start_corewindow(IntPtr coreWindow, uint width, uint height);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_attach_panel(IntPtr panel);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_rs_corewindow_vtbl();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_rs_corewindow_obj();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_corewindow_qi();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_cw_factory_stereo();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_rs_vertex_count();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_sc_step();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_sc_step_name();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_sc_variant();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_sc_variant_name();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_tick();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_tick_name();

        // --- offscreen + WinRT-interop route (the corrected Stage 1) -------------------------
        // A WP8.1 *phone* app does not present through a swapchain at all: both
        // CreateSwapChainForCoreWindow and CreateSwapChainForComposition return
        // DXGI_ERROR_INVALID_CALL on this Adreno 225 / WDDM 1.2 / FL 9.3 driver. The phone path is
        // to render into an offscreen texture, wrap it as a WinRT IDirect3DSurface plus the matching
        // IDirect3DDevice, and hand both to the XAML compositor. Rust does all of that; C# only
        // forwards the two IInspectable pointers to a XAML Image/CanvasControl.
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_start_offscreen(uint width, uint height);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_gfx_surface();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_gfx_device();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_route();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_ot_step();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_ot_step_name();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_ot_surface_kind();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_interop_source();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_interop_trace(int i);

        // --- XAML `SurfaceImageSource` route (the corrected Stage 1) -------------------------
        //
        // Rounds 45-47 established, on the device, that neither a swapchain nor the D3D/WinRT
        // interop helpers are reachable from a WP8.1 phone app:
        //
        //   * `CreateSwapChainForCoreWindow` and `CreateSwapChainForComposition` both answer
        //     `DXGI_ERROR_INVALID_CALL` for every documented argument shape on the Adreno 225 /
        //     WDDM 1.2 / FL 9.3 driver;
        //   * `CreateDirect3D11SurfaceFromDXGISurface` cannot be resolved at all - the loader probe
        //     logged `interop-trace=winrt{h=0,exp=0} d3d11{h=1,exp=0}`, i.e. `winrt.dll` will not
        //     load from an app container and `d3d11.dll` does not export the helper.
        //
        // What *is* reachable is the WinRT activation API set (`RoGetActivationFactory`, imported
        // from `runtimeobject.lib`, the same one Win2D uses). That is enough for XAML's own
        // `SurfaceImageSource`: its native interface `ISurfaceImageSourceNative` is obtained by a
        // plain `QueryInterface` - no interop helper - and its `BeginDraw` hands back an
        // `IDXGISurface*` that lives inside the compositor's own render target. Drawing into it
        // *is* presenting, so there is no swapchain, no offscreen texture and no per-frame blit.
        //
        // Rust still owns the device, the surface, the shaders and every draw call. C# receives one
        // `IInspectable*` and assigns it to a XAML `Image.Source`, because XAML cannot be driven
        // from native code without a C#/C++ object to hang it on.
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_start_surface(uint width, uint height);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_sis_source();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_sis_step();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_rs_sis_step_name();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_rs_sis_hr();

        private static bool _started;

        public static bool Started { get { return _started; } }

        /// <summary>
        /// Bring the Rust renderer up on the **XAML `SurfaceImageSource`** route - the present path
        /// that actually works on a WP8.1 phone.
        ///
        /// Rust creates the device, activates the WinRT `SurfaceImageSource`, QIs
        /// `ISurfaceImageSourceNative` and calls `SetDevice`, then builds the full pipeline. The
        /// <c>IInspectable*</c> to show is read back with <see cref="SisSource"/>.
        ///
        /// Runs the native call on the calling thread; the page calls it from a pool thread so the
        /// UI thread stays alive to flush the log if a native step hard-faults.
        /// </summary>
        public static bool StartSurface(uint pixelWidth, uint pixelHeight)
        {
            if (_started)
            {
                return true;
            }
            try
            {
                AppLog.WriteSync("RustRenderer.StartSurface: calling native " + pixelWidth + "x" + pixelHeight);
                int rc;
                try
                {
                    rc = wp81_rs_start_surface(pixelWidth, pixelHeight);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("RustRenderer.StartSurface: native threw", ex);
                    return false;
                }

                AppLog.WriteSync("RustRenderer.StartSurface: native returned rc=" + rc
                    + " tick=" + Tick + " (" + TickName + ")");
                if (rc != 0)
                {
                    AppLog.WriteSync("RustRenderer.StartSurface: failed rc=" + rc + " " + Diagnose());
                    return false;
                }

                AppLog.WriteSync("RustRenderer.StartSurface: up. route=" + Route
                    + " source=0x" + SisSource.ToInt32().ToString("X8"));
                _started = true;
                return true;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.StartSurface: FAILED", ex);
                return false;
            }
        }

        /// <summary>
        /// The WinRT <c>SurfaceImageSource*</c> (an <c>IInspectable</c>, and a
        /// <c>Windows.UI.Xaml.Media.ImageSource</c>) the compositor must display. Only valid while
        /// the surface route is up; Rust keeps ownership, so callers must NOT release it.
        /// </summary>
        public static IntPtr SisSource
        {
            get
            {
                try { return wp81_rs_sis_source(); }
                catch (Exception) { return IntPtr.Zero; }
            }
        }

        /// <summary>Raw `SurfaceImageSource` sub-step code (500+n), 0 if that route never ran.</summary>
        public static int SisStep
        {
            get
            {
                try { return wp81_rs_sis_step(); }
                catch (Exception) { return 0; }
            }
        }

        /// <summary>The failing `SurfaceImageSource` call by name, e.g. "surface: SetDevice".</summary>
        public static string SisStepName
        {
            get
            {
                try
                {
                    IntPtr p = wp81_rs_sis_step_name();
                    return p == IntPtr.Zero ? "(null)" : Marshal.PtrToStringAnsi(p);
                }
                catch (Exception) { return "(unavailable)"; }
            }
        }

        /// <summary>The HRESULT recorded alongside <see cref="SisStep"/>.</summary>
        public static int SisHr
        {
            get
            {
                try { return wp81_rs_sis_hr(); }
                catch (Exception) { return 0; }
            }
        }

        /// <summary>
        /// Bring the Rust renderer up on the **offscreen + WinRT-interop** route - the one that
        /// actually works on a WP8.1 phone. Creates the device, an offscreen render target and the
        /// WinRT <c>IDirect3DSurface</c>/<c>IDirect3DDevice</c> pair, then builds the full pipeline.
        ///
        /// Runs the native call on the calling thread; the page calls it from a pool thread so the
        /// UI thread stays alive to flush the log if a native step hard-faults.
        /// </summary>
        public static bool StartOffscreen(uint pixelWidth, uint pixelHeight)
        {
            if (_started)
            {
                return true;
            }
            try
            {
                AppLog.WriteSync("RustRenderer.StartOffscreen: calling native " + pixelWidth + "x" + pixelHeight);
                int rc;
                try
                {
                    rc = wp81_rs_start_offscreen(pixelWidth, pixelHeight);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("RustRenderer.StartOffscreen: native threw", ex);
                    return false;
                }

                AppLog.WriteSync("RustRenderer.StartOffscreen: native returned rc=" + rc
                    + " tick=" + Tick + " (" + TickName + ")");
                if (rc != 0)
                {
                    AppLog.WriteSync("RustRenderer.StartOffscreen: failed rc=" + rc + " " + Diagnose());
                    return false;
                }

                AppLog.WriteSync("RustRenderer.StartOffscreen: up. route=" + Route
                    + " surface=0x" + GfxSurface.ToInt32().ToString("X8")
                    + " device=0x" + GfxDevice.ToInt32().ToString("X8"));
                _started = true;
                return true;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.StartOffscreen: FAILED", ex);
                return false;
            }
        }

        /// <summary>Which route came up: 0 = swapchain (composition), 1 = offscreen+interop, 2 = CoreWindow, 3 = XAML SurfaceImageSource, -1 = none.</summary>
        public static int Route
        {
            get
            {
                try { return wp81_rs_route(); }
                catch (Exception) { return -1000; }
            }
        }

        /// <summary>A name for <see cref="Route"/>, for the log and the HUD.</summary>
        public static string RouteName
        {
            get
            {
                switch (Route)
                {
                    case 0: return "swapchain";
                    case 1: return "offscreen";
                    case 2: return "corewindow";
                    case 3: return "surfaceimagesource";
                    default: return "(none)";
                }
            }
        }

        /// <summary>
        /// The WinRT <c>IDirect3DSurface*</c> (an <c>IInspectable</c>) wrapping the offscreen render
        /// target. Only valid while the offscreen route is up; the compositor consumes it and the
        /// Rust side keeps ownership, so callers must NOT release it.
        /// </summary>
        public static IntPtr GfxSurface
        {
            get
            {
                try { return wp81_rs_gfx_surface(); }
                catch (Exception) { return IntPtr.Zero; }
            }
        }

        /// <summary>The WinRT <c>IDirect3DDevice*</c> matching <see cref="GfxSurface"/>. Rust owns it.</summary>
        public static IntPtr GfxDevice
        {
            get
            {
                try { return wp81_rs_gfx_device(); }
                catch (Exception) { return IntPtr.Zero; }
            }
        }

        /// <summary>Raw offscreen sub-step code (40..47), 0 if the offscreen route never ran.</summary>
        public static int OtStep
        {
            get
            {
                try { return wp81_rs_ot_step(); }
                catch (Exception) { return 0; }
            }
        }

        /// <summary>
        /// Which DXGI surface interface the QI chain resolved on the offscreen texture:
        /// 2 = IDXGISurface2, 1 = IDXGISurface1, 0 = IDXGISurface, -1 = none answered.
        ///
        /// Carried in the second slot of Rust's offscreen trace once step 47 has run, because
        /// which interface the WP8.1 interop helper accepts is the one thing that cannot be
        /// determined off-device.
        /// </summary>
        public static int OtSurfaceKind
        {
            get
            {
                try
                {
                    if (wp81_rs_ot_step() != 47) { return -1; }
                    return wp81_rs_ot_surface_kind();
                }
                catch (Exception) { return -1; }
            }
        }

        /// <summary>Name for <see cref="OtSurfaceKind"/>.</summary>
        public static string OtSurfaceKindName
        {
            get
            {
                switch (OtSurfaceKind)
                {
                    case 2: return "IDXGISurface2";
                    case 1: return "IDXGISurface1";
                    case 0: return "IDXGISurface";
                    default: return "(none)";
                }
            }
        }

        /// <summary>
        /// Which module the D3D11/XAML interop helpers resolved from: 1 = winrt.dll (the phone's
        /// implementation), 2 = d3d11.dll (desktop Windows), 0 = neither, -1 = never attempted.
        ///
        /// This is the fix for the E_NOINTERFACE that survived two rounds: resolving
        /// CreateDirect3D11SurfaceFromDXGISurface out of d3d11.dll finds a symbol on this phone,
        /// but not a working one. Win2D's ARM DLL imports both helpers from winrt.dll.
        /// </summary>
        public static int InteropSource
        {
            get
            {
                try { return wp81_rs_interop_source(); }
                catch (Exception) { return -1; }
            }
        }

        /// <summary>Name for <see cref="InteropSource"/>.</summary>
        public static string InteropSourceName
        {
            get
            {
                switch (InteropSource)
                {
                    case 1: return "winrt.dll";
                    case 2: return "d3d11.dll";
                    case 0: return "(none)";
                    default: return "(n/a)";
                }
            }
        }

        /// <summary>
        /// The interop-resolution trace, as a compact string, e.g.
        /// <c>winrt{h=1,exp=1} d3d11{h=1,exp=0}</c>.
        ///
        /// <c>h</c> = the module loaded, <c>exp</c> = it exported the helper name. This is what
        /// distinguishes "the DLL isn't there" from "the DLL is there but does not export it" -
        /// both of which currently surface as the same <c>E_NOINTERFACE</c>.
        /// </summary>
        public static string InteropTrace
        {
            get
            {
                try
                {
                    return "winrt{h=" + wp81_rs_interop_trace(0) + ",exp=" + wp81_rs_interop_trace(1)
                        + "} d3d11{h=" + wp81_rs_interop_trace(2) + ",exp=" + wp81_rs_interop_trace(3) + "}";
                }
                catch (Exception) { return "(unavailable)"; }
            }
        }

        /// <summary>The failing offscreen call by name, e.g. "offscreen: CreateTexture2D".</summary>
        public static string OtStepName
        {
            get
            {
                try
                {
                    IntPtr p = wp81_rs_ot_step_name();
                    return p == IntPtr.Zero ? "(null)" : Marshal.PtrToStringAnsi(p);
                }
                catch (Exception) { return "(unavailable)"; }
            }
        }

        /// <summary>How far Rust's start-up got: 0 = fully up, 1 = device failed, 2 = swapchain, …</summary>
        public static int Stage
        {
            get
            {
                try { return wp81_rs_stage(); }
                catch (Exception) { return -1000; }   // -1000 = the DLL did not even load
            }
        }

        /// <summary>The last HRESULT the Rust side observed.</summary>
        public static int LastHr
        {
            get
            {
                try { return wp81_rs_last_hr(); }
                catch (Exception) { return 0; }
            }
        }

        /// <summary>
        /// The last HRESULT decoded to a DXGI name, e.g. <c>DXGI_ERROR_INVALID_CALL</c>, or "" if
        /// the value has no table entry. A name is what turns "hr=0x887A0001" into a sentence.
        /// </summary>
        public static string LastHrName
        {
            get
            {
                try
                {
                    IntPtr p = wp81_rs_last_hr_name();
                    return p == IntPtr.Zero ? "" : Marshal.PtrToStringAnsi(p);
                }
                catch (Exception) { return ""; }
            }
        }

        /// <summary>Raw swapchain-attachment sub-step code (20..25), 0 if it did not get that far.</summary>
        public static int ScStep
        {
            get
            {
                try { return wp81_rs_sc_step(); }
                catch (Exception) { return 0; }
            }
        }

        /// <summary>The failing swapchain call by name, e.g. "IDXGIAdapter::GetParent(IDXGIFactory2)".</summary>
        public static string ScStepName
        {
            get
            {
                try
                {
                    IntPtr p = wp81_rs_sc_step_name();
                    return p == IntPtr.Zero ? "(null)" : Marshal.PtrToStringAnsi(p);
                }
                catch (Exception) { return "(unavailable)"; }
            }
        }

        /// <summary>Which swapchain descriptor variant the phone's driver accepted (-1 = none).</summary>
        public static int ScVariant
        {
            get
            {
                try { return wp81_rs_sc_variant(); }
                catch (Exception) { return -1; }
            }
        }

        /// <summary>The accepted descriptor shape by name, e.g. "FLIP_SEQUENTIAL x2 PREMULTIPLIED".</summary>
        public static string ScVariantName
        {
            get
            {
                try
                {
                    IntPtr p = wp81_rs_sc_variant_name();
                    return p == IntPtr.Zero ? "" : Marshal.PtrToStringAnsi(p);
                }
                catch (Exception) { return ""; }
            }
        }

        /// <summary>Which XAML panel-interop interface answered: 0 = SwapChainPanel, 1 = BackgroundPanel.</summary>
        public static int PanelIface
        {
            get
            {
                try { return wp81_rs_panel_iface(); }
                catch (Exception) { return -1; }
            }
        }

        /// <summary>Name for <see cref="PanelIface"/>.</summary>
        public static string PanelIfaceName
        {
            get
            {
                switch (PanelIface)
                {
                    case 0: return "ISwapChainPanelNative";
                    case 1: return "ISwapChainBackgroundPanelNative";
                    default: return "(none)";
                }
            }
        }

        /// <summary>A one-line diagnosis of a failed Start(), for the log and the HUD.</summary>
        public static string Diagnose()
        {
            int stage = Stage;
            int hr = LastHr;
            string name = LastHrName;
            string s = "stage=" + stage + " hr=0x" + hr.ToString("X8")
                + (name.Length > 0 ? " (" + name + ")" : "")
                + " tick=" + Tick + " (" + TickName + ")";
            // Stage codes >= 500 come from the SurfaceImageSource route (SIS_BASE + sub-step), 300+
            // from the offscreen route (OT_BASE + sub-step), 100..199 from the swapchain route.
            // Report whichever produced the code so the log names the call.
            if (stage >= 500)
            {
                s += "  surface-step=" + SisStepName + " (code " + SisStep + ")"
                    + " surface-hr=0x" + SisHr.ToString("X8");
            }
            else if (stage >= 300)
            {
                s += "  offscreen-step=" + OtStepName + " (code " + OtStep + ")";
            }
            else if (stage >= 100)
            {
                s += "  swapchain-step=" + ScStepName + " (code " + ScStep + ")";
            }
            s += CoreWindowDiag();
            return s;
        }

        /// <summary>
        /// The CoreWindow-pointer probe appended to <see cref="Diagnose"/>: object address, vtable
        /// address and the HRESULT of <c>QI(IUnknown)</c> on the pointer the composition-free route
        /// passed to <c>CreateSwapChainForCoreWindow</c>.
        ///
        /// This exists to tell a proxy apart from the real object when the call fails with
        /// <c>E_INVALIDARG</c>. Returns an empty string if the route never ran.
        /// </summary>
        private static string CoreWindowDiag()
        {
            try
            {
                uint obj = wp81_rs_corewindow_obj();
                if (obj == 0)
                {
                    return string.Empty;
                }
                uint vtbl = wp81_rs_corewindow_vtbl();
                int qi = wp81_rs_corewindow_qi();
                string qiName;
                switch (qi)
                {
                    case 0: qiName = "S_OK"; break;
                    case 1: qiName = "S_FALSE"; break;
                    case unchecked((int)0x80004002): qiName = "E_NOINTERFACE"; break;
                    case 0x7FFFFFFF: qiName = "(not probed)"; break;
                    default: qiName = "0x" + qi.ToString("X8"); break;
                }
                string stereo;
                int st = wp81_rs_cw_factory_stereo();
                if (st == -1) stereo = "n/a";
                else stereo = st.ToString();
                return "  cw-obj=0x" + obj.ToString("X8") + " cw-vtbl=0x" + vtbl.ToString("X8")
                    + " cw-qi=" + qiName + " cw-stereo=" + stereo;
            }
            catch (Exception)
            {
                return "  cw-probe=(unavailable)";
            }
        }

        /// <summary>
        /// The last native step Rust entered - the only evidence of where a hard fault happened,
        /// since the phone has no debugger and the log buffers are lost when the process dies.
        /// </summary>
        public static int Tick
        {
            get
            {
                try { return wp81_rs_tick(); }
                catch (Exception) { return -1000; }
            }
        }

        /// <summary>The progress tick by name, e.g. "swapchain: CreateSwapChainForComposition".</summary>
        public static string TickName
        {
            get
            {
                try
                {
                    IntPtr p = wp81_rs_tick_name();
                    return p == IntPtr.Zero ? "(null)" : Marshal.PtrToStringAnsi(p);
                }
                catch (Exception) { return "(unavailable)"; }
            }
        }

        /// <summary>
        /// The last tick this side actually observed *live*, kept in managed memory.
        ///
        /// <see cref="Tick"/> reads the DLL's counter, but if the process dies inside a native call
        /// that read never happens. This value is refreshed by a timer while the frame loop runs, so
        /// it always holds the last step that was known to be executing - the crash's address, in
        /// effect. -1 means nothing was sampled yet.
        /// </summary>
        public static int LastSeenTick { get { return _lastSeenTick; } }

        /// <summary>Name for <see cref="LastSeenTick"/>, also kept in managed memory.</summary>
        public static string LastSeenTickName { get { return _lastSeenTickName; } }

        private static int _lastSeenTick = -1;
        private static string _lastSeenTickName = "(never)";

        /// <summary>
        /// Read the DLL's tick once and remember it. Called from a 200 ms timer on the page, so the
        /// value survives a native fault that would otherwise kill the process before any log write.
        /// Returns true if the tick changed since the previous sample.
        /// </summary>
        public static bool SampleTick()
        {
            int t;
            try { t = wp81_rs_tick(); }
            catch (Exception) { return false; }

            // The CoreWindow prologue runs for a few microseconds and bumps ticks 200..202, which a
            // 200 ms sampler can easily miss. The probe's *values* are persistent statics, though, so
            // emit them the first time any of them is non-default - that way a crash immediately
            // after the probe still records what the pointer looked like.
            if (!_cwProbeLogged)
            {
                try
                {
                    if (wp81_rs_corewindow_obj() != 0 || wp81_rs_corewindow_qi() != 0x7FFFFFFF)
                    {
                        _cwProbeLogged = true;
                        AppLog.WriteSync("RustRenderer.cw-probe: " + CoreWindowDiag());
                    }
                }
                catch (Exception) { }
            }

            if (t == _lastSeenTick)
            {
                return false;
            }
            _lastSeenTick = t;
            _lastSeenTickName = TickName;
            return true;
        }

        private static bool _cwProbeLogged;

        /// <summary>
        /// Probe a raw <c>IUnknown*</c> before it is handed to native, and log everything observable
        /// about it. Must run on the UI thread (it is), and must log synchronously: the native call
        /// that follows can hard-fault the process, and the log's last flushed line is then the only
        /// evidence of what the pointer was.
        ///
        /// Three questions are answered, in order of how much they prove:
        ///   1. <c>QI(IUnknown)</c> - does a COM object answer at all? (It always does if it is one.)
        ///   2. <c>Marshal.GetObjectForIUnknown</c> - what managed type does the runtime project it
        ///      back to? A real CoreWindow round-trips to <c>Windows.UI.Core.CoreWindow</c>; a proxy
        ///      or an unrelated object does not.
        ///   3. A <c>QueryInterface</c> for the CoreWindow runtime class' identity - the last word on
        ///      whether DXGI's own QI inside <c>CreateSwapChainForCoreWindow</c> will succeed.
        /// </summary>
        private static void LogCoreWindowPointer(IntPtr p)
        {
            var sb = new StringBuilder();
            sb.Append("RustRenderer.cw-probe: ptr=0x").Append(p.ToInt32().ToString("X8"));

            // (1) QI(IUnknown) - the pointer must at least be a COM object.
            try
            {
                IntPtr unk;
                Guid iidUnknown = new Guid(0x00000000, 0x0000, 0x0000,
                    0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46);
                int hr = Marshal.QueryInterface(p, ref iidUnknown, out unk);
                sb.Append(" qi(IUnknown)=0x").Append(hr.ToString("X8"));
                if (hr == 0 && unk != IntPtr.Zero)
                {
                    Marshal.Release(unk);
                }
            }
            catch (Exception ex)
            {
                sb.Append(" qi(IUnknown)=threw(").Append(ex.GetType().Name).Append(')');
            }

            // (2) What does the object actually project back to as a managed type?
            try
            {
                object o = Marshal.GetObjectForIUnknown(p);
                sb.Append(" as=").Append(o == null ? "(null)" : o.GetType().FullName);
            }
            catch (Exception ex)
            {
                sb.Append(" as=threw(").Append(ex.GetType().Name).Append(')');
            }

            // (3) Does it expose the CoreWindow runtime class itself?
            try
            {
                object o = Marshal.GetObjectForIUnknown(p);
                bool isCw = o is Windows.UI.Core.CoreWindow;
                sb.Append(" isCoreWindow=").Append(isCw ? "yes" : "no");
            }
            catch (Exception ex)
            {
                sb.Append(" isCoreWindow=threw(").Append(ex.GetType().Name).Append(')');
            }

            AppLog.WriteSync(sb.ToString());
        }

        /// <summary>
        /// Log the vtable snapshot Rust recorded for the DXGI factory (see `dump_vtable`).
        ///
        /// This is the one question that cannot be asked any other way on the phone: does the object
        /// `GetParent(IID_IDXGIFactory2)` handed back actually have a slot 24? A short table is what
        /// makes `CreateSwapChainForComposition` jump into garbage and hard-fault.
        /// </summary>
        public static void LogVtable()
        {
            try
            {
                uint count = wp81_rs_vt_count();
                AppLog.WriteSync("RustRenderer.vtable: readable slots = " + count);
                if (count == 0)
                {
                    return;
                }
                StringBuilder sb = new StringBuilder();
                sb.Append("RustRenderer.vtable:");
                uint n = Math.Min(count, 26u);
                for (uint i = 0; i < n; i++)
                {
                    uint p = wp81_rs_vt_entry(i);
                    sb.Append(' ');
                    sb.Append(i);
                    sb.Append("=0x");
                    sb.Append(p.ToString("X8"));
                }
                AppLog.WriteSync(sb.ToString());
                // Slot 24 is CreateSwapChainForComposition - the call that faults.
                uint s3 = wp81_rs_vt_entry(3);
                uint s24 = wp81_rs_vt_entry(24);
                AppLog.WriteSync("RustRenderer.vtable: slot3(GetParent)=0x" + s3.ToString("X8")
                    + " slot24(CreateSwapChainForComposition)=0x" + s24.ToString("X8")
                    + (s24 == 0 ? "  <-- NULL: table is too short!" : ""));
                // The factory soundness probe: a sane IsCurrent plus a successful QI round-trip
                // means the factory object is good and only slot 24 faults.
                AppLog.WriteSync("RustRenderer.factory: IsCurrent=" + wp81_rs_factory_is_current()
                    + " QI(IDXGIFactory2) hr=0x" + wp81_rs_factory_qi_hr().ToString("X8"));
                // Which XAML interop interface the panel answered: 0 = SwapChainPanelNative,
                // 1 = SwapChainBackgroundPanelNative, -1 = neither (the attach step has not run).
                int iface = wp81_rs_panel_iface();
                AppLog.WriteSync("RustRenderer.panel: iface=" + iface
                    + (iface == 0 ? " (ISwapChainPanelNative)"
                       : iface == 1 ? " (ISwapChainBackgroundPanelNative)"
                       : " (none)"));
                // Every CreateSwapChainForComposition descriptor shape's HRESULT, in try order.
                // All the same error = "no shape works with this API"; a mix = "one field is wrong".
                // Each value is also decoded to a DXGI name, because the names are what diagnose it.
                StringBuilder schr = new StringBuilder("RustRenderer.sc-hr:");
                for (uint i = 0; i < 4; i++)
                {
                    int h = wp81_rs_sc_hr(i);
                    schr.Append(" v");
                    schr.Append(i);
                    schr.Append('=');
                    if (h == 0x7FFFFFFF)
                    {
                        schr.Append("n/a");
                    }
                    else
                    {
                        schr.Append("0x");
                        schr.Append(h.ToString("X8"));
                        if (h >= 0)
                        {
                            schr.Append("(OK)");
                        }
                    }
                }
                AppLog.WriteSync(schr.ToString());
                AppLog.WriteSync("RustRenderer.last-hr: 0x" + LastHr.ToString("X8")
                    + (LastHrName.Length > 0 ? " (" + LastHrName + ")" : ""));
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.LogVtable failed", ex);
            }
        }

        /// <summary>
        /// Bisect a hard fault inside start-up by running the native bring-up in steps, logging each.
        ///
        /// Each call stops *before* the next risky native call. If the process survives step N but
        /// dies at step N+1, the very last line in the log names the call that faulted. Returns the
        /// highest step that completed; a value less than the last requested limit means a step
        /// failed (or the process died and this line is the forensic record).
        /// </summary>
        public static int Bisect(SwapChainPanel panel, uint pixelWidth, uint pixelHeight)
        {
            AppLog.WriteSync("RustRenderer.Bisect: begin " + pixelWidth + "x" + pixelHeight);
            if (panel == null)
            {
                AppLog.WriteSync("RustRenderer.Bisect: panel is null");
                return -1;
            }

            // Steps, in order, and what the *next* call after each one does. Keep in sync with
            // wp81_rs_start_upto in rust-wp81/d3d11-rs-probe/src/lib.rs.
            //
            // 13 vs 18 are the pair that separates the two remaining suspects: both stop at the
            // factory, but 13 runs the `IsCurrent` soundness probe and 18 skips it. If 13 dies and
            // 18 is clean, `IsCurrent` is the faulting call; if both die, slot 24 is.
            int[] limits = new int[] { 2, 10, 13, 18, 19, 16, 20, 21, 30, 40, 50, 99 };
            string[] after = new string[]
            {
                "next: create_swapchain_for_panel_traced (QI/GetAdapter/GetParent/Create/attach)",
                "next: enter the swapchain+panel-attach path",
                "next: IDXGIFactory2::IsCurrent (probe ON) then CreateSwapChainForComposition",
                "next: CreateSwapChainForComposition (probe OFF - isolates slot 24 if this dies)",
                "next: CreateSwapChainForComposition on a direct CreateDXGIFactory1 factory",
                "next: IDXGISwapChain::GetBuffer(0) + CreateRenderTargetView",
                "next: ID3D11Device::CreateRenderTargetView",
                "next: build_pipeline (D3DCompile + Create*Shader + CreateInputLayout + buffers/states)",
                "next: create_test_texture (CreateTexture2D + CreateShaderResourceView)",
                "next: RSSetViewports",
                "next: (full start-up complete)",
                "(final)",
            };

            IntPtr native = Marshal.GetIUnknownForObject(panel);
            if (native == IntPtr.Zero)
            {
                AppLog.WriteSync("RustRenderer.Bisect: GetIUnknownForObject returned null");
                return -1;
            }

            int lastOk = -1;
            try
            {
                for (int i = 0; i < limits.Length; i++)
                {
                    int limit = limits[i];
                    AppLog.WriteSync("RustRenderer.Bisect: step limit=" + limit + " -> calling native  ["
                        + after[i] + "]");
                    int rc;
                    try
                    {
                        rc = wp81_rs_start_upto(native, pixelWidth, pixelHeight, limit);
                    }
                    catch (Exception ex)
                    {
                        AppLog.WriteSync("RustRenderer.Bisect: native threw at limit=" + limit, ex);
                        return lastOk;
                    }
                    int reached = -1;
                    try { reached = wp81_rs_reached(); }
                    catch (Exception) { }
                    AppLog.WriteSync("RustRenderer.Bisect: step limit=" + limit + " returned rc=" + rc
                        + " reached=" + reached + " stage=" + Stage
                        + " hr=0x" + LastHr.ToString("X8"));
                    if (rc != 0)
                    {
                        AppLog.WriteSync("RustRenderer.Bisect: step limit=" + limit + " FAILED rc=" + rc
                            + " " + Diagnose());
                        // The step that actually *executes* `CreateSwapChainForComposition` is the
                        // only one that fills in the per-variant HRESULT table, and it is exactly the
                        // step that fails. So log the table HERE, on the failure path, before
                        // returning - otherwise `sc-hr` is read after a step that never tried a
                        // variant (limits 10/13/18 all stop *before* slot 24) and reads all-n/a.
                        if (limit == 16 || limit == 19)
                        {
                            LogVtable();
                        }
                        return lastOk;
                    }
                    lastOk = reached;
                    // The step that builds the swapchain is also where the factory vtable gets
                    // snapshotted. Log it as soon as that step has succeeded, because the NEXT step
                    // is the one that dies and takes the log's tail with it.
                    if (limit == 10 || limit == 13 || limit == 18 || limit == 16 || limit == 19)
                    {
                        LogVtable();
                    }
                    // Each limited run leaves partial state behind; tear it down so the next step
                    // starts clean and eight steps do not leak eight devices.
                    try { wp81_rs_stop(); } catch (Exception) { }
                }

                // Bisecting is done and nothing faulted: now run the real thing in one piece.
                // This is the call that actually produces pixels, so it must be the last one.
                AppLog.WriteSync("RustRenderer.Bisect: bisect clean; running full start-up (limit=0)");
                int rcFull;
                try
                {
                    rcFull = wp81_rs_start_upto(native, pixelWidth, pixelHeight, 0);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("RustRenderer.Bisect: full start-up threw", ex);
                    return lastOk;
                }
                int reachedFull = -1;
                try { reachedFull = wp81_rs_reached(); }
                catch (Exception) { }
                AppLog.WriteSync("RustRenderer.Bisect: full start-up returned rc=" + rcFull
                    + " reached=" + reachedFull + " stage=" + Stage
                    + " hr=0x" + LastHr.ToString("X8")
                    + " swapchain-variant=" + ScVariant + " (" + ScVariantName + ")");
                if (rcFull != 0)
                {
                    AppLog.WriteSync("RustRenderer.Bisect: full start-up FAILED " + Diagnose());
                    return lastOk;
                }
                lastOk = reachedFull;
            }
            finally
            {
                Marshal.Release(native);
            }

            AppLog.WriteSync("RustRenderer.Bisect: all steps completed, reached=" + lastOk);
            _started = true;
            return lastOk;
        }

        /// <summary>
        /// Bring the Rust renderer up on a <see cref="SwapChainPanel"/>.
        ///
        /// The panel is passed as its native <c>IInspectable</c> pointer; Rust does the
        /// <c>QueryInterface(IID_ISwapChainPanelNative)</c> and <c>SetSwapChain</c> itself, so no
        /// interop helper is needed on this side.
        /// </summary>
        public static bool Start(SwapChainPanel panel, uint pixelWidth, uint pixelHeight)
        {
            if (_started)
            {
                return true;
            }
            if (panel == null)
            {
                AppLog.WriteSync("RustRenderer.Start: panel is null");
                return false;
            }

            try
            {
                IntPtr native = Marshal.GetIUnknownForObject(panel);
                if (native == IntPtr.Zero)
                {
                    AppLog.WriteSync("RustRenderer.Start: GetIUnknownForObject returned null");
                    return false;
                }

                int rc;
                // Log BEFORE the native call: if the process dies inside wp81_rs_start this line is
                // the proof of entry, and Rust's progress tick (read by the watchdog) says where.
                AppLog.WriteSync("RustRenderer.Start: calling native wp81_rs_start(0x"
                    + native.ToString("X8") + ", " + pixelWidth + "x" + pixelHeight + ")");
                try
                {
                    rc = wp81_rs_start(native, pixelWidth, pixelHeight);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("RustRenderer.Start: native call threw", ex);
                    return false;
                }
                finally
                {
                    Marshal.Release(native);
                }

                AppLog.WriteSync("RustRenderer.Start: native returned rc=" + rc
                    + " tick=" + Tick + " (" + TickName + ")");

                if (rc != 0)
                {
                    AppLog.WriteSync("RustRenderer.Start: failed rc=" + rc + " " + Diagnose());
                    return false;
                }

                _started = true;
                AppLog.WriteSync("RustRenderer.Start: up. " + pixelWidth + "x" + pixelHeight
                    + " stage=0 tick=" + Tick + " (" + TickName + ")");
                return true;
            }
            catch (Exception ex)
            {
                // A missing DLL, a bad image or an unresolved import surfaces here and names itself.
                AppLog.WriteSync("RustRenderer.Start: FAILED", ex);
                return false;
            }
        }

        /// <summary>
        /// The fallback bring-up route: <c>IDXGIFactory2::CreateSwapChainForCoreWindow</c>.
        ///
        /// This exists because the composition route (<see cref="Bisect"/>) is the single riskiest
        /// unknown on WP8.1, and the plan calls for measuring both. No `SwapChainPanel` is involved -
        /// the swapchain targets the app's one `CoreWindow`, so there is nothing to attach and the
        /// XAML tree is bypassed entirely.
        ///
        /// Must be called on the UI thread, because `CoreWindow.GetForCurrentThread()` is a
        /// thread-affine API. This half only *resolves and probes* the pointer; the native call is
        /// made by <see cref="StartCoreWindowNative"/> on a pool thread, so the UI thread stays free
        /// to keep flushing the log if the native call hard-faults.
        /// </summary>
        public static bool StartCoreWindow(uint pixelWidth, uint pixelHeight)
        {
            IntPtr window = ResolveCoreWindow();
            if (window == IntPtr.Zero)
            {
                return false;
            }
            try
            {
                return StartCoreWindowNative(window, pixelWidth, pixelHeight);
            }
            finally
            {
                ReleaseNativePointer(window);
            }
        }

        /// <summary>Release a raw COM reference obtained from <see cref="ResolveCoreWindow"/>.</summary>
        public static void ReleaseNativePointer(IntPtr p)
        {
            if (p != IntPtr.Zero)
            {
                Marshal.Release(p);
            }
        }

        /// <summary>
        /// Resolve the app's <c>CoreWindow</c> to a raw <c>IUnknown*</c>; **UI thread only**.
        ///
        /// Returns the **ICoreWindow interface pointer**, not the runtime class' default IUnknown.
        /// Microsoft's C++ sample does exactly this:
        ///
        ///     reinterpret_cast<IUnknown*>(m_window.Get())   // m_window is ComPtr<ICoreWindow>
        ///
        /// DXGI QIs the <c>pWindow</c> argument for the CoreWindow's own interface, and a WinRT
        /// runtime-class RCW is not agile, so taking the IUnknown of the *interface-typed* reference
        /// is the form that matches the sample. Returns <see cref="IntPtr.Zero"/> on failure; the
        /// caller owns the returned reference and must release it via
        /// <see cref="ReleaseNativePointer"/>. It also logs the C#-side pointer probe, which is the
        /// last guaranteed flush before the native call.
        /// </summary>
        public static IntPtr ResolveCoreWindow()
        {
            try
            {
                var cw = Windows.UI.Core.CoreWindow.GetForCurrentThread();
                if (cw == null)
                {
                    AppLog.WriteSync("RustRenderer.ResolveCoreWindow: GetForCurrentThread returned null");
                    return IntPtr.Zero;
                }
                // NOTE: this returns the *default* IUnknown of the CoreWindow runtime-class RCW, not
                // a cast-to-interface pointer. The two are the same pointer value in practice (an
                // earlier probe showed class-IUnknown == interface-IUnknown), so the cast was a
                // no-op. Kept un-cast because DXGI's QI inside CreateSwapChainForCoreWindow is what
                // decides, and handing it the object's own IUnknown is the least surprising form.
                IntPtr window = Marshal.GetIUnknownForObject(cw);
                AppLog.WriteSync("RustRenderer.ResolveCoreWindow: IUnknown=0x"
                    + window.ToString("X8"));
                // Log the CoreWindow's observable state: DXGI_ERROR_INVALID_CALL from
                // CreateSwapChainForCoreWindow can mean the window is not yet ready (not visible /
                // not activated), which is plausible during page navigation right after launch.
                try
                {
                    AppLog.WriteSync("RustRenderer.ResolveCoreWindow: state"
                        + " bounds=" + cw.Bounds.Width + "x" + cw.Bounds.Height
                        + " visible=" + cw.Visible);
                }
                catch (Exception exState)
                {
                    AppLog.WriteSync("RustRenderer.ResolveCoreWindow: state read failed", exState);
                }
                LogCoreWindowPointer(window);
                return window;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.ResolveCoreWindow: cannot get CoreWindow", ex);
                return IntPtr.Zero;
            }
        }

        /// <summary>
        /// A second CoreWindow pointer source: <c>Window.Current.CoreWindow</c>. DXGI's
        /// <c>INVALID_CALL</c> persists across every descriptor shape, so the pointer's *origin*
        /// is a suspect; this gives the page an alternative to try when the first one fails.
        /// UI thread only; caller owns the reference.
        /// </summary>
        public static IntPtr ResolveCoreWindowViaWindowCurrent()
        {
            try
            {
                var cw = Windows.UI.Xaml.Window.Current.CoreWindow;
                if (cw == null)
                {
                    AppLog.WriteSync("RustRenderer.ResolveWindowCurrent: Current.CoreWindow null");
                    return IntPtr.Zero;
                }
                IntPtr window = Marshal.GetIUnknownForObject(cw);
                AppLog.WriteSync("RustRenderer.ResolveWindowCurrent: IUnknown=0x"
                    + window.ToString("X8"));
                LogCoreWindowPointer(window);
                return window;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.ResolveWindowCurrent: failed", ex);
                return IntPtr.Zero;
            }
        }

        /// <summary>
        /// The native half of the CoreWindow route; **any thread**. The pointer must already have
        /// been resolved on the UI thread (see <see cref="ResolveCoreWindow"/>).
        /// </summary>
        public static bool StartCoreWindowNative(IntPtr window, uint pixelWidth, uint pixelHeight)
        {
            try
            {
                int rc;
                AppLog.WriteSync("RustRenderer.StartCoreWindow: calling native (0x"
                    + window.ToString("X8") + ", " + pixelWidth + "x" + pixelHeight + ")");
                try
                {
                    rc = wp81_rs_start_corewindow(window, pixelWidth, pixelHeight);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("RustRenderer.StartCoreWindow: native threw", ex);
                    return false;
                }
                AppLog.WriteSync("RustRenderer.StartCoreWindow: native returned rc=" + rc
                    + " " + Diagnose());
                if (rc != 0)
                {
                    return false;
                }
                _started = true;
                return true;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.StartCoreWindow: FAILED", ex);
                return false;
            }
        }

        /// <summary>
        /// Attach the live swapchain to a <see cref="SwapChainPanel"/>. **Must run on the UI thread.**
        ///
        /// Microsoft's own WP8.1 sample performs this attach inside
        /// <c>Dispatcher-&gt;RunAsync(CoreDispatcherPriority::High, …)</c>; touching a XAML object off
        /// the UI thread is not guaranteed to work, so the attach is split out of the native bring-up
        /// (which runs on a pool thread to keep the log flushable) and called from here instead.
        /// Returns true on success.
        /// </summary>
        public static bool AttachPanel(SwapChainPanel panel)
        {
            if (panel == null)
            {
                AppLog.WriteSync("RustRenderer.AttachPanel: panel is null");
                return false;
            }
            IntPtr native = IntPtr.Zero;
            try
            {
                native = Marshal.GetIUnknownForObject(panel);
                if (native == IntPtr.Zero)
                {
                    AppLog.WriteSync("RustRenderer.AttachPanel: GetIUnknownForObject returned null");
                    return false;
                }
                int rc = wp81_rs_attach_panel(native);
                AppLog.WriteSync("RustRenderer.AttachPanel: rc=" + rc
                    + " panel-iface=" + PanelIfaceName);
                return rc == 0;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.AttachPanel: threw", ex);
                return false;
            }
            finally
            {
                if (native != IntPtr.Zero)
                {
                    Marshal.Release(native);
                }
            }
        }

        /// <summary>Draw one frame: clear, one test quad, present. Returns false if not started.</summary>
        public static bool DrawTestFrame()        {
            if (!_started)
            {
                return false;
            }
            wp81_rs_clear(0.10f, 0.11f, 0.13f, 1.0f);
            wp81_rs_draw_test_quad();
            wp81_rs_present();
            return true;
        }

        public static uint LastVertexCount { get { return wp81_rs_vertex_count(); } }

        public static void Stop()
        {
            if (!_started)
            {
                return;
            }
            try
            {
                wp81_rs_stop();
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer.Stop", ex);
            }
            _started = false;
        }

        /// <summary>
        /// One-shot on-device self-test, called from the diagnostics button.
        ///
        /// Stage 1 cannot use the real GamePage yet, but it *can* prove the DLL loads and that the
        /// whole Rust pipeline comes up. Creating no panel means the swapchain step is expected to
        /// fail, so the useful signal is the stage code: 1 or 2 means the device was created.
        /// </summary>
        public static string RunSelfTest()
        {
            string deployed;
            try
            {
                Windows.Storage.StorageFile f = Windows.ApplicationModel.Package.Current
                    .InstalledLocation.GetFileAsync(Dll).AsTask().Result;
                var p = f.GetBasicPropertiesAsync().AsTask().Result;
                deployed = p.Size + " bytes, " + p.DateModified.ToString("HH:mm:ss") + " UTC";
            }
            catch (Exception ex)
            {
                deployed = "(cannot stat: " + ex.Message + ")";
            }

            string result;
            try
            {
                // Query first: a missing DLL names itself here rather than at the first call.
                int stage = wp81_rs_stage();
                result = "loaded, stage=" + stage;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustRenderer self-test: load FAILED", ex);
                return "wp81d3d11rs.dll: LOAD FAILED (" + deployed + ")\n"
                    + ex.GetType().Name + ": " + ex.Message;
            }

            AppLog.WriteSync("RustRenderer self-test: " + result + "  dll=" + deployed);
            return "wp81d3d11rs.dll\n" + deployed + "\n" + result;
        }
    }
}
