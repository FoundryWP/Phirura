using System;
using System.Runtime.InteropServices;

namespace Phirura
{
    /// <summary>
    /// Asks the device whether D3D11 actually works for our purposes.
    ///
    /// Win2D already proves the phone has a working D3D11 stack — the chart is rendered through
    /// it. But Win2D is a black box, and a `miniquad` backend would be a hand-written D3D11
    /// renderer: it has to create a device, allocate a render target, draw into it and read the
    /// result back. That is a different question, and it is the one that decides whether the Rust
    /// path is viable.
    ///
    /// `rust-wp81/d3d11-probe/d3d11probe.cpp` answers it entirely offscreen — no swap chain, no
    /// XAML — so a failure cannot be blamed on the presentation layer. It returns a stage number
    /// (0 = success) and fills in a report naming the adapter, the feature level and the pixel it
    /// read back.
    /// </summary>
    public static class D3D11Probe
    {
        // `__stdcall` on ARM is the AAPCS, which is also what Cdecl denotes there — the two are
        // indistinguishable on this architecture.
        [DllImport("wp81d3d11.dll", CallingConvention = CallingConvention.StdCall)]
        private static extern int wp81_d3d11_probe();

        [DllImport("wp81d3d11.dll", CallingConvention = CallingConvention.StdCall)]
        private static extern IntPtr wp81_d3d11_report();

        public static void Run()
        {
            string summary;
            try
            {
                AppLog.WriteSync("D3D11Probe: deployed wp81d3d11.dll = " + DescribeDll("wp81d3d11.dll"));

                int stage = wp81_d3d11_probe();
                string report = ReadUtf16(wp81_d3d11_report());

                AppLog.WriteSync("D3D11Probe: stage=" + stage + "  " + report);

                summary = (stage == 0 ? "D3D11: OK" : "D3D11: FAILED at stage " + stage)
                    + "\n" + report;
            }
            catch (Exception ex)
            {
                // A missing DLL, a bad image or a missing import surfaces here and names itself.
                AppLog.WriteSync("D3D11Probe: FAILED", ex);
                summary = "D3D11: FAILED\n" + ex.GetType().Name + ": " + ex.Message;
            }

            ShowOnScreen(summary);
        }

        private static void ShowOnScreen(string text)
        {
            try
            {
                Windows.UI.Popups.MessageDialog dlg = new Windows.UI.Popups.MessageDialog(
                    text, "D3D11 probe");
#pragma warning disable 4014
                dlg.ShowAsync();
#pragma warning restore 4014
            }
            catch (Exception)
            {
            }
        }

        /// <summary>Size and timestamp of the deployed DLL, so a stale deployment cannot be mistaken for a failure.</summary>
        private static string DescribeDll(string fileName)
        {
            try
            {
                Windows.Storage.StorageFolder dir = Windows.ApplicationModel.Package.Current.InstalledLocation;
                Windows.Storage.StorageFile f = dir.GetFileAsync(fileName).AsTask().Result;
                Windows.Storage.FileProperties.BasicProperties p =
                    f.GetBasicPropertiesAsync().AsTask().Result;
                return p.Size + " bytes, modified " + p.DateModified.ToString("HH:mm:ss") + " UTC";
            }
            catch (Exception ex)
            {
                return "(cannot stat: " + ex.Message + ")";
            }
        }

        private static string ReadUtf16(IntPtr p)
        {
            if (p == IntPtr.Zero) return "(null)";
            System.Text.StringBuilder sb = new System.Text.StringBuilder();
            for (int i = 0; i < 1024; i++)
            {
                short c = Marshal.ReadInt16(p, i * 2);
                if (c == 0) break;
                sb.Append((char)c);
            }
            return sb.ToString();
        }
    }
}
