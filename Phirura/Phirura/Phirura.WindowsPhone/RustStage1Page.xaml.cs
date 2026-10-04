using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
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
            AppLog.WriteSync("RustStage1: OnPageLoaded; RootGrid=" + RootGrid.ActualWidth
                + "x" + RootGrid.ActualHeight);
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

                // Run the native bring-up on a pool thread so the UI thread stays alive to flush the
                // log if a native step hard-faults. Each step is logged before and after, so the last
                // line in phirura.log names the call that killed the process.
                //
                // CORRECTED ROUTE (Round 45): a WP8.1 *phone* app does not present through a
                // swapchain. Both CreateSwapChainForCoreWindow and CreateSwapChainForComposition
                // return DXGI_ERROR_INVALID_CALL on this Adreno 225 / WDDM 1.2 / FL 9.3 driver, and
                // Win2D - which renders fine on the same device - imports no swapchain entry points
                // at all. So the bring-up is now the offscreen + WinRT-interop route: Rust renders
                // into an offscreen texture and hands back a WinRT IDirect3DSurface/IDirect3DDevice
                // pair for the XAML compositor. The old composition/CoreWindow fallback chain is
                // gone - neither can ever succeed here.
                AppLog.WriteSync("RustStage1: starting offscreen bring-up on pool thread");
                bool ok = false;
                await System.Threading.Tasks.Task.Run(delegate
                {
                    ok = RustRenderer.StartOffscreen(w, h);
                });

                AppLog.WriteSync("RustStage1: offscreen bring-up finished, ok=" + ok
                    + " route=" + RustRenderer.Route + " (" + RustRenderer.RouteName + ")"
                    + " ot-step=" + RustRenderer.OtStep
                    + " surface-kind=" + RustRenderer.OtSurfaceKindName
                    + " interop-src=" + RustRenderer.InteropSourceName
                    + " surface=0x" + RustRenderer.GfxSurface.ToInt32().ToString("X8")
                    + " device=0x" + RustRenderer.GfxDevice.ToInt32().ToString("X8")
                    + " " + RustRenderer.Diagnose());

                // The interop objects exist only if the route came all the way up. If it did, hand
                // the surface+device to the compositor (UI thread - it touches XAML).
                bool presented = false;
                if (ok)
                {
                    presented = AttachInteropSurface(w, h);
                    AppLog.WriteSync("RustStage1: interop attach -> " + presented);
                }

                string route = RustRenderer.RouteName;
                StageText.Text = ok ? "启动成功 · stage=0" : "启动失败 · " + RustRenderer.Diagnose();
                HrText.Text = ok
                    ? "route: " + route
                      + (RustRenderer.OtStep > 0 && RustRenderer.OtStep < 44
                          ? " · " + RustRenderer.OtStepName : "")
                    : (RustRenderer.Stage >= 300
                        ? "offscreen 步: " + RustRenderer.OtStepName
                          + "  hr=0x" + RustRenderer.LastHr.ToString("X8")
                          + (RustRenderer.LastHrName.Length > 0 ? " (" + RustRenderer.LastHrName + ")" : "")
                        : RustRenderer.ScStep > 0
                            ? "swapchain 步: " + RustRenderer.ScStepName + "  hr=0x" + RustRenderer.LastHr.ToString("X8")
                              + (RustRenderer.LastHrName.Length > 0 ? " (" + RustRenderer.LastHrName + ")" : "")
                            : "");
                SizeText.Text = w + " x " + h + " px  (scale " + scale.ToString("0.00") + ")"
                    + (ok ? "  · composite=" + (presented ? "yes" : "no") : "");
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
        /// Hand the WinRT <c>IDirect3DSurface</c>/<c>IDirect3DDevice</c> pair that Rust created to
        /// the XAML compositor. **Must run on the UI thread** - it touches XAML.
        ///
        /// This is the one place C# touches the render target, and it does <em>not</em> own it:
        /// Rust created the device, the offscreen texture and the surface, and keeps ownership. C#
        /// only carries the two raw <c>IInspectable*</c>s into the compositor, using Win2D purely
        /// as a <b>projector + blitter</b>:
        ///
        ///   <c>IDirect3DDevice*</c>  -> <c>CanvasDevice</c>   (satisfies Win2D's "has a device" rule)
        ///   <c>IDirect3DSurface*</c> -> <c>CanvasBitmap</c>   (wraps Rust's texture; no copy)
        ///   <c>CanvasImageSource</c> sized to the panel, then one <c>DrawImage</c> per frame
        ///
        /// Every signature here was verified against the vendored Win2D winmd with
        /// <c>tools/probe_api.py</c> - including the fact that the interop types live in
        /// <c>Microsoft.Graphics.Canvas.DirectX.Direct3D11</c>, because <b><c>Windows.Graphics.DirectX</c>
        /// does not exist in the WP8.1 C# reference set at all</b> (CS0234 on every type in it).
        ///
        /// Nothing here creates a device, a texture or a swapchain - that is all Rust's, which is
        /// what keeps this Path A and not a C# renderer with a Rust backend.
        /// </summary>
        private bool AttachInteropSurface(uint pixelWidth, uint pixelHeight)
        {
            IntPtr surface = RustRenderer.GfxSurface;
            IntPtr device = RustRenderer.GfxDevice;
            if (surface == IntPtr.Zero || device == IntPtr.Zero)
            {
                AppLog.WriteSync("RustStage1: interop pointers null (surface=0x"
                    + surface.ToInt32().ToString("X8") + " device=0x"
                    + device.ToInt32().ToString("X8") + ")");
                return false;
            }
            try
            {
                // Raw IInspectable*s -> managed RCWs. GetObjectForIUnknown performs the QI.
                var winrtDevice = (Microsoft.Graphics.Canvas.DirectX.Direct3D11.IDirect3DDevice)
                    Marshal.GetObjectForIUnknown(device);
                var winrtSurface = (Microsoft.Graphics.Canvas.DirectX.Direct3D11.IDirect3DSurface)
                    Marshal.GetObjectForIUnknown(surface);

                // Win2D wraps *Rust's* device and *Rust's* texture - it does not allocate either.
                _canvasDevice = Microsoft.Graphics.Canvas.CanvasDevice.CreateFromDirect3D11Device(winrtDevice);
                _canvasBitmap = Microsoft.Graphics.Canvas.CanvasBitmap.CreateFromDirect3D11Surface(
                    _canvasDevice, winrtSurface, 96.0f);

                // A CanvasImageSource is an ImageSource, and XAML blits it for us. It needs a size
                // in DIPs; at 96 dpi that is the pixel size, which is what Rust rendered at.
                _imageSource = new Microsoft.Graphics.Canvas.UI.Xaml.CanvasImageSource(
                    _canvasDevice, (float)pixelWidth, (float)pixelHeight, 96.0f);

                RustImage.Width = pixelWidth;
                RustImage.Height = pixelHeight;
                RustImage.Source = _imageSource;
                RustImage.Visibility = Visibility.Visible;

                // One blit so the very first frame is visible even before CompositionTarget ticks.
                BlitToImageSource();

                AppLog.WriteSync("RustStage1: compositor source set ("
                    + _canvasBitmap.SizeInPixels.Width + "x" + _canvasBitmap.SizeInPixels.Height
                    + ", image " + pixelWidth + "x" + pixelHeight + ")");
                return true;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("RustStage1: interop attach threw", ex);
                return false;
            }
        }

        private Microsoft.Graphics.Canvas.CanvasDevice _canvasDevice;
        private Microsoft.Graphics.Canvas.CanvasBitmap _canvasBitmap;
        private Microsoft.Graphics.Canvas.UI.Xaml.CanvasImageSource _imageSource;

        /// <summary>
        /// Copy Rust's current render target into the <c>CanvasImageSource</c> XAML displays.
        ///
        /// This is a GPU-to-GPU blit inside the compositor's own device chain: the source is the
        /// <c>CanvasBitmap</c> wrapping Rust's <c>ID3D11Texture2D</c>, the destination is the
        /// <c>CanvasImageSource</c> XAML owns. No pixels come back to the CPU, and no draw state
        /// from Rust is touched. UI thread only.
        /// </summary>
        private void BlitToImageSource()
        {
            if (_imageSource == null || _canvasBitmap == null)
            {
                return;
            }
            using (var ds = _imageSource.CreateDrawingSession(Windows.UI.Colors.Black))
            {
                ds.DrawImage(_canvasBitmap);
            }
        }

        /// <summary>
        /// One frame. Clear -> one test quad -> present, all inside Rust.
        ///
        /// Kept on the UI thread on purpose: the pixels are composited by XAML, and touching a
        /// XAML-owned surface's producing device off the UI thread is not something WP8.1
        /// guarantees. The work per frame is three native calls, so this is cheap.
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

            // Rust has just cleared + drawn into its offscreen texture; make the compositor show
            // that texture. This is the "present" step on the offscreen route - Rust's own
            // wp81_rs_present() is a no-op here because there is no swapchain to present to.
            BlitToImageSource();

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
            // Drop the compositor's reference to Rust's surface before the native side releases it,
            // so the compositor cannot be left holding a freed texture. Order matters: the XAML
            // Image lets go of the CanvasImageSource first, then the CanvasBitmap that wraps Rust's
            // texture, and only then does Rust free the texture itself (RustRenderer.Stop below).
            try
            {
                if (RustImage != null)
                {
                    RustImage.Source = null;
                    RustImage.Visibility = Visibility.Collapsed;
                }
            }
            catch (Exception) { }
            _imageSource = null;
            if (_canvasBitmap != null)
            {
                try { _canvasBitmap.Dispose(); } catch (Exception) { }
                _canvasBitmap = null;
            }
            if (_canvasDevice != null)
            {
                // Do NOT Dispose() this - it wraps Rust's device, and disposing it would release a
                // device Rust still owns. Dropping the reference is all that is correct here.
                _canvasDevice = null;
            }
            RustRenderer.Stop();
        }
    }
}
