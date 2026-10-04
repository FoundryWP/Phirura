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
                    + " -> " + uiState);
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
                    + " pxseq=" + RustRenderer.PxSeq);
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
        }
    }
}
