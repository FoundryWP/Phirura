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

        public RustStage1Page()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Disabled;
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            // Wait for the panel to have a measured size before creating a swapchain against it.
            this.Loaded += OnPageLoaded;
        }

        protected override void OnNavigatedFrom(NavigationEventArgs e)
        {
            base.OnNavigatedFrom(e);
            TearDown();
        }

        private void OnPageLoaded(object sender, RoutedEventArgs e)
        {
            this.Loaded -= OnPageLoaded;
            // The panel must be laid out before its pixel size is meaningful.
            // On WP8.1 CoreDispatcher has RunAsync, not BeginInvoke.
            var ignore = this.Dispatcher.RunAsync(Windows.UI.Core.CoreDispatcherPriority.Normal,
                () => StartRenderer());
        }

        private async void StartRenderer()
        {
            // One more layout pass: the first frame after navigation can report 0x0.
            await System.Threading.Tasks.Task.Delay(60);
            if (RustRenderer.Started)
            {
                return;
            }

            double scale = 1.0;
            try { scale = Windows.Graphics.Display.DisplayInformation.GetForCurrentView().RawPixelsPerViewPixel; }
            catch (Exception ex) { AppLog.WriteSync("RustStage1: RawPixelsPerViewPixel", ex); }

            uint w = (uint)Math.Max(1.0, Math.Round(RustPanel.ActualWidth * scale));
            uint h = (uint)Math.Max(1.0, Math.Round(RustPanel.ActualHeight * scale));

            bool ok = RustRenderer.Start(RustPanel, w, h);
            AppLog.WriteSync("RustStage1: Start(" + w + "x" + h + " @ " + scale.ToString("0.0") + ") -> " + ok
                + " " + RustRenderer.Diagnose());

            StageText.Text = ok ? "启动成功 · stage=0" : "启动失败 · " + RustRenderer.Diagnose();
            HrText.Text = ok
                ? "swapchain: " + RustRenderer.ScVariantName
                : (RustRenderer.ScStep > 0 ? "swapchain 步: " + RustRenderer.ScStepName : "");
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
