using System;
using System.Runtime.InteropServices;
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

        private static bool _started;

        public static bool Started { get { return _started; } }

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
                int rc;
                try
                {
                    rc = wp81_rs_start(native, pixelWidth, pixelHeight);
                }
                finally
                {
                    Marshal.Release(native);
                }

                if (rc != 0)
                {
                    AppLog.WriteSync("RustRenderer.Start: failed rc=" + rc
                        + " stage=" + wp81_rs_stage()
                        + " hr=0x" + wp81_rs_last_hr().ToString("X8"));
                    return false;
                }

                _started = true;
                AppLog.WriteSync("RustRenderer.Start: up. " + pixelWidth + "x" + pixelHeight
                    + " stage=0");
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
