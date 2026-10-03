using System;
using System.Runtime.InteropServices;

namespace Phirura
{
    /// <summary>
    /// Proves the Rust toolchain can reach the device.
    ///
    /// `rust/wp81probe.dll` is a real ARMNT binary built from Rust — `std` compiled from source
    /// for the custom `thumbv7a-windows-phone` target, plus `wp81-compat`, which supplies the
    /// 19 Win32 entry points WP8.1 does not have (see `rust-wp81/` in the workspace root).
    /// It is linked by the VC11 ARM linker against WindowsPhoneCore.lib, and its import table
    /// contains only WP8.1 API-set names.
    ///
    /// The probe function exercises the parts of `std` that matter: HashMap/String/Vec, file
    /// I/O (through the NtCreateFile/NtReadFile shims), thread spawning, the monotonic clock
    /// and OS randomness. If it returns a number, the whole chain works end to end; if the DLL
    /// fails to load, the exception text says so.
    ///
    /// Everything is logged rather than shown, so the result survives even if the UI never
    /// gets that far — `ApplicationData\LocalFolder\phirura.log`.
    /// </summary>
    public static class NativeProbe
    {
        // Rust's `extern "C"` maps to the ARM AAPCS, which is what Cdecl denotes here.
        [DllImport("wp81probe.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern long wp81_link_probe(uint seed);

        /// <summary>
        /// How many API-set functions the Rust side failed to resolve at run time, plus the
        /// first failure. This is the whole point of resolving them lazily instead of importing
        /// them: a wrong API set used to make the DLL refuse to load with ERROR_PROC_NOT_FOUND,
        /// naming no symbol. Now it is a count and a name we can read.
        /// </summary>
        [DllImport("wp81probe.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_api_failures(out IntPtr dllName, out IntPtr fnName,
                                                     out uint kind);

        /// <summary>
        /// Walks every (API set, function) pair the Rust side would use and reports the ones that
        /// do not resolve — without calling any of them.
        ///
        /// Called BEFORE wp81_link_probe on purpose: the last run got as far as loading the DLL and
        /// then died with 0xC0000409 (a Rust abort), which means the first *call* is what crashes.
        /// A lazy resolver can never report anything in that situation, so this walks the table
        /// first and the answer is in the log even if the next call takes the process down.
        /// </summary>
        [DllImport("wp81probe.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_probe_api_sets(out IntPtr dllName, out IntPtr fnName);

        /// <summary>
        /// Where the Rust side should append panic messages. It has no idea where the app's
        /// LocalFolder is, so it has to be told.
        /// </summary>
        [DllImport("wp81probe.dll", CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Unicode)]
        private static extern void wp81_set_log_path(string path);

        /// <summary>
        /// Installs a panic hook that writes to that path.
        ///
        /// This is the difference between "the process died with 0xC0000409" and knowing why. A
        /// Rust panic with <c>panic = "abort"</c> prints nothing at all - the default hook writes
        /// to stderr, and a WP8.1 app has no stderr. The hook *is* still called before the abort,
        /// which is what makes this work.
        /// </summary>
        [DllImport("wp81probe.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_install_panic_hook();

        public static void Run()
        {
            string summary;
            try
            {
                // Log what is ACTUALLY deployed before calling it. VS deploys incrementally into
                // its own AppxLayouts folder, so "did my rebuild actually reach the device?" is a
                // real question and the answer has to be evidence, not assumption.
                string deployed = DescribeDll();
                AppLog.WriteSync("NativeProbe: deployed wp81probe.dll = " + deployed);

                // Resolve-only pass, before anything is executed.
                IntPtr badDll, badFn;
                uint bad = wp81_probe_api_sets(out badDll, out badFn);
                AppLog.WriteSync("NativeProbe: API-set resolution = " + bad + " failure(s)"
                    + (bad == 0 ? "" : ", first: " + ReadUtf16(badDll) + " / " + ReadAnsi(badFn)));

                AppLog.WriteSync("NativeProbe: calling wp81_link_probe ...");

                // Point the Rust panic hook at this same log, and install it. Doing this BEFORE
                // the walk is the whole point: a panic during a step would otherwise abort the
                // process silently and leave no trace at all.
                try
                {
                    string logPath = System.IO.Path.Combine(
                        Windows.Storage.ApplicationData.Current.LocalFolder.Path, "phirura.log");
                    wp81_set_log_path(logPath);
                    wp81_install_panic_hook();
                    AppLog.WriteSync("NativeProbe: panic hook installed -> " + logPath);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("NativeProbe: panic hook setup failed", ex);
                }

                // Walk the steps one at a time and log after each. The last step that logged
                // before the process died is the culprit.
                //
                // ORDER MATTERS, and so does WriteSync. Write() only queues the append, so an
                // abrupt process death used to lose the final lines - which is exactly why an
                // earlier run showed steps 0-4 and 7 but not 5 or 6. Every line here is on disk
                // before the next step starts, and the fatal steps go last so the missing line
                // is the one we care about.
                uint[] order = { 0, 1, 2, 3, 4, 7, 8, 14, 17, 18, 20, 24, 19, 21, 22, 23, 15, 16, 6, 9, 10, 11, 12, 13, 5 };
                string[] names = {
                    "enter",                                  // 0
                    "alloc (HashMap/String/Vec)",             // 1
                    "raw QPC",                                // 2
                    "file I/O (Nt shims)",                    // 3
                    "randomness (ProcessPrng)",               // 4
                    "threads (thread::spawn + join)",         // 5  <-- last
                    "Instant::now().elapsed().as_millis()",   // 6
                    "raw CreateThread",                       // 7
                    "env (RUST_MIN_STACK)",                   // 8
                    "std CreateThread args, routine returns", // 9
                    "new thread: lazy resolver",              // 10
                    "new thread: SetThreadStackGuarantee",    // 11
                    "new thread: TLS",                        // 12
                    "new thread: 8 KiB stack frame",          // 13
                    "QueryPerformanceFrequency (measured)",   // 14
                    "Instant::now() only",                    // 15
                    "Instant::now().elapsed()",               // 16
                    "Duration::from_secs(1).as_millis()",     // 17
                    "u64 division  (expect 137759)",          // 18
                    "u64 remainder (expect 3879449)",         // 19
                    "u32 division  (expect 571428571)",       // 20
                    "QPC trampoline (std's own path)",        // 21
                    "mul_div_u64 replica (expect 138303315018222)", // 22
                    "Duration::from_nanos(huge) (expect 4611686018)", // 23
                    "u32 remainder (expect 6)"                // 24
                };
                long last = -2;
                int lastStep = -1;
                for (int i = 0; i < order.Length; i++)
                {
                    uint step = order[i];
                    long v = wp81_link_probe(step);
                    AppLog.WriteSync("NativeProbe: step " + step + " (" + names[step]
                        + ") returned " + v);
                    last = v;
                    lastStep = (int)step;
                }

                summary = "all steps completed\nlast step " + lastStep + " returned " + last;
            }
            catch (Exception ex)
            {
                // A missing or unloadable DLL surfaces here as DllNotFoundException /
                // BadImageFormatException, which is exactly the signal we want.
                AppLog.WriteSync("NativeProbe: FAILED", ex);
                summary = "FAILED\n" + ex.GetType().Name + ": " + ex.Message;
            }

            // Also put it on screen: a sideloaded build has no debugger attached, and reading
            // ApplicationData\LocalFolder needs tooling. The dialog costs one tap and removes the
            // need for either.
            ShowOnScreen(summary);
        }

        private static void ShowOnScreen(string text)
        {
            try
            {
                Windows.UI.Popups.MessageDialog dlg = new Windows.UI.Popups.MessageDialog(
                    text, "Rust native probe");
#pragma warning disable 4014
                dlg.ShowAsync();
#pragma warning restore 4014
            }
            catch (Exception)
            {
            }
        }

        /// <summary>
        /// Size and timestamp of the deployed DLL, from the package's install folder. This is how
        /// we tell a genuinely stale deployment from a real failure.
        /// </summary>
        private static string DescribeDll()
        {
            try
            {
                Windows.Storage.StorageFolder dir = Windows.ApplicationModel.Package.Current.InstalledLocation;
                Windows.Storage.StorageFile f =
                    dir.GetFileAsync("wp81probe.dll").AsTask().Result;
                Windows.Storage.FileProperties.BasicProperties p =
                    f.GetBasicPropertiesAsync().AsTask().Result;
                return p.Size + " bytes, modified " + p.DateModified.ToString("HH:mm:ss")
                    + " UTC (" + f.Path + ")";
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
            for (int i = 0; i < 128; i++)
            {
                short c = System.Runtime.InteropServices.Marshal.ReadInt16(p, i * 2);
                if (c == 0) break;
                sb.Append((char)c);
            }
            return sb.ToString();
        }

        private static string ReadAnsi(IntPtr p)
        {
            if (p == IntPtr.Zero) return "(null)";
            System.Text.StringBuilder sb = new System.Text.StringBuilder();
            for (int i = 0; i < 128; i++)
            {
                byte c = System.Runtime.InteropServices.Marshal.ReadByte(p, i);
                if (c == 0) break;
                sb.Append((char)c);
            }
            return sb.ToString();
        }
    }
}
