using System;
using System.Diagnostics;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media;
using Windows.UI.Xaml.Navigation;

namespace Phirura
{
    /// <summary>
    /// Path A, Stage 1 acceptance test: a <b>Rust-rendered</b> textured quad on a real phone.
    ///
    /// The plan's ordering rule is explicit — Stage 3 (porting prpr's scene) must not begin until
    /// Stage 1 draws a textured quad on a real device, because swapchain attachment is the riskiest
    /// unknown in the whole port. This page is that gate.
    ///
    /// What C# does here is deliberately minimal, so that "it renders" cannot be attributed to C#:
    ///
    ///   1. hand Rust the <see cref="SwapChainPanel"/>'s native pointer and the pixel size;
    ///   2. call clear / draw / present once per frame from <c>CompositionTarget.Rendering</c>;
    ///   3. write the numbers Rust reports (stage, HRESULT, vertex count) into TextBlocks.
    ///
    /// Rust owns the device, the swapchain, the shaders and the draw calls.
    /// </summary>
    public sealed partial class RustStage1Page : Page
    {
        private readonly Stopwatch _clock = new Stopwatch();
        private int _frames;
        private bool _hookAttached;
        private Windows.UI.Xaml.DispatcherTimer _watchdog;

        /// <summary>
        /// Guards <see cref="StartRenderer"/> against re-entry.
        ///
        /// <c>OnNavigatedTo</c> can fire more than once for the same page, and <c>StartRenderer</c>
        /// is <c>async void</c>, so without this the pool-thread bring-ups overlap. They then race
        /// on Rust's single global <c>STATE</c>: each run calls <c>release_all()</c> first, which
        /// frees the <em>other</em> run's device while it is still using it. The symptom was a hard
        /// fault inside a DXGI call that looked exactly like a driver bug - and moving a probe
        /// around appeared to "fix" it, because it changed the timing. It was a use-after-free.
        /// </summary>
        private int _startGate;

        public RustStage1Page()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Disabled;
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            // Wait for the panel to have a measured size before creating a swapchain against it.
            // -= before += so a second navigation cannot subscribe twice.
            this.Loaded -= OnPageLoaded;
            this.Loaded += OnPageLoaded;
            // A low-frequency sampler: it reads Rust's native progress counter and keeps the value
            // in managed memory. If a native call hard-faults the process, the log's last line is
            // whatever this last saw - the only crash localisation available without a debugger.
            _watchdog = new Windows.UI.Xaml.DispatcherTimer();
            _watchdog.Interval = TimeSpan.FromMilliseconds(200);
            _watchdog.Tick += OnWatchdogTick;
            _watchdog.Start();

            // The DispatcherTimer above runs on the UI thread, but the CoreWindow bring-up is a
            // *synchronous* native call made **from** the UI thread - so while it is stuck inside
            // native, the DispatcherTimer never ticks and a crash there leaves no trace at all.
            // (That is exactly what hid the CoreWindow failure for several runs.) A thread-pool
            // timer is independent of the UI thread, so it keeps sampling and its last write is
            // the crash's address.
            _bgWatchdog = new System.Threading.Timer(OnBackgroundWatchdog, null,
                TimeSpan.FromMilliseconds(150), TimeSpan.FromMilliseconds(150));
        }

        private System.Threading.Timer _bgWatchdog;

        private void OnBackgroundWatchdog(object state)
        {
            try
            {
                // SampleTick() returns true only when the tick *changed*, so a hang shows as
                // exactly one line for the stuck tick - which is what we want (it names the call
                // that never returned). This works because the CoreWindow bring-up runs on a pool
                // thread, leaving the UI thread free to keep the AppLog FileIO path alive.
                if (RustRenderer.SampleTick())
                {
                    AppLog.WriteRaw("RustStage1: [bg] tick -> " + RustRenderer.LastSeenTick
                        + " (" + RustRenderer.LastSeenTickName + ")");
                }
            }
            catch (Exception)
            {
                // The timer must never take the process down on its own.
            }
        }

