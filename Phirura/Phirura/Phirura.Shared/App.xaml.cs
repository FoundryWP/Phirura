using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading.Tasks;
using Windows.ApplicationModel;
using Windows.ApplicationModel.Activation;
using Windows.Foundation;
using Windows.Foundation.Collections;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Controls.Primitives;
using Windows.UI.Xaml.Data;
using Windows.UI.Xaml.Input;
using Windows.UI.Xaml.Media;
using Windows.UI.Xaml.Media.Animation;
using Windows.UI.Xaml.Navigation;

// The Blank Application template is documented at http://go.microsoft.com/fwlink/?LinkId=234227

namespace Phirura
{
    /// <summary>
    /// Provides application-specific behavior to supplement the default Application class.
    /// </summary>
    public sealed partial class App : Application
    {
#if WINDOWS_PHONE_APP
        private TransitionCollection transitions;

        /// <summary>
        /// Enables the native toolchain diagnostics at startup. See the note in the constructor -
        /// this must stay false in anything meant to be installed, because the Rust probe aborts
        /// the process on purpose.
        /// </summary>
        private static readonly bool RunNativeProbes = false;
#endif

        /// <summary>
        /// Initializes the singleton application object.  This is the first line of authored code
        /// executed, and as such is the logical equivalent of main() or WinMain().
        /// </summary>
        public App()
        {
            this.InitializeComponent();
            this.Suspending += this.OnSuspending;

            // Without this, an exception in a UI handler kills the process with no trace
            // on a phone (Debug.WriteLine never reaches a file). Handled stays false so
            // the failure is not swallowed and hidden behind a half-broken UI.
            this.UnhandledException += OnUnhandledException;
            TaskScheduler.UnobservedTaskException += OnUnobservedTaskException;

            AppLog.Write("=== Phirura launched ===");

#if WINDOWS_PHONE_APP
            // Native toolchain diagnostics, OFF by default.
            //
            // These are how the Rust-on-WP8.1 workstream (see rust-wp81/) gets evidence off a real
            // device, and they must stay off in anything anyone is meant to install: the Rust probe
            // deliberately walks steps that are *known* to abort the process, because that is how it
            // localises a failure. With it enabled the app dies during startup, every launch.
            //
            // Flipping this to true re-enables both. D3D11 runs first on purpose - the Rust probe
            // kills the process, so anything scheduled after it may never run, and the D3D11 result
            // is what decides whether the Rust renderer path is viable.
            //
            // `static readonly`, not `const`: a const false makes the whole block unreachable and
            // the compiler says so (CS0162), which is noise in every build.
            // The Rust prpr port (rust-wp81/prpr-port).
            //
            // Install() is cheap and must happen before anything calls into the port: the shims'
            // audio and file-dialog entry points forward through the table it hands over, so a late
            // install means the first such call finds nothing. Done unconditionally, because unlike
            // the probes it cannot abort the process.
            try
            {
                PrprPort.Install();
                AppLog.WriteSync("PrprPort: callbacks installed at startup");
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("PrprPort: install failed", ex);
            }

            if (RunNativeProbes)
            {
                D3D11Probe.Run();
                NativeProbe.Run();

                // Proves the real Rust chart parser end to end. The chart is inline rather than an
                // asset so the test does not depend on file IO or on which charts happen to be
                // installed. Three notes at 120 BPM: a non-zero answer means the PEC parser, the
                // chart model, nalgebra and Chart::finalize all ran on the device.
                PrprPort.RunSelfTest(
                    "0" + Environment.NewLine +
                    "bp 0.000 120.000" + Environment.NewLine +
                    "n1 0 1.000 512.00 1 0" + Environment.NewLine +
                    "n1 0 2.000 512.00 1 0" + Environment.NewLine +
                    "n1 0 3.000 512.00 1 0" + Environment.NewLine);
            }
#endif
        }

