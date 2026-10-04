using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Input;
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
        /// <summary>
        /// The 5-second average the log carries, kept separate from <see cref="_clock"/> (1 s, the
        /// HUD) because they answer different questions: the HUD tells the person holding the phone
        /// what it is doing *now*, the log line tells whoever reads it later whether the renderer
        /// was ever competing with something else on the device.
        /// </summary>
        private readonly Stopwatch _clock5 = new Stopwatch();
        private int _frames5;
        /// <summary>
        /// Layout units to viewport pixels, captured from <c>RawPixelsPerViewPixel</c> during bring-up
        /// and used for every pointer position afterwards.
        ///
        /// Rust's projection is expressed in viewport pixels (it sizes the viewport from
        /// <c>width x height</c>), while XAML reports pointer positions in layout units - so this
        /// factor is the only thing standing between "the quad is under the finger" and "the quad is
        /// somewhere on the screen". A stale value of 1.0 would put everything in the top-left
        /// quarter, which is why it is written on every bring-up rather than once.
        /// </summary>
        private double _scale = 1.0;
        /// <summary>True while a finger is down, so press/release can be logged and moves cannot.</summary>
        private bool _touchDown;
        private bool _touchHooked;
        private bool _hookAttached;
        /// <summary>
        /// Set once the first frame's outcome has been logged. That line is the only place the phone
        /// log says whether pixels were actually drawn (and, if not, what the target surface looks
        /// like), so it must fire exactly once per bring-up - once every second would drown it, and
        /// never would leave the acceptance test unanswerable.
        /// </summary>
        private bool _firstFrameLogged;
        /// <summary>
        /// The last pixel-readback line written to the log, so each distinct answer is written once.
        ///
        /// Deduplicating on the *text* rather than on a success counter is deliberate: a readback
        /// that fails returns the same status every frame, and a counter would never advance, so the
        /// failure would never be logged at all. That is exactly how the first attempt produced a
        /// log with no pixel line in it and no way to tell a refused probe from an unexported one.
        /// </summary>
        private string _lastPxText;
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

        // ===========================================================================================
        // M1 on device: boot the *real* prpr engine on a phone.
        //
        // Path A's order is Stage 1 (this page, draws a quad) then Stage 3 (replay prpr's command
        // stream). Between them sits the question Stage 1 cannot answer and a desktop cannot ask:
        // does the engine actually run on a phone? Five things are only true or false on hardware -
        // thumbv7a code paths that have only ever been `cargo check`ed, 512 MB of RAM holding an
        // 8.8 MB chart plus a 32 MiB texture budget, `std::thread` aborting with 0xC0000409, the
        // shell's brokered file IO, and the audio header parsers.
        //
        // None of those need a picture, so this round deliberately asks for *logs only*: the engine
        // creates, ticks and reports how many draw commands it produced, while Stage 1 keeps
        // drawing underneath it. Finding out here costs a deploy; finding out after `scene.rs` is
        // written costs rewriting it.
        // ===========================================================================================

        /// <summary>Gate for the engine round. Stage 1 runs either way - the two are independent.</summary>
        /// <remarks>
        /// `static readonly`, not `const`: a `const bool` is folded by the compiler, so `if
        /// (!RunEngine)` with the value `true` would earn a CS0162 "unreachable code" warning on the
        /// `return`. A field is still a one-line gate when it is time to turn this off.
        /// </remarks>
        private static readonly bool RunEngine = true;

        /// <summary>Chart under `charts\` - the same one the desktop M1 host runs.</summary>
        private const string EngineFolder = "190";
        private const string EngineChart = "57649733.json";

        /// <summary>
        /// Engine lifecycle: 0 not started, 1 creating, 2 ready, 3 failed.
        ///
        /// **Static**, because the state it describes is not the page's. `wp81_game_create` is a
        /// process-wide singleton in the DLL, so two page instances - which happens the moment the
        /// page is re-entered, since this is the launch page and has no back stack - would each see
        /// their own private `0` and each fire a create. The first one is still parsing when the
        /// second arrives, and the second gets `AlreadyRunning` back: that is not a theory, it is
        /// what the first device run logged (`-> -3 (AlreadyRunning) in 167 ms`). A per-instance
        /// gate cannot protect a process-wide resource.
        ///
        /// Read by the frame loop and written by <see cref="StartEngineAsync"/> through
        /// <see cref="System.Threading.Interlocked"/>, so a torn read cannot have the frame loop
        /// calling into a half-built engine - which is exactly the class of crash this page's
        /// watchdogs exist to localise.
        /// </summary>
        private static int _engineState;

        /// <summary>Frames the engine has been ticked for, and the draw commands that produced.</summary>
        private static int _engineFrames;
        private static long _engineCmds;

        /// <summary>
        /// Viewport in pixels, captured when Stage 1 was brought up.
        ///
        /// The engine needs it before <c>GameCreate</c> (prpr projects through statics that default to
        /// 0x0, at which the aspect ratio is degenerate), and it must be the *same* size the renderer
        /// is using - otherwise the two would disagree about where anything is the moment Stage 3
        /// starts drawing it. <c>host_m1</c> passes 768 x 1229, which is exactly this phone.
        /// </summary>
        private uint _vpW;
        private uint _vpH;

        /// <summary>Timestamp source for the engine's <c>dt</c>, kept separate from the fps clocks.</summary>
        private readonly Stopwatch _engineClock = new Stopwatch();

        /// <summary>True once the engine's first tick has been logged. Static for the same reason as
        /// <see cref="_engineState"/>: it describes the process-wide engine, not this page.</summary>
        private static bool _engineTicked;

        /// <summary>
        /// Last engine progress step written to the log.
        ///
        /// Compared as text so each distinct step is written exactly once - a step that simply stops
        /// advancing has to be visible as "the last thing that happened", and a counter would say the
        /// same thing whether it was advancing or not.
        /// </summary>
        private string _lastEngineStep;

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

        /// <summary>
        /// UI-thread frame entries, counted independently of <c>_frames</c> (which resets every
        /// second to compute fps).
        ///
        /// Read by the background watchdog, whose thread-pool timer keeps firing while the UI thread
        /// does not. Without it a UI thread stuck inside a native call and a dead process look
        /// identical in the log: both are just "the log stopped".
        /// </summary>
        private int _uiFrames;
        private int _lastReportedUiFrames;
        private int _bgHeartbeat;

        private void OnBackgroundWatchdog(object state)
        {
            try
            {
                // Engine progress first, so it is sampled on *every* tick rather than only when
                // this thread has nothing else to say (the branches below both return early).
                //
                // This is the line that survives a `GameCreate` that kills the process: it runs on a
                // thread-pool timer, independent of both the UI thread and the pool thread doing the
                // creating, so when the log's last line is "engine step N", step N is what was
                // executing. Nothing else can say that - the create returns no code and writes no
                // line when it goes down.
                string engineStep = null;
                try
                {
                    engineStep = PrprPort.GameProgressText();
                }
                catch (Exception)
                {
                    // DLL not present or not yet loaded; the tick line below already reports that.
                }
                if (engineStep != null && engineStep != _lastEngineStep)
                {
                    _lastEngineStep = engineStep;
                    AppLog.WriteRaw("RustStage1: [bg] engine step " + engineStep);
                }

                // SampleTick() returns true only when the tick *changed*, so a hang shows as
                // exactly one line for the stuck tick - which is what we want (it names the call
                // that never returned). This works because the CoreWindow bring-up runs on a pool
                // thread, leaving the UI thread free to keep the AppLog FileIO path alive.
                if (RustRenderer.SampleTick())
                {
                    AppLog.WriteRaw("RustStage1: [bg] tick -> " + RustRenderer.LastSeenTick
                        + " (" + RustRenderer.LastSeenTickName + ")");
                    System.Threading.Interlocked.Exchange(ref _bgHeartbeat, 0);
                    return;
                }

                // ~4 s with no tick change: heartbeat anyway. A healthy frame loop changes the tick
                // several times a second, so this branch only fires when something has stopped.
                if (System.Threading.Interlocked.Increment(ref _bgHeartbeat) < 27)
                {
                    return;
                }
                System.Threading.Interlocked.Exchange(ref _bgHeartbeat, 0);

                int ui = System.Threading.Interlocked.CompareExchange(ref _uiFrames, 0, 0);
                int prev = System.Threading.Interlocked.CompareExchange(ref _lastReportedUiFrames, 0, 0);
                System.Threading.Interlocked.Exchange(ref _lastReportedUiFrames, ui);
                string uiState;
                if (!_hookAttached)
                {
                    uiState = "frame loop not attached";
                }
                else
                {
                    uiState = ui != prev ? "ui advancing" : "UI THREAD STUCK";
                }
                AppLog.WriteRaw("RustStage1: [bg] held tick=" + RustRenderer.LastSeenTick
                    + " (" + RustRenderer.LastSeenTickName + ") ui-frames=" + ui
                    + " -> " + uiState
                    // Engine state on the line that survives a hang. If the UI thread stalls inside
                    // `game_frame` this is the only place that says so: state 2 with the frame count
                    // frozen is the engine, state 2 with `ui advancing` is not.
                    + " | engine st=" + System.Threading.Interlocked.CompareExchange(ref _engineState, 0, 0)
                    + " " + System.Threading.Interlocked.CompareExchange(ref _engineFrames, 0, 0) + "f");
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
            AppLog.WriteSync("RustStage1: OnPageLoaded; RootGrid=" + RootGrid.ActualWidth
                + "x" + RootGrid.ActualHeight);
            // Pointer input. Hooked once, here, rather than in StartRenderer: a gesture that lands
            // before the renderer is up should still be reported, because "the touch did nothing" and
            // "the touch never reached the page" are otherwise the same line in the log. The HUD
            // overlay is not hit-test visible and the buttons handle their own clicks, but both
            // bubble, so a press anywhere on this grid arrives here.
            if (!_touchHooked)
            {
                _touchHooked = true;
                RootGrid.PointerPressed += OnPointerPressed;
                RootGrid.PointerMoved += OnPointerMoved;
                RootGrid.PointerReleased += OnPointerReleased;
                RootGrid.PointerCaptureLost += OnPointerCaptureLost;
            }
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

                uint w = (uint)Math.Max(1.0, Math.Round(RootGrid.ActualWidth * scale));
                uint h = (uint)Math.Max(1.0, Math.Round(RootGrid.ActualHeight * scale));
                AppLog.WriteSync("RustStage1: viewport " + RootGrid.ActualWidth + "x" + RootGrid.ActualHeight
                    + " -> " + w + "x" + h + " px @ " + scale.ToString("0.0"));
                // Pointer positions are in layout units; Rust's projection is in the pixel viewport
                // above. Same number, taken at the same moment as the size it describes.
                _scale = scale;

                // The bring-up is split in two because the two halves have different thread
                // requirements, and both requirements were measured on the device:
                //
                //  * `_prepare` (pool thread) - device, IDXGIDevice, pipeline, test texture. This is
                //    the unbounded half that can fault inside a driver call, so it must NOT run on
                //    the UI thread: a native fault there blocks WinRT FileIO, AppLog never flushes,
                //    and the crash arrives with no log at all.
                //  * `_attach` (UI thread)   - SurfaceImageSource activation + SetDevice. Running
                //    this half from a pool thread returns RPC_E_WRONG_THREAD (0x8001010E) at
                //    CreateInstanceWithDimensionsAndOpacity, because SurfaceImageSource is a XAML
                //    object with XAML's thread affinity. Measured on device (Round 49).
                //
                // ROUTE (Round 48): neither a swapchain nor the D3D/WinRT interop helpers are
                // reachable from a WP8.1 phone app. CreateSwapChainForCoreWindow and
                // CreateSwapChainForComposition both return DXGI_ERROR_INVALID_CALL on this Adreno
                // 225 / WDDM 1.2 / FL 9.3 driver, and the interop load probe logged
                // `interop-trace=winrt{h=0,exp=0} d3d11{h=1,exp=0}` - winrt.dll will not load from
                // an app container and d3d11.dll does not export the helper.
                //
                // What is reachable is XAML's own SurfaceImageSource: Rust activates it with
                // RoGetActivationFactory (from runtimeobject.lib, the API set an app *can* load and
                // the one Win2D itself imports), QIs ISurfaceImageSourceNative with a plain
                // QueryInterface - no interop helper - and draws into the IDXGISurface that
                // BeginDraw hands back, which lives inside the compositor's own render target.
                // Drawing into it *is* presenting.
                AppLog.WriteSync("RustStage1: surface prepare on pool thread");
                bool ok = false;
                await System.Threading.Tasks.Task.Run(delegate
                {
                    ok = RustRenderer.StartSurfacePrepare(w, h);
                });

                AppLog.WriteSync("RustStage1: surface prepare finished ok=" + ok
                    + " tick=" + RustRenderer.Tick + " (" + RustRenderer.TickName + ")");

                // UI thread from here down (this method resumed on it after the await).
                bool attached = false;
                if (ok)
                {
                    AppLog.WriteSync("RustStage1: surface attach on UI thread");
                    attached = RustRenderer.StartSurfaceAttach();
                    AppLog.WriteSync("RustStage1: surface attach finished ok=" + attached
                        + " route=" + RustRenderer.Route + " (" + RustRenderer.RouteName + ")"
                        + " sis-step=" + RustRenderer.SisStep
                        + " sis-hr=0x" + RustRenderer.SisHr.ToString("X8")
                        + " source=0x" + RustRenderer.SisSource.ToInt32().ToString("X8")
                        // The attach-time probe just read XAML's surface: log its shape here, before
                        // any frame can try to build a view of it and take the process down.
                        + " | " + RustRenderer.SisDiagText
                        + " | " + RustRenderer.Diagnose());
                }
                ok = ok && attached;

                // The SurfaceImageSource object exists only if the route came all the way up. If it
                // did, hand it to XAML (UI thread - it touches XAML).
                bool presented = false;
                if (ok)
                {
                    presented = AttachSurfaceSource(w, h);
                    AppLog.WriteSync("RustStage1: surface attach -> " + presented);
                    // A live renderer with nothing to show is still a failure - the frame loop would
                    // run and report fps against an invisible Image.
                    ok = presented;
                }

                string route = RustRenderer.RouteName;
                StageText.Text = ok ? "启动成功 · stage=0" : "启动失败 · " + RustRenderer.Diagnose();
                HrText.Text = ok
                    ? "route: " + route
                    : (RustRenderer.Stage >= 500
                        ? "surface 步: " + RustRenderer.SisStepName
                          + "  hr=0x" + RustRenderer.SisHr.ToString("X8")
                          + (RustRenderer.LastHrName.Length > 0 ? " (" + RustRenderer.LastHrName + ")" : "")
                        : RustRenderer.Stage >= 300
                            ? "offscreen 步: " + RustRenderer.OtStepName
                              + "  hr=0x" + RustRenderer.LastHr.ToString("X8")
                              + (RustRenderer.LastHrName.Length > 0 ? " (" + RustRenderer.LastHrName + ")" : "")
                            : RustRenderer.ScStep > 0
                                ? "swapchain 步: " + RustRenderer.ScStepName + "  hr=0x" + RustRenderer.LastHr.ToString("X8")
                                  + (RustRenderer.LastHrName.Length > 0 ? " (" + RustRenderer.LastHrName + ")" : "")
                                : "");
                SizeText.Text = w + " x " + h + " px  (scale " + scale.ToString("0.00") + ")"
                    + (ok ? "  · attach=" + (presented ? "yes" : "no") : "");
                if (!ok)
                {
                    // Do not attach the frame loop to a renderer that never came up - and release
                    // whatever half did (a prepared device, or a live one with no XAML source),
                    // otherwise a failed run leaves a D3D11 device behind on a 512 MB phone. Hand
                    // XAML's reference back first (same order as TearDown) so the compositor cannot
                    // be left holding a surface Rust is about to free.
                    try
                    {
                        RustImage.Source = null;
                        RustImage.Visibility = Visibility.Collapsed;
                    }
                    catch (Exception) { }
                    RustRenderer.Stop();
                    return;
                }

                _clock.Start();
                _frames = 0;
                _clock5.Restart();
                _frames5 = 0;
                _firstFrameLogged = false;
                _lastPxText = null;
                // The size Stage 1 is drawing at, for the engine to project through. Same numbers,
                // taken at the same moment as the swapchain they describe.
                _vpW = w;
                _vpH = h;
                // Recentre. Rust already forgets the touch inside `release_all`, which ran during
                // prepare; doing it here too makes the *page* the one that decided where the quad
                // starts, and gives the missing-import case a line of its own instead of a silent
                // quad that never comes back to the middle.
                RustRenderer.SetTouch(0, 0, -1);
                if (!_hookAttached)
                {
                    CompositionTarget.Rendering += OnFrame;
                    _hookAttached = true;
                }

                // Engine *after* the frame loop is attached, on purpose: Stage 1 is the proven half
                // and has to keep drawing and reporting even if the engine never comes up. The loop
                // below ticks it only when _engineState says GameCreate has returned, so a create
                // still running on a pool thread is never entered concurrently with a frame.
                var engineIgnored = StartEngineAsync();
            }
            finally
            {
                System.Threading.Interlocked.Exchange(ref _startGate, 0);
            }
        }

        /// <summary>
        /// Hand the WinRT <c>SurfaceImageSource</c> that Rust created to XAML. **Must run on the UI
        /// thread** - it touches XAML.
        ///
        /// This is the entire C# involvement in presentation: one <c>IInspectable*</c> is projected
        /// back to a managed object and assigned to an <see cref="Image"/>'s <c>Source</c>. The
        /// projection round-trips to <c>Windows.UI.Xaml.Media.Imaging.SurfaceImageSource</c>, which
        /// <b>is</b> a <c>Windows.UI.Xaml.Media.ImageSource</c> - verified with
        /// <c>tools/probe_api.py --file t_sis1.cs</c>, which compiled
        /// <c>img.Source = new SurfaceImageSource(64, 64);</c> against the real WP8.1 reference set.
        ///
        /// Nothing here creates a device, a texture, a swapchain, a Win2D object or a blit. Rust
        /// owns the device, the surface and every draw call; the compositor reads the surface Rust
        /// draws into, because that surface <em>is</em> part of the compositor's own render target.
        /// That is what keeps this Path A and not a C# renderer with a Rust backend.
        /// </summary>
        private bool AttachSurfaceSource(uint pixelWidth, uint pixelHeight)
        {
            IntPtr source = RustRenderer.SisSource;
            if (source == IntPtr.Zero)
            {
                AppLog.WriteSync("RustStage1: sis source pointer null");
                return false;
            }
            try
            {
                // Raw IInspectable* -> managed RCW. The runtime QIs and projects it.
                object obj = Marshal.GetObjectForIUnknown(source);
                AppLog.WriteSync("RustStage1: sis source projects to "
                    + (obj == null ? "(null)" : obj.GetType().FullName));

                var imageSource = obj as ImageSource;
                if (imageSource == null)
                {
                    AppLog.WriteSync("RustStage1: sis source is not an ImageSource");
                    return false;
                }

                // No Width/Height to set: a SurfaceImageSource already knows its pixel size
                // (Rust created it at exactly this size), and XAML sizes the Image from that.
                RustImage.Source = imageSource;
                RustImage.Visibility = Visibility.Visible;

                AppLog.WriteSync("RustStage1: compositor source set (image "
                    + pixelWidth + "x" + pixelHeight + ")");
                return true;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustStage1: surface attach threw", ex);
                return false;
            }
        }

        /// <summary>
        /// Boot the real <c>prpr</c> engine on the phone: point it at <c>charts\</c>, tell it the
        /// screen size, create the game.
        ///
        /// This is M1's device leg. Everything it does has been proven on the desktop host - the
        /// same chart, the same 8.8 MB parse, the same 75 textures - and none of it has ever run on
        /// ARM. Five outcomes are only knowable here: whether the thumbv7a code paths that have only
        /// ever been compiled actually execute, whether 512 MB survives an 8.8 MB chart plus a 32 MiB
        /// texture budget, whether anything reaches <c>std::thread</c> and aborts with 0xC0000409,
        /// whether the shell's brokered file IO answers from inside a native frame, and whether the
        /// audio header parsers behave.
        ///
        /// On a **pool thread**, for the same reason <c>StartSurfacePrepare</c> is: <c>GameCreate</c>
        /// can fault anywhere, and a native fault on the UI thread blocks WinRT <c>FileIO</c>, which
        /// stops <c>AppLog</c> flushing and ends the run with no log at all. It has the second
        /// benefit that the asset reads' <c>.AsTask().Result</c> run where there is no
        /// SynchronizationContext, so their continuations are free to complete rather than queue
        /// behind the thread waiting on them.
        ///
        /// Failure is logged and swallowed. Stage 1 is this page's primary verdict and must keep
        /// drawing; an engine that does not come up is a failed experiment, not a broken page.
        /// </summary>
        private async System.Threading.Tasks.Task StartEngineAsync()
        {
            if (!RunEngine)
            {
                return;
            }
            // Interlocked, because this runs on the UI thread while the frame loop reads it there
            // too and a second navigation can re-enter. CompareExchange makes "who starts it" a
            // single decision rather than a check-then-act.
            if (System.Threading.Interlocked.CompareExchange(ref _engineState, 1, 0) != 0)
            {
                AppLog.WriteSync("RustStage1: engine start skipped (state=" + _engineState + ")");
                return;
            }

            string root = Windows.Storage.ApplicationData.Current.LocalFolder.Path + "\\charts";
            PrprPort.AssetRoot = root;
            AppLog.WriteSync("RustStage1: engine create \"" + EngineFolder + "/" + EngineChart
                + "\" root=" + root + " screen=" + _vpW + "x" + _vpH + " (pool thread)");

            PrprPort.GameResult result = PrprPort.GameResult.NoChart;
            string threw = null;
            Stopwatch create = Stopwatch.StartNew();

            // Sampler first, so it is already looping before the first native call.
            //
            // The 150 ms watchdog lost this race on the first instrumented run: it read `idle` at
            // 19:27:52.448, the create started at 19:27:53.057, and by the next tick the process was
            // gone - so the log could not distinguish "died before it started" from "died in the
            // middle". A thread that is *spinning* rather than scheduled reads the step in the
            // microseconds between the store and the crash, which is the whole difference between
            // knowing and guessing.
            var spin = System.Threading.Tasks.Task.Run(() => SampleCreateProgress());

            await System.Threading.Tasks.Task.Run(delegate
            {
                try
                {
                    // Before create: prpr projects through statics that default to 0x0, and at 0x0
                    // the aspect ratio is degenerate, so everything projects onto a point. The same
                    // pixel size Stage 1 is drawing at - host_m1 passes 768 x 1229, this phone's
                    // viewport - so that engine and renderer agree about where things are.
                    PrprPort.GameSetScreen(_vpW, _vpH);
                    // "Returned" is the point of this line. `set_screen` writes one static and
                    // cannot fail, so a missing line means the death came *before* `game_create`,
                    // and a present one means it came inside. Without it those two are the same
                    // silence.
                    AppLog.WriteSync("PrprPort: GameSetScreen " + _vpW + "x" + _vpH + " ok");
                    // Read BEFORE the create, not after.
                    //
                    // `GameRunning` and `GameFrames` both go through `host::GAME`, which is a
                    // `thread_local!` - and on the first device run `GameCreate` came back
                    // `AlreadyRunning` on a brand-new process where nothing had created anything.
                    // The only way that happens is if the thread-local reads as occupied when it is
                    // not, i.e. if the TLS slot this CRT-free ARM DLL was linked with does not hold
                    // the value Rust thinks it does. Checked here, before any code has had a chance
                    // to populate `GAME`, so the answer is unambiguous: `running=False frames=-1`
                    // means TLS is sound and the problem is elsewhere; anything else names it.
                    AppLog.WriteSync("PrprPort: before create: running=" + PrprPort.GameRunning()
                        + " frames=" + PrprPort.GameFrames());
                    result = PrprPort.GameCreate(EngineFolder, EngineChart);
                }
                catch (Exception ex)
                {
                    threw = ex.GetType().Name + ": " + ex.Message;
                }
            });
            create.Stop();

            // Status first and separately, because it is the part that can throw again: if the DLL
            // failed to load, asking it for its state inside the *log line* would escape the method
            // and take down the page the watchdogs were there to describe.
            string status;
            try
            {
                status = "running=" + PrprPort.GameRunning()
                    + " | " + PrprPort.TextureSummary();
            }
            catch (Exception ex)
            {
                status = "status unavailable: " + ex.GetType().Name;
            }

            AppLog.WriteSync("RustStage1: GameCreate(" + EngineFolder + ", " + EngineChart + ") -> "
                + (int)result + " (" + result + ") in " + create.ElapsedMilliseconds + " ms"
                + (threw == null ? "" : " THREW " + threw)
                + " | " + status);

            // The reason, on its own line and only when there is something to say. The return code
            // by itself was worth very little: `-2` covered a chart the parser rejects, a
            // `GameScene::new` dying part-way through its textures and a `Main::new` with no font,
            // and on the phone the `tracing` event carrying the message has no subscriber to give
            // it to. This read-back in the same turn is the whole diagnosis, so it is logged
            // separately rather than folded into the line above - a failure here must not be able
            // to lose the line that carries the actual result.
            if (result != PrprPort.GameResult.Ok || threw != null)
            {
                try
                {
                    string why = PrprPort.GameLastError();
                    AppLog.WriteSync("RustStage1: GameCreate reason: "
                        + (why ?? "(none recorded)"));
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("RustStage1: GameCreate reason unavailable: " + ex.Message);
                }
            }

            bool ready = result == PrprPort.GameResult.Ok && threw == null;
            System.Threading.Interlocked.Exchange(ref _engineState, ready ? 2 : 3);
            AppLog.WriteSync(ready
                ? "RustStage1: engine ready"
                : "RustStage1: engine NOT running - Stage 1 continues alone");
        }

        /// <summary>
        /// Read <c>GameCreate</c>'s progress step in a tight loop while it runs, logging each change.
        ///
        /// The background watchdog samples every 150 ms, which is right for a *renderer* hang and
        /// wrong here: on the first instrumented run the process died well inside one interval, so
        /// the last step it ever saw was still `idle` and the log could not say what was executing.
        /// Spinning instead of being scheduled is the difference - the store happens in Rust, the
        /// crash happens microseconds later, and only a loop that is already running catches the
        /// value in between.
        ///
        /// Bounded rather than infinite: if the create hangs instead of dying, this must not become
        /// a second permanent busy loop on a phone. Ten seconds is several times the longest create
        /// seen anywhere, desktop included.
        /// </summary>
        private void SampleCreateProgress()
        {
            const int maxIterations = 5000;   // ~2 ms apart, so about 10 s
            string last = null;
            for (int i = 0; i < maxIterations; i++)
            {
                if (System.Threading.Interlocked.CompareExchange(ref _engineState, 1, 1) != 1)
                {
                    return;
                }
                string step;
                try
                {
                    step = PrprPort.GameProgressText();
                }
                catch (Exception)
                {
                    // The DLL has gone with the process. Nothing further to read, and the last line
                    // already written is the answer.
                    return;
                }
                if (step != last)
                {
                    last = step;
                    AppLog.WriteRaw("RustStage1: [spin] engine step " + step);
                }
            }
        }

        /// <summary>
        /// One frame. Clear -> one test quad -> present, all inside Rust.
        ///
        /// Kept on the UI thread on purpose: the pixels are composited by XAML, and XAML's
        /// <c>SurfaceImageSource</c> is a UI-thread object, so the <c>BeginDraw</c>/<c>EndDraw</c>
        /// pair Rust drives must happen on the thread that owns it. The work per frame is three
        /// native calls, so this is cheap.
        /// </summary>
        private void OnFrame(object sender, object e)
        {
            // First, before anything that can block: this counter is the background watchdog's proof
            // that the UI thread is still running (see `_uiFrames`). Incrementing it after the
            // native call would make the proof circular - a UI thread stuck *inside* that call would
            // look identical to a healthy idle one.
            System.Threading.Interlocked.Increment(ref _uiFrames);

            if (!RustRenderer.Started)
            {
                return;
            }

            // M1: tick the engine, but only once GameCreate has returned. The create runs on a pool
            // thread, so this gate is what keeps `game_frame` from ever entering a half-built engine.
            if (_engineState == 2)
            {
                double dt;
                if (_engineClock.IsRunning)
                {
                    dt = _engineClock.Elapsed.TotalSeconds;
                    _engineClock.Restart();
                }
                else
                {
                    // No previous timestamp to subtract from on the first tick. One frame's worth,
                    // because starting at 0 s would advance the engine by nothing at all and later
                    // read as "the engine never moved".
                    _engineClock.Restart();
                    dt = 1.0 / 60.0;
                }

                // Zero capacities, and this is not an oversight: `Drain` allocates
                // `new PortVertex[vertexCapacity]` and reads every element of it on *every* frame
                // whether or not anything was recorded, and this round needs only `Count` - which
                // comes back either way. The real capacities belong to Stage 3, when something
                // actually draws them.
                var frame = PrprPort.GameFrame(dt, 0, 0);
                _engineFrames++;
                _engineCmds += frame.Count;
                if (!_engineTicked)
                {
                    _engineTicked = true;
                    AppLog.WriteSync("RustStage1: engine frame#1 -> " + frame.Count
                        + " commands (dt=" + dt.ToString("0.000") + ")");
                }
            }

            // `draw_test_quad` is the whole frame on this route: BeginDraw, draw, EndDraw. There is
            // no blit afterwards because there is nothing to blit *to* - EndDraw is what publishes.
            bool drew = RustRenderer.DrawTestFrame();

            // The first frame is the whole Stage 1 verdict, so it is logged either way. On a blank
            // screen this line is the only evidence: `rc=-3` plus the surface's format/BindFlags and
            // both render-target-view attempts' HRESULTs say which of the two opposite fixes applies,
            // while `rc=0` proves the draw and points at XAML/visibility instead.
            if (!_firstFrameLogged)
            {
                _firstFrameLogged = true;
                AppLog.WriteSync("RustStage1: frame#1 rc=" + RustRenderer.LastFrameRc
                    + " verts=" + RustRenderer.LastVertexCount
                    + " sis-step=" + RustRenderer.SisStep
                    + " sis-hr=0x" + RustRenderer.SisHr.ToString("X8")
                    + " | " + RustRenderer.SisDiagText);
            }

            // What the driver painted, read back out of the surface by the frame itself.
            //
            // `frame#1` reports that the calls returned; this reports what they *produced*, which is
            // the only distinction the acceptance test turns on. Logged whenever the answer changes
            // — including a refusal, which is a stable string and so still written exactly once.
            string px = RustRenderer.PxText;
            if (px != _lastPxText)
            {
                _lastPxText = px;
                AppLog.WriteSync("RustStage1: " + px);
            }

            if (!drew)
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

            // The 5-second average, into the log rather than the HUD.
            //
            // One line every 5 s whether the number is good or bad, and *not* deduplicated: a
            // renderer holding 60 fps while the phone does something else behind it looks identical
            // to one that is simply idle until the samples are spread over time. Cheap enough to
            // keep (one string and one file append), which is the whole reason it is a fixed window
            // and not a profiler.
            _frames5++;
            if (_clock5.ElapsedMilliseconds >= 5000)
            {
                long ms = _clock5.ElapsedMilliseconds;
                AppLog.WriteSync("RustStage1: fps avg=" + (_frames5 * 1000.0 / ms).ToString("0.0")
                    + " over " + ms + " ms (" + _frames5 + " frames)"
                    + " verts=" + RustRenderer.LastVertexCount
                    + " stage=" + RustRenderer.Stage
                    + " pxseq=" + RustRenderer.PxSeq
                    // Engine half, on the same line so a single grep answers "was it ticking, and
                    // was it producing commands". A frame count alone would not: an engine that
                    // ticks and yields zero commands every time looks identical to a healthy one
                    // that is simply between notes.
                    + " | engine st=" + _engineState + " " + _engineFrames + "f "
                    + _engineCmds + "cmds");
                _clock5.Restart();
                _frames5 = 0;
            }
        }

        // ---- pointer input ------------------------------------------------------------------------
        //
        // Everything below converts one thing and hands it to Rust: where the finger is, in the
        // pixel viewport. There is no managed rendering state to keep in sync - the quad's position
        // *is* Rust's projection matrix - so a missed move costs a frame of lag and nothing else.

        /// <summary>
        /// Forward a pointer position to Rust, in viewport pixels.
        ///
        /// <paramref name="down"/> is the tri-state Rust's single import takes: 1 down (the quad
        /// follows), 0 lifted (it stays, and Rust arms one readback there), -1 never-touched.
        /// Only presses and lifts are logged - a move is up to 60 a second, and the interesting
        /// question is not where the finger was but that the gesture started and ended.
        /// </summary>
        private void ReportTouch(PointerRoutedEventArgs e, int down)
        {
            var pt = e.GetCurrentPoint(RootGrid);
            float x = (float)(pt.Position.X * _scale);
            float y = (float)(pt.Position.Y * _scale);
            bool edge = (down == 1) != _touchDown;
            _touchDown = down == 1;
            RustRenderer.SetTouch(x, y, down);
            // Only the edges. A hover-style move (no button) would otherwise log on every event,
            // and moves during a drag are exactly the traffic the 5-second FPS line exists to keep
            // the log readable against.
            if (edge)
            {
                AppLog.WriteSync("RustStage1: touch " + (down == 1 ? "down" : "up")
                    + " @(" + x.ToString("0") + "," + y.ToString("0") + ") vp"
                    + " scale=" + _scale.ToString("0.0")
                    + " started=" + RustRenderer.Started);
            }
        }

        private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
        {
            ReportTouch(e, 1);
        }

        private void OnPointerMoved(object sender, PointerRoutedEventArgs e)
        {
            // `IsLeftButtonPressed` rather than an unconditional 1: a move with no button is the
            // hover/second-finger case, and telling Rust "down" for it would leave `down` stuck on
            // after the real finger lifts, so the readback would never be armed.
            ReportTouch(e, e.GetCurrentPoint(RootGrid).Properties.IsLeftButtonPressed ? 1 : 0);
        }

        private void OnPointerReleased(object sender, PointerRoutedEventArgs e)
        {
            ReportTouch(e, 0);
        }

        /// <summary>
        /// The finger is gone without a `PointerReleased` - the usual reason a XAML app sees one is
        /// another element taking capture. Lift anyway, or `down` stays set in Rust and the quad
        /// would follow a finger that is no longer there, with the readback never armed to say so.
        /// </summary>
        private void OnPointerCaptureLost(object sender, PointerRoutedEventArgs e)
        {
            if (_touchDown)
            {
                ReportTouch(e, 0);
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
                return;
            }
            // No back stack: this is the launch page (App.xaml.cs navigates straight here), so
            // `CanGoBack` is false and doing nothing closes the app - which is what it did, leaving
            // nowhere to go and no way back to the menu. Navigate to MainPage instead; a bare back
            // button on the root page is a dead end, not a decision.
            this.Frame.Navigate(typeof(MainPage));
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
            // Drop XAML's reference to Rust's SurfaceImageSource before the native side releases it,
            // so the compositor cannot be left holding a freed surface. There is no Win2D object in
            // the chain any more - on this route the surface *is* the compositor's render target, so
            // clearing Image.Source is the whole hand-back.
            try
            {
                if (RustImage != null)
                {
                    RustImage.Source = null;
                    RustImage.Visibility = Visibility.Collapsed;
                }
            }
            catch (Exception) { }
            RustRenderer.Stop();

            // Engine, last, and only when it is not mid-create: destroying while `GameCreate` is
            // still running on a pool thread would free the engine out from under it. Leaving it
            // running instead is recoverable - a later create reports AlreadyRunning, which the log
            // names - while the race is not.
            int engineState = _engineState;
            if (engineState == 1)
            {
                AppLog.WriteSync("RustStage1: engine create still in flight at teardown; not destroyed");
            }
            else
            {
                if (engineState != 0)
                {
                    try
                    {
                        PrprPort.GameDestroy();
                        AppLog.WriteSync("RustStage1: GameDestroy after " + _engineFrames
                            + " engine frames / " + _engineCmds + " commands");
                    }
                    catch (Exception ex)
                    {
                        AppLog.WriteSync("RustStage1: GameDestroy", ex);
                    }
                }
                _engineState = 0;
                _engineFrames = 0;
                _engineCmds = 0;
                _engineTicked = false;
                // Cleared, not just left: `IsRunning` would still be true from the run that just
                // ended, so the next first tick would compute `dt` against a timestamp taken
                // minutes ago and advance the engine by that whole gap in one step.
                _engineClock.Reset();
            }
        }
    }
}