        private void OnWatchdogTick(object sender, object e)
        {
            if (RustRenderer.SampleTick())
            {
                AppLog.WriteSync("RustStage1: tick -> " + RustRenderer.LastSeenTick
                    + " (" + RustRenderer.LastSeenTickName + ")");
            }
        }

        protected override void OnNavigatedFrom(NavigationEventArgs e)
        {
            base.OnNavigatedFrom(e);
            TearDown();
        }

        private void OnPageLoaded(object sender, RoutedEventArgs e)
        {
            this.Loaded -= OnPageLoaded;
            AppLog.WriteSync("RustStage1: OnPageLoaded; ActualWidth=" + RustPanel.ActualWidth
                + " ActualHeight=" + RustPanel.ActualHeight);
            // The panel must be laid out before its pixel size is meaningful.
            // On WP8.1 CoreDispatcher has RunAsync, not BeginInvoke.
            var ignore = this.Dispatcher.RunAsync(Windows.UI.Core.CoreDispatcherPriority.Normal,
                () => StartRenderer());
        }

        private async void StartRenderer()
        {
            // Re-entrancy gate. Two overlapping bring-ups corrupt Rust's global STATE (see
            // _startGate); only one may run at a time, and a second nav must not restart a live one.
            if (System.Threading.Interlocked.CompareExchange(ref _startGate, 1, 0) != 0)
            {
                AppLog.WriteSync("RustStage1: StartRenderer re-entry blocked (already running)");
                return;
            }
            try
            {
                AppLog.WriteSync("RustStage1: StartRenderer entered");
                // One more layout pass: the first frame after navigation can report 0x0.
                await System.Threading.Tasks.Task.Delay(60);

                if (RustRenderer.Started)
                {
                    AppLog.WriteSync("RustStage1: already started, skipping");
                    return;
                }

                double scale = 1.0;
                try { scale = Windows.Graphics.Display.DisplayInformation.GetForCurrentView().RawPixelsPerViewPixel; }
                catch (Exception ex) { AppLog.WriteSync("RustStage1: RawPixelsPerViewPixel", ex); }

                uint w = (uint)Math.Max(1.0, Math.Round(RustPanel.ActualWidth * scale));
                uint h = (uint)Math.Max(1.0, Math.Round(RustPanel.ActualHeight * scale));
                AppLog.WriteSync("RustStage1: panel " + RustPanel.ActualWidth + "x" + RustPanel.ActualHeight
                    + " -> " + w + "x" + h + " px @ " + scale.ToString("0.0"));

                // Run the native bring-up on a pool thread so the UI thread stays alive to flush the
                // log if a native step hard-faults. Each step is logged before and after, so the last
                // line in phirura.log names the call that killed the process.
                AppLog.WriteSync("RustStage1: starting bisect on pool thread");
                int reached = -1;
                await System.Threading.Tasks.Task.Run(delegate
                {
                    reached = RustRenderer.Bisect(RustPanel, w, h);
                });

                bool ok = RustRenderer.Started;
                AppLog.WriteSync("RustStage1: bisect finished, reached=" + reached + " started=" + ok);

                // Route 2: if the composition swapchain could not be created, fall back to the
                // CoreWindow route before giving up. It bypasses XAML entirely, so if the failure is
                // in the panel attach rather than in DXGI this still produces pixels. Must run on
                // the UI thread - CoreWindow.GetForCurrentThread is thread-affine.
                string route = "composition";
                if (ok)
                {
                    // The attach is the UI thread's job (see RustRenderer.AttachPanel); the native
                    // bring-up above ran on a pool thread, so it deliberately did not attach.
                    bool attached = RustRenderer.AttachPanel(RustPanel);
                    AppLog.WriteSync("RustStage1: UI-thread attach -> " + attached);
                    if (!attached)
                    {
                        AppLog.WriteSync("RustStage1: composition create ok but attach failed");
                    }
                }
                else
                {
                    AppLog.WriteSync("RustStage1: composition route failed; trying CoreWindow route");
                    // Resolve the CoreWindow on the UI thread (thread-affine API).
                    //
                    // The native bring-up now runs **on the UI thread**. It used to run on a pool
                    // thread to keep the log flushable, but the real problem that made the pool
                    // thread necessary was a hanging `Release()` inside the old pWindow probe; that
                    // probe is gone, so the call returns promptly. And `CreateSwapChainForCoreWindow`
                    // is documented to be UI-thread work - running it on a pool thread is a strong
                    // candidate for the DXGI_ERROR_INVALID_CALL we still see.
                    IntPtr cwPtr = RustRenderer.ResolveCoreWindow();
                    if (cwPtr == IntPtr.Zero)
                    {
                        ok = false;
                    }
                    else
                    {
                        try
                        {
                            ok = RustRenderer.StartCoreWindowNative(cwPtr, w, h);
                        }
                        finally
                        {
                            RustRenderer.ReleaseNativePointer(cwPtr);
                        }
                    }
                    AppLog.WriteSync("RustStage1: CoreWindow route started=" + ok + " " + RustRenderer.Diagnose());
                    if (ok)
                    {
                        route = "corewindow";
                    }
                }

                StageText.Text = ok ? "启动成功 · stage=0" : "启动失败 · " + RustRenderer.Diagnose();
                HrText.Text = ok
                    ? "route: " + route + (route == "composition" ? " · " + RustRenderer.ScVariantName : "")
                    : (RustRenderer.ScStep > 0
                        ? "swapchain 步: " + RustRenderer.ScStepName + "  hr=0x" + RustRenderer.LastHr.ToString("X8")
                          + (RustRenderer.LastHrName.Length > 0 ? " (" + RustRenderer.LastHrName + ")" : "")
                        : "");
                SizeText.Text = w + " x " + h + " px  (scale " + scale.ToString("0.00") + ")";
                if (!ok)
                {
                    // Do not attach the frame loop to a renderer that never came up.
                    return;
                }

                _clock.Start();
                _frames = 0;
                if (!_hookAttached)
                {
                    CompositionTarget.Rendering += OnFrame;
                    _hookAttached = true;
                }
            }
            finally
            {
                System.Threading.Interlocked.Exchange(ref _startGate, 0);
            }
        }