        // Fully qualified: System.UnhandledExceptionEventArgs also exists and would be
        // ambiguous under `using System;` + `using Windows.UI.Xaml;`.
        private void OnUnhandledException(object sender, Windows.UI.Xaml.UnhandledExceptionEventArgs e)
        {
            AppLog.Write("UNHANDLED", e.Exception);
        }

        private void OnUnobservedTaskException(object sender, System.Threading.Tasks.UnobservedTaskExceptionEventArgs e)
        {
            AppLog.Write("UNOBSERVED TASK", e.Exception);
        }

        /// <summary>
        /// Invoked when the application is launched normally by the end user.  Other entry points
        /// will be used when the application is launched to open a specific file, to display
        /// search results, and so forth.
        /// </summary>
        /// <param name="e">Details about the launch request and process.</param>
        protected override void OnLaunched(LaunchActivatedEventArgs e)
        {
#if DEBUG
            if (System.Diagnostics.Debugger.IsAttached)
            {
                this.DebugSettings.EnableFrameRateCounter = true;
            }
#endif

            Frame rootFrame = Window.Current.Content as Frame;

            // Do not repeat app initialization when the Window already has content,
            // just ensure that the window is active
            if (rootFrame == null)
            {
                // Create a Frame to act as the navigation context and navigate to the first page
                rootFrame = new Frame();

                // TODO: change this value to a cache size that is appropriate for your application
                rootFrame.CacheSize = 1;

                if (e.PreviousExecutionState == ApplicationExecutionState.Terminated)
                {
                    // TODO: Load state from previously suspended application
                }

                // Place the frame in the current Window
                Window.Current.Content = rootFrame;
            }

            if (rootFrame.Content == null)
            {
#if WINDOWS_PHONE_APP
                // Removes the turnstile navigation for startup.
                if (rootFrame.ContentTransitions != null)
                {
                    this.transitions = new TransitionCollection();
                    foreach (var c in rootFrame.ContentTransitions)
                    {
                        this.transitions.Add(c);
                    }
                }

                rootFrame.ContentTransitions = null;
                rootFrame.Navigated += this.RootFrame_FirstNavigated;
#endif

                // When the navigation stack isn't restored navigate to the first page,
                // configuring the new page by passing required information as a navigation
                // parameter
                //
                // The Stage 1 shortcut that used to land straight on `RustStage1Page` is gone:
                // Stage 1 has passed acceptance on device (60 fps, 154 commands per frame, 76
                // textures - the same numbers the desktop harness reports), and while it was in
                // place every redeploy put the acceptance harness on screen instead of the chart
                // list, so a run could never reach `GamePage` without a manual back-press. The
                // harness stays reachable from `MainPage`, which is where a redeploy should start
                // now that the thing being exercised is chart playback rather than the renderer.
                if (!rootFrame.Navigate(typeof(MainPage), e.Arguments))
                {
                    throw new Exception("Failed to create initial page");
                }
            }

            // Ensure the current window is active
            Window.Current.Activate();
        }

#if WINDOWS_PHONE_APP
        /// <summary>
        /// Restores the content transitions after the app has launched.
        /// </summary>
        /// <param name="sender">The object where the handler is attached.</param>
        /// <param name="e">Details about the navigation event.</param>
        private void RootFrame_FirstNavigated(object sender, NavigationEventArgs e)
        {
            var rootFrame = sender as Frame;
            rootFrame.ContentTransitions = this.transitions ?? new TransitionCollection() { new NavigationThemeTransition() };
            rootFrame.Navigated -= this.RootFrame_FirstNavigated;
        }
#endif

        /// <summary>
        /// Invoked when application execution is being suspended.  Application state is saved
        /// without knowing whether the application will be terminated or resumed with the contents
        /// of memory still intact.
        /// </summary>
        /// <param name="sender">The source of the suspend request.</param>
        /// <param name="e">Details about the suspend request.</param>
        private void OnSuspending(object sender, SuspendingEventArgs e)
        {
            var deferral = e.SuspendingOperation.GetDeferral();

            // TODO: Save application state and stop any background activity
            deferral.Complete();
        }
    }
}