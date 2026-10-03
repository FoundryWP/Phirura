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

        private static bool _started;

        public static bool Started { get { return _started; } }

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

        /// <summary>A one-line diagnosis of a failed Start(), for the log and the HUD.</summary>
        public static string Diagnose()
        {
            int stage = Stage;
            int hr = LastHr;
            string s = "stage=" + stage + " hr=0x" + hr.ToString("X8") + " tick=" + Tick + " (" + TickName + ")";
            if (stage >= 100)
            {
                s += "  swapchain-step=" + ScStepName + " (code " + ScStep + ")";
            }
            return s;
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
            if (t == _lastSeenTick)
            {
                return false;
            }
            _lastSeenTick = t;
            _lastSeenTickName = TickName;
            return true;
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
            int[] limits = new int[] { 2, 10, 13, 16, 20, 21, 30, 40, 50, 99 };
            string[] after = new string[]
            {
                "next: create_swapchain_for_panel_traced (QI/GetAdapter/GetParent/Create/attach)",
                "next: enter the swapchain+panel-attach path",
                "next: CreateSwapChainForComposition  <-- the call that has been faulting",
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
                        return lastOk;
                    }
                    lastOk = reached;
                    // The step that builds the swapchain is also where the factory vtable gets
                    // snapshotted. Log it as soon as that step has succeeded, because the NEXT step
                    // is the one that dies and takes the log's tail with it.
                    if (limit == 10 || limit == 13)
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

        /// <summary>Draw one frame: clear, one test quad, present. Returns false if not started.</summary>
        public static bool DrawTestFrame()
        {
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
