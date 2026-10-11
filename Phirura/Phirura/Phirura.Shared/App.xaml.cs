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
            // Paired with Suspending, on purpose. A log that simply stops is otherwise
            // indistinguishable from a crash, and port 8765 going quiet is otherwise
            // indistinguishable from the app dying - when the usual cause is the phone
            // locking its screen and the socket stopping accepting. These two lines are
            // what make "interrupted" and "faulted" different readings of the same file.
            this.Resuming += OnResuming;

            // Without this, an exception in a UI handler kills the process with no trace
            // on a phone (Debug.WriteLine never reaches a file). Handled stays false so
            // the failure is not swallowed and hidden behind a half-broken UI.
            this.UnhandledException += OnUnhandledException;
            TaskScheduler.UnobservedTaskException += OnUnobservedTaskException;

            AppLog.Write("=== Phirura launched ===");

#if WINDOWS_PHONE_APP
            // Started before anything else that can fail, on purpose. The log server is the
            // channel you reach for when the app is wedged or silent, and a channel opened after
            // the wedge is exactly as useful as no channel. It never throws - Start() is
            // idempotent and reports its own failure through AppLog - and it binds
            // asynchronously, so it cannot delay or hang the launch either.
            LogServer.Start();
#endif

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
            // `WriteSync`, not `Write`. `Write` queues the append onto the thread pool, and a
            // queued continuation is precisely what never runs when the next thing to happen is
            // the process dying - so the one message worth having was the one that got dropped.
            // Handled stays false (see the constructor), so this exception does still take the
            // process down: this line is the last one, and it has to be on disk already.
            // `e.Handled` rides along because "the framework kept the app alive despite this"
            // and "the app stopped right here" read the same in a log that simply ends.
            AppLog.WriteSync("UNHANDLED handled=" + e.Handled, e.Exception);
        }

        private void OnUnobservedTaskException(object sender, System.Threading.Tasks.UnobservedTaskExceptionEventArgs e)
        {
            // Same reason as above: this fires on the way down, where the queue has no thread
            // left to drain it.
            AppLog.WriteSync("UNOBSERVED TASK", e.Exception);
        }

        /// <summary>
        /// Invoked when the application is activated by some means other than normal launching:
        /// file-picker continuations, mostly. Routes those to the page; everything else falls
        /// through to the default handling.
        /// </summary>
        protected override void OnActivated(IActivatedEventArgs e)
        {
            Windows.ApplicationModel.Activation.FileOpenPickerContinuationEventArgs cont =
                e as Windows.ApplicationModel.Activation.FileOpenPickerContinuationEventArgs;
            if (cont != null)
            {
                Frame rootFrame = Window.Current.Content as Frame;
                if (rootFrame == null)
                {
                    rootFrame = new Frame();
                    Window.Current.Content = rootFrame;
                }
                if (rootFrame.Content == null)
                {
                    rootFrame.Navigate(typeof(GamePage));
                }
                GamePage.FilePickerContinuation(cont.Files);
                Window.Current.Activate();
                return;
            }
            base.OnActivated(e);
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

                // First page. On Windows Phone that is `GamePage` booting straight into Rust
                // phira's own front end - not the chart list.
                //
                // `MainPage.PendingChart` is what GamePage reads to decide what to play, and null
                // is the only signal it has for "play nothing, run phira instead": a cold start
                // and "nobody picked anything" are indistinguishable from outside. It is assigned
                // here rather than left to the field default because a value left behind by an
                // earlier navigation in the same process would silently launch into a chart nobody
                // chose.
                //
                // The list is still reachable - GamePage's back button goes back to it - which is
                // also where the Stage 1 acceptance harness is linked from.
                Type firstPage = typeof(MainPage);
#if WINDOWS_PHONE_APP
                MainPage.PendingChart = null;
                firstPage = typeof(GamePage);
#endif
                if (!rootFrame.Navigate(firstPage, e.Arguments))
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

            // Sync, because termination can follow immediately and there may be no thread pool
            // left to drain a queued append. This is the line that says the session was
            // interrupted rather than crashed.
            AppLog.WriteSync("=== suspending ===");
            deferral.Complete();
        }

        /// <summary>
        /// The other half of <see cref="OnSuspending"/>.
        ///
        /// The listener is deliberately *not* restarted here. `LogServer.Start()` is guarded by
        /// an interlocked flag and would return immediately, so calling it would only look like
        /// a repair that did not happen; the honest thing is to report the state the original
        /// bind is actually in, which is what the line below does.
        /// </summary>
        private void OnResuming(object sender, object e)
        {
            AppLog.WriteSync("=== resumed === logServer=" + LogServer.State);
        }
    }
}