        /// <summary>
        /// One frame. Clear -> one test quad -> present, all inside Rust.
        ///
        /// Kept on the UI thread on purpose: a swapchain attached to a <see cref="SwapChainPanel"/>
        /// is presented by the XAML compositor, and on WP8.1 the panel interaction is not free-
        /// threaded the way a desktop HWND swapchain is.
        /// </summary>
        private void OnFrame(object sender, object e)
        {
            if (!RustRenderer.Started)
            {
                return;
            }

            if (!RustRenderer.DrawTestFrame())
            {
                return;
            }

            _frames++;
            VertsText.Text = "vertices = " + RustRenderer.LastVertexCount;

            if (_clock.ElapsedMilliseconds >= 1000)
            {
                double fps = _frames * 1000.0 / _clock.ElapsedMilliseconds;
                FpsText.Text = fps.ToString("0.0") + " fps";
                _clock.Restart();
                _frames = 0;
            }
        }

        private void RestartButton_Click(object sender, RoutedEventArgs e)
        {
            TearDown();
            StartRenderer();
        }

        private void StopButton_Click(object sender, RoutedEventArgs e)
        {
            TearDown();
            StageText.Text = "已停止";
            HrText.Text = "";
            VertsText.Text = "";
            FpsText.Text = "";
        }

        private void BackButton_Click(object sender, RoutedEventArgs e)
        {
            TearDown();
            if (this.Frame.CanGoBack)
            {
                this.Frame.GoBack();
            }
        }

        private void TearDown()
        {
            if (_watchdog != null)
            {
                _watchdog.Stop();
                _watchdog.Tick -= OnWatchdogTick;
                _watchdog = null;
            }
            if (_bgWatchdog != null)
            {
                _bgWatchdog.Dispose();
                _bgWatchdog = null;
            }
            if (_hookAttached)
            {
                CompositionTarget.Rendering -= OnFrame;
                _hookAttached = false;
            }
            _clock.Reset();
            RustRenderer.Stop();
        }
    }
}
