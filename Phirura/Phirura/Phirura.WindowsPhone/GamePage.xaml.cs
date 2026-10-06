using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Microsoft.Graphics.Canvas;
using Phirura.Core;
using Phirura.Game;
using Phirura.Parse;
using Windows.Graphics.Display;
using Windows.Phone.UI.Input;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Input;
using Windows.UI.Xaml.Media;
using Windows.UI.Xaml.Navigation;

namespace Phirura
{
    /// <summary>
    /// gameplay. Owns the chart, the judge, the renderer and the music clock, and drives
    /// them from CompositionTarget.Rendering (WP8.1 has no XAML-native game loop).
    /// </summary>
    public sealed partial class GamePage : Page, PrprPort.IMusicSink, PrprPort.ISfxSink, PrprPort.ITextInputSink
    {
        public static PlayResult LastResult;
        public static string LastChartTitle = string.Empty;

        private const double StartDelay = 1.6;

        private ChartEntry _entry;
        private Chart _chart;
        private Judge _judge;
        private Win2DChartRenderer _renderer;
        private MusicClock _clock;
        /// <summary>
        /// How many times the music element's own state event fired. `sfxCut` samples once per
        /// sound and can miss a flip that reverses inside the same second; this counts the event
        /// itself, so nothing in between is invisible.
        /// </summary>
        private int _musicStateFlips;

        /// <summary>
        /// Whether the engine last asked for the music to be running.
        ///
        /// `MusicPlay` and `MusicPause` are the only *intentional* writers of the element's
        /// state, so they record the intent here and <see cref="GuardMusic"/> is the only thing
        /// that acts on the gap between that intent and `Player.CurrentState`. Without a flag
        /// the guard cannot tell a sound that knocked the track out from a pause the player
        /// asked for, and restarting the second one would make the pause button useless.
        /// </summary>
        private bool _musicWantPlaying;

        /// <summary>
        /// How many times <see cref="GuardMusic"/> had to restart the track. Reported next to
        /// `sfxCut` because the two answer halves of one question: `sfxCut` counts the moments
        /// the state *changed*, this counts the ones where nothing else would have put it back.
        /// </summary>
        private int _musicGuardRescues;

        private int _musicGuardLogTick = int.MinValue / 2;

        /// <summary>
        /// Throttle for the <see cref="GuardMusic"/> restart attempt. `Play()` on an element
        /// that stays stuck is cheap but not free, and once per frame at 60 fps would bury the
        /// log line that says it is happening - 250 ms is short enough that a real knock-out
        /// costs at most a quarter second of silence and long enough that one stuck element
        /// produces four log lines a second rather than sixty.
        /// </summary>
        private int _musicGuardLastTry = int.MinValue / 2;

        /// <summary>
        /// The last position the music element reported as having *moved*, or -1 before the
        /// first sample.
        ///
        /// The state check alone cannot see the failure that actually happens: on device the
        /// element sat in <c>Playing</c> with its position frozen at 10.27 s for 280 s, so a
        /// guard that only reacts to <c>Paused</c>/<c>Stopped</c> reported "nothing wrong"
        /// throughout. State says what the pipeline intends; position says whether it is doing
        /// it, and only the second one is audible.
        /// </summary>
        private double _musicLastPos = -1.0;

        /// <summary>
        /// <see cref="Environment.TickCount"/> of the first sample that found the position
        /// unchanged, or 0 when it is advancing. Zero rather than a timestamp because a real
        /// tick count of 0 is indistinguishable from "not stalled" only in the next call, and
        /// the next call happens within a quarter of a second.
        /// </summary>
        private int _musicStallTick;

        /// <summary>
        /// 0 = nothing tried yet, 1 = seeked onto the clock's position, 2 = bounced the
        /// pipeline with Pause/Play. The two are deliberately separate attempts rather than
        /// one combined one: a seek alone un-wedges most stalls and preserves where the song
        /// was, and the Pause/Play that follows is the blunter instrument for the ones it does
        /// not, so paying for both every time would trade a cheap correction for a loud one.
        /// </summary>
        private int _musicStallStage;

        /// <summary>How many stalls were detected, for the per-second frame line.</summary>
        private int _musicStalls;

        private GameSettings _settings;
        private SfxPlayer _sfx;

        // Where phira's own library copy of the chart sits, as an absolute path.
        private static string PhiraChartDir;

        // ms-appdata URI of that copy's music, or null when no music file was found.
        // Resolved before the boot rather than on demand: music_new arrives on the frame
        // thread as a clip id and nothing else - the bytes are Rust's and never reach the
        // shell - so the one thing a MediaElement needs, a source, has to be waiting here.
        private static string PhiraMusicUri;

        // ms-appdata URI of the res pack's `ending` clip - what prpr asks for once the chart is
        // over (`ending.rs:90`, created with `loop_mix_time: 0.`). Null when no such file was
        // shipped, in which case the result page stays silent instead of replaying the chart.
        private static string PhiraEndingMusicUri;

        // The source handed to the player by the most recent `MusicNew`, so `OnMediaOpened` can
        // tell which of the two URIs just produced a duration.
        private static string ActiveMusicUri;

        // Seconds of whatever `PhiraMusicUri` points at, taken from the first MediaOpened that
        // was opened *for the chart*. This is the only handle the shell has on clip identity:
        // `AudioClip` hands every clip id 0, so the only way to tell the chart's own music from
        // the res pack's `ending` is to ask for a clip of a different length. 0 means nothing
        // has opened yet - which is the very first music_new, the song page's preview - and is
        // therefore read as the chart.
        private static double PhiraMusicSeconds;

        private readonly List<BadNote> _badNotes = new List<BadNote>();
        private readonly List<Judge.JudgementEvent> _events = new List<Judge.JudgementEvent>();
        private readonly Dictionary<uint, TouchInput> _active = new Dictionary<uint, TouchInput>();
        private readonly HashSet<uint> _justStarted = new HashSet<uint>();
        private readonly List<uint> _toRemove = new List<uint>();

        /// <summary>Scratch for the finished-touch sweep in `PushRustTouches`; never allocated per frame.</summary>
        private readonly List<uint> _retire = new List<uint>();
        private readonly List<TouchInput> _frameTouches = new List<TouchInput>();

        private bool _hooked;
        private bool _paused;
        private bool _finished;
        private bool _mediaReady;
        private bool _started;
        private DateTime _startedUtc;
        private bool _immersive;
        private float _aspectRatio = 16f / 9f;
        private int _frameCount;
        private double _judgeTextUntil;
        private string _judgeText = string.Empty;

        // --- Rust engine (M3) ----------------------------------------------
        //
        // `prpr` runs its own judge and draws its own HUD (`scene/game.rs` reads
        // `self.judge.score()` at :432 and `self.judge.combo()` at :468), so the C# judge and the
        // C# chart renderer cannot stay on this path: that would be two judges, two HUDs and two
        // sets of hit sounds. The C# half is kept behind `UseRustEngine` because it is the only
        // reference implementation to compare a first device run against - without it there is no way
        // to tell "the engine is wrong" from "the wiring is wrong".
        private const bool UseRustEngine = true;

        /// Viewport in **pixels**, which is what `wp81_game_set_screen` wants: `prpr` projects
        /// through `screen_width`/`screen_height`, and the recorded frame is 768x1229 px while
        /// `GameSurface.ActualWidth` is in DIPs.
        private double _rustScale = 1.0;
        private uint _rustW;
        private uint _rustH;
        private bool _rustCreated;

        /// Chart time of each finger's press, for the Rust path only.
        ///
        /// `TouchInput.Time` cannot be reused: `Judge.cs:368` reads it and treats `NegativeInfinity`
        /// as "now", so writing a real time into it would silently change the C# judge on the
        /// fallback path. `prpr` instead ages a touch with `t - (uptime - time) * speed`, which
        /// needs the moment the finger actually went down - up to a frame of error otherwise.
        private readonly Dictionary<uint, double> _rustPressTime = new Dictionary<uint, double>();

        // --- phira mode -----------------------------------------------------
        //
        // Phira mode is what "launched without a chart" means. `App.OnLaunched` navigates here
        // with `MainPage.PendingChart` still null, and this page then runs phira's own front end
        // instead of a chart from the list.
        //
        // `_chart`, `_judge` and `_clock` stay null for the entire session, which is why every one
        // of them sits behind this flag rather than merely behind an "unlikely" branch: a phira
        // build that reached `_chart.Duration` or `_clock.Resync` would throw in the render loop,
        // i.e. inside `CompositionTarget.Rendering`, where the exception takes down the frame loop
        // rather than the operation that caused it.
        private bool _phiraMode;

        /// phira's timeline in seconds - the value `host::CLOCK` holds, advanced by `dt`.
        ///
        /// The chart path takes this from `MusicClock`, anchored to the audio. There is no audio
        /// on this path: phira's menu, its animations and prpr's note timings are all integrated
        /// from `dt`, so the shell is the one keeping the number. Touch stamps come from here for
        /// the same reason `prpr` ages a press with `t - (uptime - time) * speed` - the press time
        /// and the frame time have to be counting the same seconds, or every judgement is late by
        /// however long ago the finger went down.
        private double _phiraTime;
        private DateTime _phiraLastFrameUtc;

        /// The chart phira opens on launch, spelled the way phira's `LocalChart::local_path` spells
        /// it - relative to `data/charts`, never a path. `EnsureBundledChartAsync` derives the
        /// packaged folder from the segment after the last slash, so the two cannot drift into
        /// "the app ships a chart and phira is asked for a different one".
        private const string PhiraAutoChart = "custom/190";

        /// Whether phira opens that chart by itself, instead of stopping on its own home screen.
        ///
        /// Off. This is what "get it to the main screen" needed: with it on, the boot drove straight
        /// past `home.rs` into the song, so the home screen was never reachable and every tap landed
        /// on a page under autoplay.
        ///
        /// It is safe to turn off even though the shell resolves the music from `PhiraAutoChart`
        /// alone: the library contains exactly that one chart - the one the shell seeds - so whatever
        /// the player picks *is* that chart, and the music is right by construction. **That stops
        /// being true the moment a second chart can be imported**, and then the music has to travel
        /// with the chart the engine actually opened: carry the name across `wp81_audio_music_new`
        /// the same way the sfx name now rides `wp81_audio_sfx_new`.
        private const bool PhiraAutoOpen = false;

        /// Whether phira starts that chart rather than stopping on its song page. Kept separate
        /// from *which* chart because they change for different reasons: which chart is what was
        /// packaged, this is whether the app drops the player straight into playing it.
        ///
        /// This was `true`, and together with the auto-open above it is why the app appeared to have
        /// no working buttons: the boot drove past `home.rs` into the song, and then **autoplay
        /// played it** - and autoplay does not take input, so every tap was ignored by design.
        /// Neither the home screen nor the buttons were ever broken; the app never stopped on the
        /// home screen, and the one screen it did reach was under autoplay.
        ///
        /// Left off, and so is the auto-open above it. The comment used to claim auto-open stayed on
        /// so that the music would follow `PhiraAutoChart`; both are false now, and the risk that
        /// note described is still real: the shell resolves the music from `PhiraAutoChart` and
        /// nothing else, so a *second* imported chart would play this one's music. Fix that by
        /// carrying the name across `wp81_audio_music_new`, the way the sfx name rides
        /// `wp81_audio_sfx_new` - not by leaving auto-open switched on.
        private const bool PhiraAutoPlay = false;

        public GamePage()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Disabled;
        }

        // ------------------------------------------------------------------
        // Lifecycle
        // ------------------------------------------------------------------

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            HardwareButtons.BackPressed += OnBackPressed;
            DisplayInformation.AutoRotationPreferences = DisplayOrientations.Landscape | DisplayOrientations.LandscapeFlipped;
            _immersive = true;
            // OnNavigatedTo can fire before the page is realised, so this first attempt is
            // best-effort; OnPageLoaded and OnWindowActivated re-apply it.
            SetImmersive(true);
            Window.Current.Activated += OnWindowActivated;
            Application.Current.Resuming += OnAppResuming;
            _entry = MainPage.PendingChart;
            // `null` is both "cold start" and "nobody picked anything", and on this page they mean
            // the same thing: run phira. The timeline is reset here too rather than in the field
            // initializer, because it has to be re-zeroed on every arrival - the back-and-forth
            // between the chart list and this page reuses the same navigation stack, and a second
            // phira run inheriting the first one's accumulated time would start every judgement
            // against a clock that never went back to zero.
            _phiraMode = (_entry == null);
            _phiraTime = 0.0;
            _settings = GameSettings.Load();
            this.Loaded += OnPageLoaded;
        }

        protected override void OnNavigatedFrom(NavigationEventArgs e)
        {
            base.OnNavigatedFrom(e);
            HardwareButtons.BackPressed -= OnBackPressed;
            DisplayInformation.AutoRotationPreferences = DisplayOrientations.Portrait;
            _immersive = false;
            Window.Current.Activated -= OnWindowActivated;
            Application.Current.Resuming -= OnAppResuming;
            SetImmersive(false);
            StopLoop();
            // The log refresh rides a DispatcherTimer, which survives navigation unless it is
            // stopped - and a timer still ticking against a page that has been torn down keeps the
            // page (and its whole renderer) alive.
            if (_logTimer != null)
            {
                _logTimer.Stop();
            }
            if (_calibrating)
            {
                EndCalibration();
            }
            if (_sfx != null)
            {
                _sfx.Stop();
                _sfx = null;
            }
            if (_renderer != null)
            {
                _renderer.Detach();
                _renderer = null;
            }
            // `host::GAME` is a `thread_local!`, so this has to be the thread that created it - which
            // is this one, because `StartRustEngine` ran on the UI thread and so did every frame.
            if (UseRustEngine && _rustCreated)
            {
                try
                {
                    PrprPort.GameDestroy();
                }
                catch (Exception)
                {
                }
                _rustCreated = false;

                // After `GameDestroy`, so no frame can still be inside a callback whose sink is
                // the page being torn down - and cleared rather than left pointing, because the
                // table is static and outlives this page by a long way.
                PrprPort.TextInputSink = null;
            CancelOpenTextInput();
            PrprPort.MusicSink = null;
                PrprPort.SfxSink = null;
            }
            try
            {
                Player.Stop();
                Player.Source = null;
            }
            catch (Exception)
            {
            }
        }

        /// <summary>
        /// Takes the page genuinely full-screen, and restores the system chrome on exit.
        ///
        /// Four independent things have to line up on WP8.1, and each one is applied in its
        /// own try block so a failure in one cannot silently skip the others. That was the
        /// original bug: the colour setters ran before the hide, so when they threw the
        /// hide never happened and the status bar stayed on screen while the failure
        /// disappeared into the log.
        ///
        ///  1. ApplicationView.SuppressSystemOverlays - the Windows Phone specific switch
        ///     that suppresses the system tray. Synchronous, so it cannot be skipped.
        ///  2. SetDesiredBoundsMode(UseCoreWindow)    - lets the layout use the whole
        ///     window instead of the area left over above the tray.
        ///  3. StatusBar.HideAsync()                  - the documented hide.
        ///  4. BackgroundOpacity = 0                  - belt and braces: a zero-opacity tray
        ///     is invisible even if the hide is refused.
        ///
        /// Note that Windows.UI.Xaml.WindowActivatedEventArgs does not exist on this
        /// profile, and neither do ApplicationView.IsFullScreenMode / TryEnterFullScreenMode
        /// (those are desktop-only). Everything used here was compile-checked against the
        /// WP8.1 reference set.
        /// </summary>
        private void SetImmersive(bool on)
        {
            // Fire-and-forget for the lifecycle hooks; OnPageLoaded awaits the real one so
            // that the layout has settled before the renderer measures itself.
#pragma warning disable 4014
            SetImmersiveAsync(on);
#pragma warning restore 4014
        }

        private async Task SetImmersiveAsync(bool on)
        {
            Windows.UI.ViewManagement.ApplicationView view = null;
            try
            {
                view = Windows.UI.ViewManagement.ApplicationView.GetForCurrentView();
            }
            catch (Exception ex)
            {
                AppLog.Write("ApplicationView.GetForCurrentView", ex);
            }

            if (view != null)
            {
                try
                {
                    view.SuppressSystemOverlays = on;
                }
                catch (Exception ex)
                {
                    AppLog.Write("SuppressSystemOverlays=" + on, ex);
                }

                try
                {
                    view.SetDesiredBoundsMode(on
                        ? Windows.UI.ViewManagement.ApplicationViewBoundsMode.UseCoreWindow
                        : Windows.UI.ViewManagement.ApplicationViewBoundsMode.UseVisible);
                }
                catch (Exception ex)
                {
                    AppLog.Write("SetDesiredBoundsMode", ex);
                }
            }

            Windows.UI.ViewManagement.StatusBar bar = null;
            try
            {
                bar = Windows.UI.ViewManagement.StatusBar.GetForCurrentView();
            }
            catch (Exception ex)
            {
                AppLog.Write("StatusBar.GetForCurrentView", ex);
            }

            if (bar == null)
            {
                AppLog.Write("StatusBar: GetForCurrentView() returned null");
                return;
            }

            try
            {
                if (on)
                {
                    await bar.HideAsync();
                }
                else
                {
                    await bar.ShowAsync();
                }
            }
            catch (Exception ex)
            {
                AppLog.Write(on ? "StatusBar hide" : "StatusBar show", ex);
            }

            try
            {
                bar.BackgroundOpacity = on ? 0.0 : 1.0;
            }
            catch (Exception ex)
            {
                AppLog.Write("StatusBar.BackgroundOpacity", ex);
            }

            if (on)
            {
                try
                {
                    bar.ForegroundColor = Windows.UI.Colors.White;
                    bar.BackgroundColor = Windows.UI.Colors.Black;
                }
                catch (Exception ex)
                {
                    // Cosmetic only — never let it undo the hide above.
                    AppLog.Write("StatusBar colours", ex);
                }
            }

            // Read the result back. OccludedRect is the area the bar actually covers, so
            // this single log line says whether the page is really full-screen (height 0)
            // or whether the system refused the hide.
            try
            {
                Windows.Foundation.Rect r = bar.OccludedRect;
                AppLog.Write("StatusBar state: immersive=" + on
                    + " occluded=" + r.Width + "x" + r.Height);
            }
            catch (Exception ex)
            {
                AppLog.Write("StatusBar.OccludedRect", ex);
            }
        }

        /// <summary>
        /// Waits one layout pass.
        ///
        /// Hiding the status bar grows the page's content area, and the renderer's
        /// projection is derived from that size, so measuring before the re-layout would
        /// squash the chart vertically and put the judge line in the wrong place.
        /// Completes on the first SizeChanged, or on the next low-priority dispatcher turn
        /// if the size never changes (the bar was already hidden).
        /// </summary>
        private Task WaitForLayoutAsync()
        {
            TaskCompletionSource<bool> tcs = new TaskCompletionSource<bool>();
            SizeChangedEventHandler handler = null;
            handler = delegate(object s, SizeChangedEventArgs args)
            {
                this.SizeChanged -= handler;
                tcs.TrySetResult(true);
            };
            this.SizeChanged += handler;

            Windows.UI.Core.CoreDispatcher d = this.Dispatcher;
            if (d == null)
            {
                this.SizeChanged -= handler;
                tcs.TrySetResult(true);
                return tcs.Task;
            }
#pragma warning disable 4014
            d.RunAsync(Windows.UI.Core.CoreDispatcherPriority.Low, delegate
            {
                this.SizeChanged -= handler;
                tcs.TrySetResult(true);
            });
#pragma warning restore 4014
            return tcs.Task;
        }

        /// <summary>
        /// The status bar comes back on its own when the app is reactivated, so re-hide it
        /// whenever the window regains focus while gameplay is on screen.
        ///
        /// The WP8.1 projection does NOT expose Windows.UI.Xaml.WindowActivatedEventArgs.
        /// Window.Activated is instead projected with the Core args type, so the handler has
        /// to be declared against Windows.UI.Core.WindowActivatedEventArgs, and the state
        /// enum is spelled CoreWindowActivationState. Both facts were confirmed by compiling
        /// the exact signatures against the WP8.1 reference set.
        /// </summary>
        private void OnWindowActivated(object sender, Windows.UI.Core.WindowActivatedEventArgs e)
        {
            if (_immersive && e.WindowActivationState != Windows.UI.Core.CoreWindowActivationState.Deactivated)
            {
                SetImmersive(true);
            }
        }

        /// <summary>
        /// Second safety net for the same problem: resuming from a suspended state does not
        /// always raise Window.Activated, and the status bar is restored across suspension.
        /// </summary>
        private void OnAppResuming(object sender, object e)
        {
            if (_immersive)
            {
                SetImmersive(true);
            }
        }

        private void OnBackPressed(object sender, BackPressedEventArgs e)
        {
            e.Handled = true;
            // phira mode: there is no `_clock` to pause against, phira owns its own screens, and
            // the XAML pause overlay is collapsed - so `DoPause` here would raise a menu over a
            // game that keeps running underneath it. Back leaves instead. Going *forward* to
            // `MainPage` rather than only `GoBack` matters because this page can be the first one
            // in the stack (that is how a launch arrives), where `CanGoBack` is false.
            if (_phiraMode)
            {
                if (this.Frame.CanGoBack)
                {
                    this.Frame.GoBack();
                }
                else
                {
                    this.Frame.Navigate(typeof(MainPage));
                }
                return;
            }
            if (!_paused && !_finished)
            {
                DoPause();
            }
            else if (this.Frame.CanGoBack)
            {
                this.Frame.GoBack();
            }
        }

        private async void OnPageLoaded(object sender, RoutedEventArgs e)
        {
            this.Loaded -= OnPageLoaded;
            // Hide the status bar and let the content area grow BEFORE measuring: the
            // renderer and the judge both derive their geometry from that size.
            await SetImmersiveAsync(true);
            await WaitForLayoutAsync();
            await StartAsync();
        }

        // ------------------------------------------------------------------
        // Setup
        // ------------------------------------------------------------------

        private async Task StartAsync()
        {
            // phira mode has no chart to parse, so it is decided first: the `_entry` guard below
            // would otherwise reject the launch outright, which is exactly the "no chart selected"
            // screen this page used to show.
            if (_phiraMode)
            {
                // phira reads its own library with real `std::fs`, not through `load_asset`, and it
                // scans that library exactly once - during construction, inside `PhiraBoot`. A chart
                // that exists only as packaged `ms-appx:` content is therefore invisible to it
                // however well the shell can read it, so the bytes have to be on disk before this
                // call, which never yields control back.
                LoadingText.Text = "正在内置谱面…";
                try
                {
                    await EnsureBundledAssetsAsync();
                    await EnsureBundledChartAsync();
                }
                catch (Exception ex)
                {
                    // Worth failing loudly. The alternative symptom is phira opening with an empty
                    // library, the auto-open finding nothing, and the log showing a successful boot
                    // that did nothing - three separate observations to reconcile into one cause.
                    AppLog.Write("GamePage.EnsureBundledChart", ex);
                    LoadingText.Text = "内置谱面失败：" + ex.Message;
                    return;
                }
                // The sound-effect voices, created here as well as on the chart path.
                //
                // It used to exist only there, so in phira mode `_sfx` was null and `SfxPlay`
                // returned before playing anything - which is why the hit sounds stayed silent even
                // after the callbacks were wired to a real sink. The sink was fine; the device
                // behind it was never built.
                _sfx = new SfxPlayer(SfxHost);
                _sfx.Volume = _settings == null ? 1.0 : _settings.VolumeSfx;
                _sfx.Enabled = true;
                _sfx.Music = Player;

                // Before `StartPhiraAsync`, which never yields: the music URI has to exist by the
                // time phira first asks for the music, and phira asks from the frame thread.
                await ResolvePhiraMusicAsync();
                StartPhiraAsync();
                return;
            }
            if (_entry == null)
            {
                LoadingText.Text = "没有选中的谱面。";
                return;
            }
            try
            {
                LoadingText.Text = "正在解析谱面…";
                string chartText = await ChartLibrary.ReadTextAsync(_entry.BasePath, _entry.ChartFile);
                string ext = _entry.ChartFile.Substring(_entry.ChartFile.LastIndexOf('.') + 1);
                List<string> warnings = new List<string>();
                _chart = ChartParser.Parse(chartText, ext, warnings);
                _entry.ParsedChart = _chart;
                _entry.Warnings = warnings;

                if (_chart.Lines.Count == 0 || _chart.NoteCount() == 0)
                {
                    LoadingText.Text = "这个谱面没有可玩的音符。";
                    return;
                }

                LoadingText.Text = "正在载入贴图…";
                // Textures go on the shared device, which is also what GameCanvas runs on
                // (UseSharedDevice="True"), so the control can draw bitmaps loaded here.
                CanvasDevice device = CanvasDevice.GetSharedDevice();
                ChartTexture click = await ChartLibrary.LoadCanvasTextureAsync(device, "ms-appx:///Assets/game", "click.png");
                ChartTexture drag = await ChartLibrary.LoadCanvasTextureAsync(device, "ms-appx:///Assets/game", "drag.png");
                ChartTexture flick = await ChartLibrary.LoadCanvasTextureAsync(device, "ms-appx:///Assets/game", "flick.png");
                // hold.png is Phira's real single-sheet hold atlas (holdAtlas: [50, 50]);
                // hit_fx.png is its 8x7 hit-particle atlas.
                ChartTexture hold = await ChartLibrary.LoadCanvasTextureAsync(device, "ms-appx:///Assets/game", "hold.png");
                ChartTexture hitFx = await ChartLibrary.LoadCanvasTextureAsync(device, "ms-appx:///Assets/game", "hit_fx.png");

                // Judge-line textures referenced by the chart (illustrations, custom lines).
                LoadingText.Text = "正在载入谱面贴图…";
                for (int i = 0; i < _chart.Lines.Count; i++)
                {
                    JudgeLine line = _chart.Lines[i];
                    if (line.Kind != JudgeLineKindType.Texture || string.IsNullOrEmpty(line.TexturePath))
                    {
                        continue;
                    }
                    if (_loadedLineTextures.Contains(line.TexturePath))
                    {
                        continue;
                    }
                    ChartTexture tex = await ChartLibrary.TryLoadCanvasTextureAsync(device, _entry.BasePath, line.TexturePath);
                    if (tex != null)
                    {
                        _lineTextures[line.TexturePath] = tex;
                        _loadedLineTextures.Add(line.TexturePath);
                    }
                }

                double w = GameSurface.ActualWidth;
                double h = GameSurface.ActualHeight;
                if (w < 10 || h < 10)
                {
                    w = Window.Current.Bounds.Width;
                    h = Window.Current.Bounds.Height;
                }
                _aspectRatio = (float)Math.Min(16.0 / 9.0, w / h);

                _renderer = new Win2DChartRenderer(GameCanvas, w, h, _aspectRatio, click, drag, flick, hold, hitFx);
                _renderer.FlipVertical = _settings.FlipVertical;
                _renderer.NoteWidth = (float)(Consts.NoteWidthRatioBase * _settings.NoteScale);
                // prpr sizes the hit spark from the user's note scale
                // (hit_fx_scale * note_scale / 5); keep the same relationship here.
                _renderer.NoteScale = _settings.NoteScale;
                // prpr's default line stroke is 0.01 chart units, scaled by the user setting.
                _renderer.LineThickness = 0.01 * _settings.LineThickness;
                foreach (KeyValuePair<string, ChartTexture> kv in _lineTextures)
                {
                    _renderer.RegisterLineTexture(kv.Key, kv.Value);
                }

                // Hit sounds. WP8.1 has no low-latency audio API for C#, so this is a pool of
                // MediaElements parented into the hidden SfxHost panel.
                _sfx = new SfxPlayer(SfxHost);
                _sfx.Volume = _settings.VolumeSfx;
                _sfx.Enabled = true;
                _sfx.Music = Player;

                _judge = new Judge(_chart);
                _judge.AspectRatio = _aspectRatio;

                _clock = new MusicClock(Player);
                HookMusicStateEvents();
                _clock.Offset = -StartDelay - _chart.Offset + _settings.Offset;
                Player.Volume = _settings.VolumeMusic;

                if (UseRustEngine)
                {
                    StartRustEngine();
                }

                SongText.Text = _entry.Title + "  ·  " + _entry.LevelDisplay;

                // Positive-path trace. Without this the log says nothing between "launched" and
                // the first exception, which makes a healthy run indistinguishable from a run
                // that silently stalled.
                int noteCount = 0;
                for (int i = 0; i < _chart.Lines.Count; i++)
                {
                    noteCount += _chart.Lines[i].Notes.Count;
                }
                AppLog.Write("GamePage: loaded \"" + _entry.Title + "\" format=" + _chart.SourceFormat
                    + " lines=" + _chart.Lines.Count + " notes=" + noteCount
                    + " duration=" + _chart.Duration.ToString("0.0") + "s"
                    + " audio=\"" + _entry.AudioFile + "\" ar=" + _aspectRatio.ToString("0.###"));

                LoadingOverlay.Visibility = Visibility.Collapsed;
                StartPlayback();
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.StartAsync", ex);
                LoadingText.Text = "载入失败：" + ex.Message;
            }
        }

        /// <summary>
        /// Boots phira's own front end instead of parsing a chart.
        ///
        /// Everything the chart path does above is either phira's job or not on this path: phira
        /// loads its own textures through `load_texture`, owns its own `Judge` and HUD, and has no
        /// music to anchor a `MusicClock` to. So the chart parse, the five note textures, the
        /// judge-line textures, `_judge` and `_clock` are all simply absent, and what remains is
        /// the geometry, the renderer that rasterises the port frame, and the engine.
        ///
        /// Synchronous, and called without `await`, because everything in it is: `PhiraBoot`
        /// blocks on the UI thread by design (see its own doc - it polls phira's `the_main` until
        /// it parks, and phira is served entirely by synchronous shell hooks), so there is no point
        /// pretending to yield.
        /// </summary>
        private void StartPhiraAsync()
        {
            try
            {
                LoadingText.Text = "正在启动 phira…";

                double w = GameSurface.ActualWidth;
                double h = GameSurface.ActualHeight;
                if (w < 10 || h < 10)
                {
                    w = Window.Current.Bounds.Width;
                    h = Window.Current.Bounds.Height;
                }
                _aspectRatio = (float)Math.Min(16.0 / 9.0, w / h);

                // Null textures. The five are what the *C#* chart path draws with, and `OnDraw`
                // cannot reach that path while a port frame is set - it replays the port frame and
                // returns - nor crash when there is neither, because it bails out on
                // `_chart == null` before touching any of them. The constructor tolerates the
                // nulls too: `BitmapOf(null)` is null and `Tint` only runs behind a `!= null`, so
                // this also drops five PNG decodes between the launch and the first frame.
                _renderer = new Win2DChartRenderer(GameCanvas, w, h, _aspectRatio, null, null, null, null, null);
                _renderer.FlipVertical = _settings.FlipVertical;
                _renderer.NoteWidth = (float)(Consts.NoteWidthRatioBase * _settings.NoteScale);
                _renderer.NoteScale = _settings.NoteScale;
                _renderer.LineThickness = 0.01 * _settings.LineThickness;

                // prpr draws all three of these itself, and the XAML copies of them are only ever
                // written from the chart branch of `OnRendering` - so left up they would sit on
                // top of phira's HUD at whatever they were initialised to ("0", "100.00%").
                HudRoot.Visibility = Visibility.Collapsed;
                PauseButton.Visibility = Visibility.Collapsed;

                // Installed before the engine exists: `PhiraBoot` blocks until phira parks and
                // never yields, so a sink installed afterwards would miss the first `music_new`.
                PrprPort.MusicSink = this;
                // The other half of the same wiring. Without it `sfx_new`/`sfx_play` only logged, so
                // every sound phira asked for - the note hits above all - was silent by construction.
                PrprPort.SfxSink = this;
                // Text input goes the same way: phira's `request_input` lands in
                // `HostCallbacks.text_input`, which hands the request here. Installed alongside
                // the other two because a login or search prompt can be opened the instant
                // `MainScene` is up - which is while `PhiraBoot` still holds the thread.
                PrprPort.TextInputSink = this;

                StartRustEngine();
                if (!_rustCreated)
                {
                    LoadingText.Text = "phira 启动失败：" + PrprPort.GameLastError();
                    return;
                }

                // Before the first frame: the overlay is a sibling of `GameSurface`, so it would
                // otherwise cover whatever phira is drawing behind it.
                LoadingOverlay.Visibility = Visibility.Collapsed;
                AppLog.Write("GamePage: phira mode, ar=" + _aspectRatio.ToString("0.###")
                    + " screen=" + _rustW + "x" + _rustH + " px @ " + _rustScale.ToString("0.0"));
                _phiraLastFrameUtc = DateTime.UtcNow;
                HookLoop();
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.StartPhiraAsync", ex);
                LoadingText.Text = "启动失败：" + ex.Message;
            }
        }

        /// <summary>
        /// Copies the shell's own copy of the chart into phira's library, and returns its
        /// <c>local_path</c>.
        ///
        /// The two halves of this app keep charts in different places on purpose. The shell reads
        /// them from <c>LocalFolder\charts\&lt;id&gt;</c> through <c>load_asset</c> (see
        /// <see cref="ChartLibrary.LocalFolderName"/>), because those are files the shell imported.
        /// phira looks in exactly two places of its own - <c>data/charts/custom</c> and
        /// <c>data/charts/download/&lt;id&gt;</c> - and it looks with real <c>std::fs</c>, because
        /// those are *its* files. Neither side can be pointed at the other's: phira's
        /// <c>local_path</c> is resolved relative to <c>dir::charts()</c>, so a path that did not
        /// sit underneath <c>data/charts</c> would scan once and then fail to reopen itself on every
        /// later use. The bytes therefore have to be copied, once, before <c>PhiraBoot</c> - which
        /// never yields control - runs and <c>Data::init</c> scans.
        ///
        /// The destination is derived from <see cref="PhiraAutoChart"/> rather than spelled out, so
        /// the folder copied and the path handed to <c>PhiraBoot</c> are the same string by
        /// construction. The failure this avoids is shipping a chart under one name and asking
        /// phira for another, which presents as "it opened the menu instead of the chart".
        ///
        /// <summary>
        /// Seeds `LocalFolder\charts` from the copy packaged inside the appx, once.
        ///
        /// phira reads its assets with real `std::fs` at paths under `LocalFolder\charts`, and the
        /// chart bootstrap below reads the same tree - so bytes that exist only as packaged
        /// `ms-appx:` content are invisible to both. This is what makes `stage-chart.ps1 -Push`
        /// unnecessary: identical layout, identical consumers, only the source of the bytes changed.
        ///
        /// A marker file makes it idempotent, and its absence is the only trustworthy signal - a
        /// half-copied tree counts the same as a whole one. `rs` replaces LocalFolder, which wipes
        /// both the tree and the marker, so a pushed build simply re-seeds on the next launch.
        /// </summary>
        private static async Task EnsureBundledAssetsAsync()
        {
            Windows.Storage.StorageFolder local = Windows.Storage.ApplicationData.Current.LocalFolder;
            Windows.Storage.StorageFolder dest = await local.CreateFolderAsync(
                "charts", Windows.Storage.CreationCollisionOption.OpenIfExists);
            try
            {
                await dest.GetFileAsync(".seeded");
                return;
            }
            catch (Exception)
            {
                // Not seeded (or wiped by a push) - fall through and copy.
            }

            Windows.Storage.StorageFolder pkg =
                await Windows.ApplicationModel.Package.Current.InstalledLocation.GetFolderAsync("Assets");
            pkg = await pkg.GetFolderAsync("charts");

            int n = await CopyTreeAsync(pkg, dest);
            await dest.CreateFileAsync(".seeded", Windows.Storage.CreationCollisionOption.ReplaceExisting);
            AppLog.WriteSync("GamePage: seeded " + n + " packaged assets -> " + dest.Path);
        }

        /// <summary>Recursive copy. `StorageFolder` has no one-call equivalent on this profile.</summary>
        private static async Task<int> CopyTreeAsync(Windows.Storage.StorageFolder src,
                                                     Windows.Storage.StorageFolder dest)
        {
            int n = 0;
            foreach (Windows.Storage.StorageFile file in await src.GetFilesAsync())
            {
                await file.CopyAsync(dest, file.Name, Windows.Storage.NameCollisionOption.ReplaceExisting);
                n++;
            }
            foreach (Windows.Storage.StorageFolder sub in await src.GetFoldersAsync())
            {
                Windows.Storage.StorageFolder child = await dest.CreateFolderAsync(
                    sub.Name, Windows.Storage.CreationCollisionOption.OpenIfExists);
                n += await CopyTreeAsync(sub, child);
            }
            return n;
        }

        /// <summary>
        /// Copies the bundled chart into phira's own library, once per install.
        ///
        /// The completion marker - not "the folder is non-empty" - is what makes it idempotent: a
        /// copy interrupted by a crash leaves a full-looking folder missing the largest file, and
        /// the next launch would skip it forever. ~15 MB is copied exactly once per install.
        /// </summary>
        private static async Task<string> EnsureBundledChartAsync()
        {
            int cut = PhiraAutoChart.LastIndexOf('/');
            string folder = PhiraAutoChart.Substring(cut + 1);
            string parent = PhiraAutoChart.Substring(0, cut);

            Windows.Storage.StorageFolder local = Windows.Storage.ApplicationData.Current.LocalFolder;
            Windows.Storage.CreationCollisionOption open = Windows.Storage.CreationCollisionOption.OpenIfExists;

            // Built one segment at a time: `StorageFolder.CreateFolderAsync` takes a single name on
            // this API surface, and a path with separators in it is not portable between the places
            // this could be pointed at.
            Windows.Storage.StorageFolder dest = await local.CreateFolderAsync("data", open);
            dest = await dest.CreateFolderAsync("charts", open);
            dest = await dest.CreateFolderAsync(parent, open);
            dest = await dest.CreateFolderAsync(folder, open);

            // Recorded before the idempotency check: the music is resolved from this folder on
            // both the "already bundled" and the "just copied" path, and deriving it from
            // `PhiraAutoChart` a second time would be a second chance to get it wrong.
            PhiraChartDir = dest.Path;

            try
            {
                await dest.GetFileAsync(".bundled");
                return PhiraAutoChart;
            }
            catch (Exception)
            {
                // Not finished - fall through and copy. Absence of the marker is the only thing
                // that can be trusted, because a half-copied folder is indistinguishable from a
                // whole one by counting files.
            }

            // The shell's own copy, already on disk because this is where `ChartLibrary` imports
            // charts and where `-Push` puts them. Its absence is reported rather than ignored: the
            // auto-open would otherwise find nothing and phira would open on an empty library, which
            // is three separate observations to reconcile into one cause.
            Windows.Storage.StorageFolder src = await local.CreateFolderAsync("charts", open);
            try
            {
                src = await src.GetFolderAsync(folder);
            }
            catch (Exception)
            {
                throw new System.IO.FileNotFoundException(
                    "内置谱面缺失：charts\\" + folder + "（先用 stage-chart.ps1 -Push 推送）");
            }

            foreach (Windows.Storage.StorageFile file in await src.GetFilesAsync())
            {
                await file.CopyAsync(dest, file.Name, Windows.Storage.NameCollisionOption.ReplaceExisting);
            }
            await dest.CreateFileAsync(".bundled", Windows.Storage.CreationCollisionOption.ReplaceExisting);

            AppLog.Write("GamePage: chart copied into phira library -> " + dest.Path
                + " (" + (await dest.GetFilesAsync()).Count + " files)");
            return PhiraAutoChart;
        }

        private readonly Dictionary<string, ChartTexture> _lineTextures = new Dictionary<string, ChartTexture>();
        private readonly HashSet<string> _loadedLineTextures = new HashSet<string>();

        private void StartPlayback()
        {
            _started = true;
            _finished = false;
            _paused = false;
            _mediaReady = false;
            _startedUtc = DateTime.UtcNow;
            PauseOverlay.Visibility = Visibility.Collapsed;

            if (!string.IsNullOrEmpty(_entry.AudioFile))
            {
                try
                {
                    Player.Source = ChartLibrary.UriOf(_entry.BasePath, _entry.AudioFile);
                }
                catch (Exception ex)
                {
                    AppLog.Write("StartPlayback.Source", ex);
                }
            }
            else
            {
                // No audio to wait for, so nothing will ever raise MediaOpened.
                _mediaReady = true;
            }

            // StartFree rather than Start: WP8.1 ignores Position/Play until the element has
            // opened its source. The clock free-runs on the stopwatch until OnMediaOpened
            // re-anchors it and re-issues Play(). Without that handshake the first entry into
            // a chart ran silently until the player paused and resumed.
            _clock.StartFree();
            TryStartMedia();
            HookLoop();
        }

        /// <summary>
        /// Issues Play() and swallows the failure WP8.1 raises when the media is not open yet.
        /// OnMediaOpened calls this again once it is.
        /// </summary>
        private void TryStartMedia()
        {
            if (_paused || _finished || !_started)
            {
                return;
            }
            try
            {
                Player.Play();
            }
            catch (Exception ex)
            {
                AppLog.Write("Player.Play", ex);
            }
        }

        private void RestartPlayback()
        {
            StopLoop();
            try
            {
                Player.Stop();
            }
            catch (Exception)
            {
            }
            _chart.Reset();
            _judge.Reset();
            _badNotes.Clear();
            _events.Clear();
            _active.Clear();
            _justStarted.Clear();
            _toRemove.Clear();
            _mediaReady = false;
            _clock = new MusicClock(Player);
            HookMusicStateEvents();
            _clock.Offset = -StartDelay - _chart.Offset + _settings.Offset;
            StartPlayback();
        }

        private void HookLoop()
        {
            if (!_hooked)
            {
                CompositionTarget.Rendering += OnRendering;
                _hooked = true;
            }
        }

        private void StopLoop()
        {
            if (_hooked)
            {
                CompositionTarget.Rendering -= OnRendering;
                _hooked = false;
            }
        }

        // ------------------------------------------------------------------
        // Input
        // ------------------------------------------------------------------

        private TouchInput MakeTouch(uint id, TouchPhase phase, Windows.Foundation.Point p)
        {
            double w = GameSurface.ActualWidth;
            double h = GameSurface.ActualHeight;
            if (w < 1) w = 1;
            if (h < 1) h = 1;
            float nx = (float)(p.X / w * 2.0 - 1.0);
            float ny = (float)(p.Y / h * 2.0 - 1.0);
            TouchInput t = new TouchInput();
            t.Id = id;
            t.Phase = phase;
            t.Ndc = new Vec2(nx, ny);
            t.World = new Vec2(nx, -ny / _aspectRatio);
            t.Time = double.NegativeInfinity;
            return t;
        }

        // ------------------------------------------------------------------
        // Sound effects (phira's own)
        // ------------------------------------------------------------------

        /// <summary>
        /// The URI each registered effect should play, indexed by the handle `SfxPlay` receives.
        ///
        /// A table rather than a dictionary keyed by a real id, because there is no real id: every
        /// `AudioClip` on the Rust side reports 0, so the handle this shell returns is just the slot
        /// the name was recorded in.
        /// </summary>
        private readonly List<string> _sfxUris = new List<string>();

        /// <summary>
        /// Where an effect lives.
        ///
        /// Everything phira registers now resolves to a packaged `.wav`: the three note sounds,
        /// the calibration pair, and - as of this change - the three UI clicks as well. That last
        /// group used to fall through to `charts\<name>.ogg`, because that is where the seeded
        /// asset tree keeps them, and WP8.1's MediaElement refuses Vorbis outright
        /// (`MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED`, 0xC00D36C4). Every button press therefore
        /// logged `Catastrophic failure` from `MediaElement.Play()` and produced nothing: the
        /// main menu had no click sound at all, and the log said so six times over. The `.ogg`
        /// files are still the source; `button.wav`, `button_large.wav` and `switch.wav` are
        /// their decodable copies, made with ffmpeg and shipped in the package so they survive a
        /// `-Push` (which REPLACES LocalFolder).
        ///
        /// The default branch stays as it is for chart-supplied hit sounds, whose names cannot be
        /// known here - and a miss there is still worth reporting, because it yields a MediaFailed
        /// line naming the exact URI that failed.
        /// </summary>
        private static string SfxUri(string name)
        {
            if (string.IsNullOrEmpty(name))
            {
                return null;
            }
            string bare = name;
            int dot = bare.LastIndexOf('.');
            if (dot > 0)
            {
                bare = bare.Substring(0, dot);
            }
            switch (bare)
            {
                case "click":
                case "drag":
                case "flick":
                case "cali":
                case "cali_hit":
                case "button":
                case "button_large":
                case "switch":
                    return "ms-appx:///Assets/game/" + bare + ".wav";
                default:
                    return "ms-appdata:///local/charts/" + bare + ".ogg";
            }
        }

        uint PrprPort.ISfxSink.SfxNew(string name, double secs)
        {
            string uri = SfxUri(name);
            _sfxUris.Add(uri);
            AppLog.Write("GamePage: sfx_new '" + name + "' -> " + (uri == null ? "(none)" : uri));
            // The slot, unconditionally. Returning 0 for a URI that resolved to nothing made
            // that registration alias slot 0, so a silent effect played whatever the *first*
            // registered sound was - and if slot 0 was itself the null one, the first effect ever
            // registered became unplayable while every later one worked. A null entry now maps to
            // its own index, which `SfxPlay` already treats as "play nothing".
            return (uint)(_sfxUris.Count - 1);
        }

        void PrprPort.ISfxSink.SfxPlay(uint id, float amp)
        {
            if (id >= (uint)_sfxUris.Count || _sfx == null)
            {
                return;
            }
            string uri = _sfxUris[(int)id];
            if (uri == null)
            {
                return;
            }
            try
            {
                _sfx.Play(uri);
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.SfxPlay(" + uri + ")", ex);
            }
        }

        /// <summary>
        /// Whether a pointer event should reach the engine.
        ///
        /// `_started` is the C# chart path's flag - `StartPlayback` is its only writer - so a guard
        /// on it alone silently drops *every* tap in phira mode, where the page never starts a
        /// `MediaElement`-driven play of its own. That is precisely what "the screen does not
        /// respond to touch" was, and it was invisible because the guard returned before anything
        /// was recorded: the presses arrived and vanished with no trace. The phira path's
        /// equivalent of "started" is <c>_rustCreated</c> - the engine exists and its frame loop is
        /// running, which is the only precondition a touch actually needs.
        /// </summary>
        private bool InputLive
        {
            get
            {
                if (_paused || _finished)
                {
                    return false;
                }
                return UseRustEngine ? _rustCreated : _started;
            }
        }

        /// <summary>
        /// Whether a per-touch line may be written now.
        ///
        /// Every line costs six blocking WinRT file calls (`AppLog.Append` writes the Local store and
        /// the temp mirror), and the appends run as one serialised chain. A finger burst is five
        /// touches in twenty milliseconds - ten lines, sixty calls, seconds of queued work - and the
        /// next `WriteSync`, which runs on the UI thread once a second, waits for that whole queue.
        /// That is what froze the picture on touch while the engine kept running: the render loop was
        /// blocked, not stopped. Gating the burst keeps the signal and drops the pile-up.
        /// </summary>
        private bool TouchLogDue()
        {
            int now = Environment.TickCount;
            if (unchecked(now - _touchLogTick) < 200)
            {
                _touchLogSkipped++;
                return false;
            }
            _touchLogTick = now;
            return true;
        }

        private int _touchLogTick = int.MinValue / 2;
        private int _touchLogSkipped;

        private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
        {
            // Logged *before* the guard on purpose. A press that arrives and is dropped because the
            // page is still loading looks identical, in every other line, to a press that never
            // arrived at all - and that difference is the whole question behind "the screen does not
            // respond to touch". Rate-gated: see `TouchLogDue`.
            uint id = e.Pointer.PointerId;
            Windows.Foundation.Point where = e.GetCurrentPoint(GameSurface).Position;
            if (TouchLogDue())
            {
                AppLog.Write("GamePage: touch down id=" + id + " at " + where.X.ToString("0.#")
                    + "," + where.Y.ToString("0.#") + " rust=" + UseRustEngine
                    + " started=" + _started + " paused=" + _paused + " finished=" + _finished
                    + " live=" + InputLive + " skipped=" + _touchLogSkipped);
                _touchLogSkipped = 0;
            }

            if (!InputLive)
            {
                return;
            }
            _active[id] = MakeTouch(id, TouchPhase.Started, where);
            _justStarted.Add(id);
            // The Rust path's press timestamp. Kept out of `TouchInput.Time` because the C# judge
            // reads that field and treats `NegativeInfinity` as "now" (`Judge.cs:368`) - writing a
            // real time there would change the fallback path's behaviour. A stale entry is harmless:
            // the next press of the same id overwrites it before it can be read.
            if (UseRustEngine)
            {
                // Stamped from the *engine's* judgement clock, not the music clock or `_phiraTime`.
                // `prpr::judge` ages a touch with `t - (uptime - it.time) * speed`, where `uptime`
                // is the value `wp81_game_time` returns - the engine's own `CLOCK`, which
                // accumulates `dt` per frame. `_phiraTime` accumulates a separately measured `dt`
                // and the music clock is absolute; both drift from `CLOCK`, and the judge uses
                // only the *difference*, so either stamp reads as a judgement error that grows
                // with play time. A hold lives long enough for that error to reach its stored
                // `time`, which is why holds never registered while taps looked merely flaky.
                _rustPressTime[id] = PrprPort.GameTime();
            }
            try
            {
                GameSurface.CapturePointer(e.Pointer);
            }
            catch (Exception)
            {
            }
        }

        private void OnPointerMoved(object sender, PointerRoutedEventArgs e)
        {
            uint id = e.Pointer.PointerId;
            TouchInput cur;
            if (!_active.TryGetValue(id, out cur))
            {
                return;
            }
            TouchInput next = MakeTouch(id, TouchPhase.Moved, e.GetCurrentPoint(GameSurface).Position);
            next.Time = cur.Time;
            _active[id] = next;
        }

        private void OnPointerReleased(object sender, PointerRoutedEventArgs e)
        {
            uint id = e.Pointer.PointerId;
            TouchInput cur;
            if (!_active.TryGetValue(id, out cur))
            {
                // Logged because it is the one case where a release cannot be paired with a press:
                // either the press was dropped by the guard (so the finger was never tracked) or the
                // pointer was already retired. Both look like "the game ignored my tap" from outside.
                if (TouchLogDue())
                {
                    AppLog.Write("GamePage: touch up id=" + id + " (not tracked)");
                }
                return;
            }
            TouchInput next = MakeTouch(id, TouchPhase.Ended, e.GetCurrentPoint(GameSurface).Position);
            next.Time = cur.Time;
            _active[id] = next;
            _toRemove.Add(id);
            if (TouchLogDue())
            {
                AppLog.Write("GamePage: touch up id=" + id + " at " + next.Ndc.X.ToString("0.##")
                    + "," + next.Ndc.Y.ToString("0.##") + " remaining=" + _active.Count);
            }
        }

        private void OnPointerCanceled(object sender, PointerRoutedEventArgs e)
        {
            OnPointerReleased(sender, e);
        }

        private void OnPointerCaptureLost(object sender, PointerRoutedEventArgs e)
        {
            uint id = e.Pointer.PointerId;
            // Logged because this and `PointerReleased` are the two ways a finger can end, and only
            // one of them is what WP8.1 actually raises here. If releases never arrive, the finger
            // table can only drain through the sweep - and a press that never ends is a button that
            // draws its pressed look and never clicks.
            if (TouchLogDue())
            {
                AppLog.Write("GamePage: capture lost id=" + id + " tracked=" + _active.ContainsKey(id));
            }
            TouchInput cur;
            if (_active.TryGetValue(id, out cur))
            {
                // WP8.1 raises this for **every** pointer captured by `CapturePointer`, and it
                // raises it *after* `PointerReleased` - so it was overwriting a correct `Ended`
                // with `Cancelled` on the way to `PushRustTouches`, on every single touch. The
                // Rust phase census proved it: `e0` in every sampled window while `c` carried all
                // 97 presses, and there is nowhere else in this file that writes `Cancelled`.
                //
                // That is the whole "button draws its pressed look and never clicks" report. A UI
                // button completes on `Ended`; `Cancelled` means "aborted, ignore" - and held
                // drags, which never end, were unaffected. Only a finger still down may be
                // cancelled; one that already ended must keep its `Ended`.
                if (cur.Phase != TouchPhase.Ended && cur.Phase != TouchPhase.Cancelled)
                {
                    cur.Phase = TouchPhase.Cancelled;
                    _active[id] = cur;
                }
                _toRemove.Add(id);
            }
        }

        // ------------------------------------------------------------------
        // Frame
        // ------------------------------------------------------------------

        private void OnRendering(object sender, object e)
        {
            // The calibrator runs while the game itself is paused, so it gets the frame first.
            if (_calibrating)
            {
                UpdateCalibration();
                return;
            }

            // Before either render path: a track knocked out during the *previous* frame is
            // already silent by the time this one draws, and the chart path returns further
            // down for a dozen reasons that have nothing to do with the audio.
            GuardMusic();

            if (_phiraMode)
            {
                // phira owns its judge, its clock and its HUD, so none of the guards below apply:
                // there is no `_chart` to wait for, no `_clock` to resync against the audio, no
                // duration to finish at, and nothing to resync. `_paused` is deliberately not
                // tested either - it is only ever set by `DoPause`, which phira mode cannot reach
                // (the pause button is collapsed and Back leaves instead), and honouring it would
                // freeze the frame loop while phira kept animating in its own future.
                RenderPhiraFrame();
                return;
            }

            if (_paused || _finished || _chart == null || _renderer == null || _clock == null)
            {
                return;
            }

            // Hold the chart still until the audio is genuinely open, otherwise the lead-in
            // elapses in silence and the chart snaps backwards when MediaOpened re-anchors.
            // If the media never opens (missing or unsupported file) fall through after a
            // short grace period so the chart is still playable.
            if (!_mediaReady && !string.IsNullOrEmpty(_entry.AudioFile))
            {
                if ((DateTime.UtcNow - _startedUtc).TotalSeconds < 3.0)
                {
                    return;
                }
                _mediaReady = true;
            }

            if (_mediaReady)
            {
                _clock.Resync();
            }

            double t = _clock.Now;

            if (UseRustEngine)
            {
                RenderRustFrame(t);
            }
            else
            {
                // Snapshot active touches for this frame.
                _frameTouches.Clear();
                foreach (KeyValuePair<uint, TouchInput> kv in _active)
                {
                    TouchInput touch = kv.Value;
                    // Same rule as the Rust path's copy of this loop: a resolved `Ended`/`Cancelled`
                    // is final, and forcing `Started` over it swallows a tap whose press and release
                    // both landed inside one frame.
                    if (touch.Phase != TouchPhase.Ended && touch.Phase != TouchPhase.Cancelled)
                    {
                        if (_justStarted.Contains(kv.Key))
                        {
                            touch.Phase = TouchPhase.Started;
                        }
                        else if (touch.Phase == TouchPhase.Started)
                        {
                            touch.Phase = TouchPhase.Stationary;
                        }
                    }
                    _frameTouches.Add(touch);
                }

                _events.Clear();
                _judge.Update(_chart, t, 1.0, _frameTouches, _badNotes, _settings.Autoplay, _events);
                for (int i = 0; i < _events.Count; i++)
                {
                    ShowJudgement(_events[i]);
                }

                for (int i = 0; i < _toRemove.Count; i++)
                {
                    _active.Remove(_toRemove[i]);
                }
                _toRemove.Clear();

                // Expire ghost notes.
                for (int i = _badNotes.Count - 1; i >= 0; i--)
                {
                    if (t > _badNotes[i].Time + Win2DChartRenderer.BadNoteLifetime)
                    {
                        _badNotes.RemoveAt(i);
                    }
                }

                _renderer.Render(_chart, t, 1f);
                RenderBadNotes(t);
                // Render()/RenderBadNote() only record the frame; this schedules the Win2D
                // Draw callback that rasterises it.
                _renderer.Present();
            }
            _justStarted.Clear();

            _frameCount++;
            if (_frameCount % 4 == 0 && !UseRustEngine)
            {
                // `prpr` draws its own score, combo and judgement text (`scene/game.rs` reads
                // `self.judge.score()`), so the C# HUD would be a second one disagreeing with it.
                UpdateHud(t);
            }

            if (t > _chart.Duration + 1.2)
            {
                Finish();
            }
        }

        /// <summary>
        /// Maps this page's <see cref="TouchPhase"/> onto `prpr`'s encoding.
        ///
        /// By name, never by number. This enum is declared <c>Started, Moved, Stationary, Ended,
        /// Cancelled</c> while <c>miniquad::TouchPhase</c> - which is what <c>wp81_game_touch</c>
        /// documents and what <c>prpr</c> matches on - is <c>Started, Stationary, Moved, Ended,
        /// Cancelled</c>. <c>Stationary</c> and <c>Moved</c> are swapped between the two, so a numeric
        /// cast would report every held finger as moving on every frame that it did not move.
        /// </summary>
        private static uint RustPhase(TouchPhase phase)
        {
            switch (phase)
            {
                case TouchPhase.Started: return 0;
                case TouchPhase.Stationary: return 1;
                case TouchPhase.Moved: return 2;
                case TouchPhase.Ended: return 3;
                default: return 4; // Cancelled
            }
        }

        /// <summary>
        /// Creates the Rust game and tells it the screen size.
        ///
        /// Called from the load path rather than the render loop because <c>wp81_game_create</c>
        /// blocks for seconds - it parses the chart, builds the scene and uploads the textures - and
        /// doing that inside <c>CompositionTarget.Rendering</c> would freeze the UI thread mid-frame.
        ///
        /// <c>host::GAME</c> is a <c>thread_local!</c>, so this has to run on the same thread as every
        /// later <c>wp81_game_frame</c>. <c>OnRendering</c> is the UI thread and so is this, which is
        /// why there is no background task here - unlike <c>RustStage1Page</c>, which needed one.
        /// </summary>
        private void StartRustEngine()
        {
            double w = GameSurface.ActualWidth;
            double h = GameSurface.ActualHeight;
            if (w < 1)
            {
                w = Window.Current.Bounds.Width;
            }
            if (h < 1)
            {
                h = Window.Current.Bounds.Height;
            }
            try
            {
                _rustScale = Windows.Graphics.Display.DisplayInformation.GetForCurrentView().RawPixelsPerViewPixel;
            }
            catch (Exception)
            {
                // A pre-layout read can fail. 1.0 keeps the projection self-consistent rather than
                // degenerate, and the log line below carries the numbers that were actually used.
            }
            _rustW = (uint)Math.Max(1.0, Math.Round(w * _rustScale));
            _rustH = (uint)Math.Max(1.0, Math.Round(h * _rustScale));
            PrprPort.GameSetScreen(_rustW, _rustH);

            if (_phiraMode)
            {
                // The screen size is the only thing this path has in common with the chart one:
                // `PhiraBoot` pumps phira's own `the_main` future rather than building a
                // `GameScene`, so there is no folder and no chart file - and `_entry` must not be
                // read here, because in phira mode there is not one. Its `BasePath` would be the
                // NRE this branch exists to avoid.
                // `AssetRoot` is what every bare asset name gets resolved against, and phira asks
                // for `phigros.ttf` + `font.ttf` before it can build a single scene. Only
                // `RustStage1Page` ever assigned it, so on this path `ReadAsset` fell straight
                // through its `IsNullOrEmpty(root)` guard and reported every asset missing - the
                // boot came back `-2: asset not found: phigros.ttf` with nothing recorded saying
                // what the root had actually been. The chart is deliberately NOT resolved here:
                // `Data::init` reads it with `std::fs` from
                // `LocalFolder\data\charts\custom\190`, a completely different path.
                string assetRoot = Windows.Storage.ApplicationData.Current.LocalFolder.Path + "\\charts";
                PrprPort.AssetRoot = assetRoot;
                AppLog.WriteSync("GamePage: phira asset root = " + assetRoot);

                var boot = System.Diagnostics.Stopwatch.StartNew();
                AppLog.WriteSync("GamePage: phira boot (screen " + _rustW + "x" + _rustH + " px @ "
                    + _rustScale.ToString("0.0") + ", chart " + PhiraAutoChart + ")");
                // `LocalFolder` is handed over as phira's `DATA_PATH`. It is the one directory this
                // process is certainly allowed to create and write, and phira builds `data`,
                // `data/charts` and its cache underneath it; left unset phira falls back to a
                // relative `./data`, which is not ours here, and the boot dies before its first
                // frame with a reason that used to be dropped on the floor.
                PrprPort.GameResult bootResult = PrprPort.PhiraBoot(
                    Windows.Storage.ApplicationData.Current.LocalFolder.Path,
                    PhiraAutoOpen ? PhiraAutoChart : null,
                    PhiraAutoPlay);
                boot.Stop();
                _rustCreated = bootResult == PrprPort.GameResult.Ok;
                // The stage is logged on every attempt and the reason whenever the boot did not
                // succeed. The two answer different questions and both are needed on a phone: a
                // bare `-2` cannot say whether phira never left its entry point or built the whole
                // library scene and then failed, and `GameLastError` alone cannot say which of the
                // three `-2`s it was.
                AppLog.WriteSync("GamePage: phira boot -> " + (int)bootResult + " in " + boot.ElapsedMilliseconds
                    + " ms, stage " + PrprPort.GameProgressText()
                    + (_rustCreated ? "" : ", " + PrprPort.GameLastError()));
                return;
            }

            // `BasePath` is a URI ("ms-appx:///Assets/charts/190") or a StorageFolder path, and the
            // engine wants the chart's directory *name* under `charts\` - the last segment of
            // either form.
            string s = _entry.BasePath;
            int slash = s.LastIndexOfAny(new[] { '/', '\\' });
            string folder = slash >= 0 ? s.Substring(slash + 1) : s;

            var sw = System.Diagnostics.Stopwatch.StartNew();
            AppLog.WriteSync("GamePage: rust create " + folder + " / " + _entry.ChartFile
                + " (screen " + _rustW + "x" + _rustH + " px @ " + _rustScale.ToString("0.0") + ")");
            PrprPort.GameResult r = PrprPort.GameCreate(folder, _entry.ChartFile);
            sw.Stop();
            _rustCreated = r == PrprPort.GameResult.Ok;
            AppLog.WriteSync("GamePage: rust create -> " + (int)r + " in " + sw.ElapsedMilliseconds + " ms"
                + (_rustCreated ? "" : " (" + PrprPort.GameLastError() + ")"));
        }

        /// <summary>
        /// Drives the real <c>prpr</c> engine for one frame.
        ///
        /// The order is a contract, and each step depends on the one before it:
        ///
        ///   1. <c>GameSetTime(t)</c> - the engine timeline is set *absolutely* from the music clock.
        ///      <c>wp81_game_frame</c> advances it by <c>dt</c> internally, so the frame call must pass
        ///      0 or the chart would run ahead of the music.
        ///   2. push this frame's touches - <c>prpr</c> reads them during <c>update</c>, which happens
        ///      inside the frame call, so they have to be queued already.
        ///   3. <c>GameFrameSized(0)</c> - advances, renders, and drains at exactly the size the frame
        ///      produced. The capacities cannot be guessed: <c>Drain</c> reads exactly
        ///      <c>vertexCapacity</c> elements, so a larger one copies arena bytes no command refers to
        ///      and <c>OnPortDraw</c> rasterises those.
        ///   4. <c>DrawPortFrame</c> then <c>Present</c> - the frame is only stored by the first; the
        ///      drawing happens in <c>OnPortDraw</c> on the UI thread, because a Win2D session is not
        ///      valid outside the draw callback.
        /// </summary>
        private void RenderRustFrame(double t)
        {
            PrprPort.GameSetTime(t);
            PushRustTouches(t);

            PrprPort.DrainFrame frame = PrprPort.GameFrameSized(0.0);
            _renderer.DrawPortFrame(frame);
            _renderer.Present();
        }

        /// <summary>
        /// Queues this frame's pointers for the Rust engine, and retires the released ones.
        ///
        /// Shared verbatim by both Rust paths, and split out of <c>RenderRustFrame</c> because the
        /// only thing that differs between them is which number is used as the press time - the
        /// music clock on one side, <c>_phiraTime</c> on the other - and that difference is
        /// already resolved by the time either caller gets here.
        ///
        /// It has to run <em>before</em> the frame call: <c>prpr</c> reads the queue during
        /// <c>update</c>, which happens inside it, so a pointer pushed afterwards is a frame late.
        /// </summary>
        private void PushRustTouches(double t)
        {
            // The frame's engine time, sampled once. `prpr::judge` reads its `uptime` from the
            // engine clock and compares it against these stamps, so the two must come from the
            // same timeline: `t` is the shell's own (music clock or `_phiraTime`), which drifts
            // from the engine's `CLOCK` - and the judge uses only the *difference*, so any gap
            // is a judgement error, not a cosmetic one.
            double engineTime = PrprPort.GameTime();
            // Every finger still down gets an event this frame, not only the ones that moved:
            // `prpr` takes the touch *state* from `touches()` and the *events* from its own queue,
            // and a frame with no event for a finger reads as "nothing is holding that note".
            //
            // `Ndc` is converted here rather than in `MakeTouch` because the C# path needs NDC and
            // `prpr` needs pixels, and `MakeTouch` is shared with the fallback.
            foreach (KeyValuePair<uint, TouchInput> kv in _active)
            {
                TouchInput touch = kv.Value;
                // A phase the pointer handler already resolved is FINAL and must pass through.
                //
                // A press and a release can both land between two frames - that is what a fast tap
                // *is* - and `_justStarted` still holds the id on the frame that pushes it. Forcing
                // `Started` here therefore threw the `Ended` away, so the engine saw a finger go
                // down and never come up: a UI button drew its pressed look and never clicked, and a
                // fast tap on a note was never released. A held finger was unaffected, which is why
                // drag and long-press worked while everything else seemed dead.
                //
                // `_justStarted` only ever *upgrades* a touch that is still logically down.
                if (touch.Phase != TouchPhase.Ended && touch.Phase != TouchPhase.Cancelled)
                {
                    if (_justStarted.Contains(kv.Key))
                    {
                        touch.Phase = TouchPhase.Started;
                    }
                    else if (touch.Phase == TouchPhase.Started)
                    {
                        touch.Phase = TouchPhase.Stationary;
                    }
                }
                double time = engineTime;
                // Pre-declared: the project compiles as C# 5, which has no inline `out` declarations.
                double pressed;
                if (touch.Phase == TouchPhase.Started
                    && _rustPressTime.TryGetValue(kv.Key, out pressed))
                {
                    time = pressed;
                }
                PrprPort.GameTouch(
                    kv.Key,
                    RustPhase(touch.Phase),
                    (float)((touch.Ndc.X + 1.0) * 0.5) * _rustW,
                    (float)((touch.Ndc.Y + 1.0) * 0.5) * _rustH,
                    time);
            }

            // Released pointers are retired here, not by the C# branch's copy of this loop: the
            // Rust path never enters that branch, so without this `_active` would never empty and
            // every frame would keep pushing `Ended` for a finger that is already gone - and when
            // the OS reuses that pointer id, `MakeTouch` would find stale state waiting for it.
            for (int i = 0; i < _toRemove.Count; i++)
            {
                _active.Remove(_toRemove[i]);
                // The press stamp has to go with the finger. It is keyed by pointer id and the OS
                // reuses ids, so leaving it behind would hand the *next* press on that id the
                // previous one's timestamp - and it is only ever read for a `Started` event, which
                // is exactly what a fresh press is.
                _rustPressTime.Remove(_toRemove[i]);
            }
            _toRemove.Clear();

            // Second sweep, and the reason `_active` was growing without bound: retirement used to
            // depend *entirely* on `_toRemove`, so any release that did not reach it left a finger in
            // the table for ever. The log showed `remaining=` climbing 20 -> 23 -> 30 -> 33 in three
            // seconds, i.e. the engine was being handed thirty simultaneous fingers - and a button
            // that thinks other fingers are still down draws its pressed look and never completes a
            // click, which is exactly the report. Anything already finished is retired here whatever
            // the bookkeeping says.
            _retire.Clear();
            foreach (KeyValuePair<uint, TouchInput> kv in _active)
            {
                if (kv.Value.Phase == TouchPhase.Ended || kv.Value.Phase == TouchPhase.Cancelled)
                {
                    _retire.Add(kv.Key);
                }
            }
            for (int i = 0; i < _retire.Count; i++)
            {
                _active.Remove(_retire[i]);
                _rustPressTime.Remove(_retire[i]);
            }
            _retire.Clear();

            // Consumed once per frame, exactly as the C# judge path does it (see the clear at the
            // end of that branch). Without this the set only ever grew: a pointer id stayed in it
            // for the rest of the session, so every frame after a press re-sent that finger as
            // `Started` carrying its *original* press time. `prpr` ages a touch with
            // `t - (uptime - time) * speed`, so a pinned press time is not a cosmetic error - the
            // touch stopped being a held finger and became a press that never ages, which is why
            // the screen looked dead to input even though the events were arriving.
            _justStarted.Clear();
        }

        /// <summary>
        /// Drives phira's own loop for one frame.
        ///
        /// The mirror of <c>RenderRustFrame</c>, and every difference is the clock.
        ///
        ///   1. There is no <c>GameSetTime</c>. That call exists to pin the engine timeline to the
        ///      music, and phira has none: <c>host::frame(dt)</c> advances `CLOCK` itself from
        ///      wherever it already is. Setting it as well would rewind the timeline to the value
        ///      it had a frame ago, and the game would advance at half the rate the shell thinks it
        ///      is running at.
        ///   2. `GameFrameSized(dt)` instead of `0` for the same reason - the chart path hands the
        ///      engine an absolute time and asks it not to move, this path hands it a delta and
        ///      asks it to integrate.
        ///   3. `dt` is measured here rather than taken from `GetFrameTime`, which answers a
        ///      hard-coded 1/60 on this target (miniquad's `set_now_micros` has no caller, so the
        ///      macroquad clock never runs).
        /// </summary>
        private void RenderPhiraFrame()
        {
            DateTime now = DateTime.UtcNow;
            double dt = (now - _phiraLastFrameUtc).TotalSeconds;
            _phiraLastFrameUtc = now;
            if (!(dt > 0.0))
            {
                dt = 1.0 / 60.0;
            }
            else if (dt > 0.05)
            {
                // 50 ms - twice the 30 fps period. Anything longer is a hitch, and the very first
                // frame is the worst of them: it follows a boot that took seconds, so without this
                // clamp phira would open by integrating one enormous step, jumping its animations
                // and eating whatever input window fell inside it.
                dt = 0.05;
            }
            _phiraTime += dt;

            // `_phiraTime` rather than a separate "now": it has just been advanced, which is
            // exactly where `host::CLOCK` now points, so a press stamped with it is measured
            // against the same origin `prpr` will read.
            PushRustTouches(_phiraTime);

            // Split into its three parts. A single fps number cannot say *which* of them got
            // slower, and that question is exactly what came up when the scissor landed: the same
            // screen went 26 -> 13 fps with essentially the same command census, so the cost is
            // either Rust's update, the Win2D replay, or the present - three completely different
            // fixes. Each part is timed separately and reported as a per-frame average in ms.
            System.Diagnostics.Stopwatch part = System.Diagnostics.Stopwatch.StartNew();
            long t0 = part.ElapsedTicks;

            PrprPort.DrainFrame frame = PrprPort.GameFrameSized(dt);
            long t1 = part.ElapsedTicks;

            _renderer.DrawPortFrame(frame);
            long t2 = part.ElapsedTicks;

            _renderer.Present();
            long t3 = part.ElapsedTicks;

            _sumRust += t1 - t0;
            _sumDraw += t2 - t1;
            _sumPresent += t3 - t2;
            if (t3 - t0 > _maxFrame) _maxFrame = t3 - t0;

            // One line a second, and it is the shell's own number. The Rust side logs its frame
            // counter, but that only says the engine is being ticked - it cannot see how long the
            // drain, the command replay and the present took, which is where the port's cost lives
            // and what the fixes in this round were aimed at. Printed with the live touch count so
            // "laggy and ignoring taps" is one line to read rather than two logs to correlate.
            _frames++;
            int nowTick = Environment.TickCount;
            if (unchecked(nowTick - _fpsTick) >= 1000)
            {
                double secs = (nowTick - _fpsTick) / 1000.0;
                _fpsTick = nowTick;
                double fps = _frames / secs;
                double ticksPerMs = System.Diagnostics.Stopwatch.Frequency / 1000.0;
                double rustMs = _frames > 0 ? (_sumRust / ticksPerMs) / _frames : 0.0;
                double drawMs = _frames > 0 ? (_sumDraw / ticksPerMs) / _frames : 0.0;
                double presMs = _frames > 0 ? (_sumPresent / ticksPerMs) / _frames : 0.0;
                double maxMs = _maxFrame / ticksPerMs;
                AppLog.Write("GamePage: fps=" + fps.ToString("0.#")
                    + " rustMs=" + rustMs.ToString("0.0")
                    + " drawMs=" + drawMs.ToString("0.0")
                    + " presMs=" + presMs.ToString("0.0")
                    + " worstMs=" + maxMs.ToString("0.0")
                    + " touches=" + _active.Count + " paused=" + _paused
                    // The music's own state, sampled every second: a hit sound that takes the BGM
                    // with it shows up as `music=Paused`/`Stopped` from the next second on, and a
                    // quiet volume or a dead element shows up as `mv=0`. Without this the report
                    // and "the log shows nothing wrong" are indistinguishable.
                    + " music=" + Player.CurrentState
                    + " mv=" + Player.Volume.ToString("0.##")
                    // Cumulative count of sounds this pool has been asked to play. `music=` is a
                    // one-per-second sample of the element's state; this is how many sounds went
                    // out during that same second. Together they attribute a `music=Playing ->
                    // Paused` flip to a hit or rule one out, instead of leaving the two facts
                    // known separately. It is a counter, so it adds no lines to a log that has
                    // already proven too small.
                    + " sfxN=" + (_sfx == null ? 0 : _sfx.Played) + " sfxCut=" + (_sfx == null ? 0 : _sfx.MusicStateChanges)
                    // ...and how many times the frame guard had to put the track back. `sfxCut`
                    // counts state *changes*; this counts the ones where nothing else would have
                    // recovered, so `music=Paused ... mGuard=5` is "the sound kept killing it and
                    // it kept coming back" while `music=Paused ... mGuard=0` is "nobody tried".
                    + " mGuard=" + _musicGuardRescues
                    // ...and how many times the element claimed to be playing while standing
                    // still. `music=Playing ... mGuard=0 mStall=3` is the state check passing
                    // and the position check failing - the one case the state line alone
                    // reports as healthy, and the one the report describes.
                    + " mStall=" + _musicStalls);
                _frames = 0;
                _sumRust = 0; _sumDraw = 0; _sumPresent = 0; _maxFrame = 0;
            }
        }

        private int _frames;
        private int _fpsTick = Environment.TickCount;

        /// <summary>Per-second accumulators for the three phases of one frame, in Stopwatch ticks.</summary>
        private long _sumRust, _sumDraw, _sumPresent, _maxFrame;

        private void RenderBadNotes(double t)
        {
            // The ghost notes are drawn by the renderer's pool as well; to keep the
            // renderer simple they are approximated by reusing the note pool through a
            // dedicated pass.
            for (int i = 0; i < _badNotes.Count; i++)
            {
                _renderer.RenderBadNote(_badNotes[i], t, _aspectRatio);
            }
        }

        /// <summary>
        /// Reacts to a judgement.
        ///
        /// prpr (judge.rs) plays the note's hit sound and emits the hit spark on exactly the
        /// same two outcomes: Perfect and Good. Bad pushes a ghost note and stays silent,
        /// Miss does nothing. The sound is chosen by the note kind
        /// (HitSound::default_from_kind: Click/Hold -> click, Flick -> flick, Drag -> drag).
        /// </summary>
        private void ShowJudgement(Judge.JudgementEvent ev)
        {
            bool hit = ev.What == Judgement.Perfect || ev.What == Judgement.Good;

            if (ev.What == Judgement.Perfect) _judgeText = "PERFECT";
            else if (ev.What == Judgement.Good) _judgeText = "GOOD";
            else if (ev.What == Judgement.Bad) _judgeText = "BAD";
            else _judgeText = "MISS";
            _judgeTextUntil = _clock.Now + 0.35;

            if (!hit || _chart == null || ev.LineId < 0 || ev.LineId >= _chart.Lines.Count)
            {
                return;
            }
            List<Note> notes = _chart.Lines[ev.LineId].Notes;
            if (ev.NoteId < 0 || ev.NoteId >= notes.Count)
            {
                return;
            }
            Note note = notes[ev.NoteId];

            PlayHitSound(note.Kind.Type);
            if (_settings.ShowHitEffects)
            {
                EmitHitFx(ev, note, ev.What == Judgement.Good);
            }
        }

        /// <summary>Maps a note kind onto Phira's sfx_click / sfx_flick / sfx_drag.</summary>
        private void PlayHitSound(NoteKindType kind)
        {
            if (_sfx == null)
            {
                return;
            }
            string name = kind == NoteKindType.Flick ? "flick"
                        : kind == NoteKindType.Drag ? "drag"
                        : "click";   // Click and Hold both use the click sound
            _sfx.Play("ms-appx:///Assets/game/" + name + ".wav");
        }

        /// <summary>
        /// Queues the hit spark at the note's current chart position.
        ///
        /// prpr emits the particle from inside `with_model(line_tr * note.object.now())`, so
        /// the spark lands on the judge line at the note's own translation - the falling
        /// `base` offset is deliberately not included, which is why NoteTransform is asked
        /// for base 0 and no incline.
        /// </summary>
        private void EmitHitFx(Judge.JudgementEvent ev, Note note, bool good)
        {
            if (_renderer == null)
            {
                return;
            }
            JudgeLine line = _chart.Lines[ev.LineId];

            // The renderer pushes the animation clocks forward in its own Draw pass; do the
            // same here so the spark is placed with the same line transform as the frame.
            double t = _clock.Now;
            line.Obj.SetTime(t);
            line.Height.SetTime(t);

            Mat3 frame = line.NowTransform(_aspectRatio, _chart.Lines);
            if (!note.Above)
            {
                frame = frame * Mat3.Scaling(1f, -1f);
            }
            Mat3 m = frame * Renderer.NoteTransform(note, line, 0f, 0f, _aspectRatio, false);

            HitFx fx = new HitFx();
            fx.Position = m.TransformPoint(Vec2.Zero);
            fx.Time = t;
            fx.Good = good;
            _renderer.AddHitFx(fx);
        }

        private void UpdateHud(double t)
        {
            ScoreText.Text = _judge.Score.ToString("N0");
            AccText.Text = (_judge.RealTimeAccuracy * 100.0).ToString("0.00") + "%";
            uint combo = _judge.Combo;
            // prpr shows the combo from 3 upwards, with a caption underneath that reads
            // AUTOPLAY while autoplay is on.
            bool show = combo > 2;
            ComboText.Text = show ? combo.ToString() : string.Empty;
            ComboLabelText.Text = show ? (_settings.Autoplay ? "AUTOPLAY" : "COMBO") : string.Empty;
            JudgeText.Text = t < _judgeTextUntil ? _judgeText : string.Empty;
        }

        /// <summary>
        /// The play's result, from whichever judge actually ran.
        ///
        /// On the Rust path the shell's <see cref="Judge"/> was never advanced, so its
        /// <c>Result()</c> would describe a flawless zero-score play - a result page reading
        /// "0 points, 100% accuracy" after a real play is worse than one reading nothing, because
        /// it looks like a real verdict. <c>GamePlayResult</c> carries the whole thing, counts and
        /// early/late included, because <c>ResultPage</c> displays all of them.
        /// </summary>
        private PlayResult RustResult()
        {
            PlayResult rust;
            if (!UseRustEngine || PrprPort.GamePlayResult(out rust) != 0)
            {
                return _judge.Result();
            }
            return rust;
        }

        private void Finish()
        {
            if (_finished)
            {
                return;
            }
            _finished = true;
            _musicWantPlaying = false;
            StopLoop();
            try
            {
                Player.Pause();
            }
            catch (Exception)
            {
            }
            LastResult = RustResult();
            LastChartTitle = _entry.Title + "  ·  " + _entry.LevelDisplay;
            this.Frame.Navigate(typeof(ResultPage));
        }

        // ------------------------------------------------------------------
        // phira's music (PrprPort.IMusicSink)
        // ------------------------------------------------------------------

        /// <summary>
        /// Records the `ms-appdata:` URI of the chart's music, before the phira boot.
        ///
        /// The port's `music_new` arrives as a clip id, a rate and an amplifier and nothing else:
        /// the bytes belong to Rust and never reach the shell, so the one thing a `MediaElement`
        /// needs - a source - cannot be discovered when the call lands. It is therefore found
        /// here, while there is still an `await` available, from the same folder
        /// `EnsureBundledChartAsync` just wrote and recorded in <see cref="PhiraChartDir"/>.
        ///
        /// The folder is rebuilt from <see cref="PhiraAutoChart"/> rather than from the recorded
        /// path, so the directory read here and the one phira reopens with `std::fs` are the same
        /// string by construction - the reasoning that already makes the bundler derive its
        /// destination instead of spelling it out.
        ///
        /// `info.yml`'s `music:` field is the authoritative answer, with "the first audio file in
        /// the folder" as fallback: a chart that misquotes its own metadata still has music, and
        /// silence there would read as a broken engine rather than a badly described chart.
        /// </summary>
        private static async Task ResolvePhiraMusicAsync()
        {
            PhiraMusicUri = null;
            PhiraMusicSeconds = 0.0;
            try
            {
                Windows.Storage.CreationCollisionOption open =
                    Windows.Storage.CreationCollisionOption.OpenIfExists;
                Windows.Storage.StorageFolder local = Windows.Storage.ApplicationData.Current.LocalFolder;
                Windows.Storage.StorageFolder dir = await local.CreateFolderAsync("data", open);
                dir = await dir.CreateFolderAsync("charts", open);
                int cut = PhiraAutoChart.LastIndexOf('/');
                dir = await dir.CreateFolderAsync(PhiraAutoChart.Substring(0, cut), open);
                dir = await dir.CreateFolderAsync(PhiraAutoChart.Substring(cut + 1), open);

                string music = null;
                try
                {
                    Windows.Storage.StorageFile info = await dir.GetFileAsync("info.yml");
                    music = YamlScalar(await Windows.Storage.FileIO.ReadTextAsync(info), "music");
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.ResolvePhiraMusic(info.yml)", ex);
                }

                if (string.IsNullOrEmpty(music))
                {
                    foreach (Windows.Storage.StorageFile file in await dir.GetFilesAsync())
                    {
                        string lower = file.Name.ToLowerInvariant();
                        if (lower.EndsWith(".mp3") || lower.EndsWith(".ogg") || lower.EndsWith(".wav"))
                        {
                            music = file.Name;
                            break;
                        }
                    }
                }

                if (string.IsNullOrEmpty(music))
                {
                    AppLog.Write("GamePage: no music file in " + PhiraChartDir);
                    return;
                }

                // Rebuilt rather than converted from `PhiraChartDir`: `ms-appdata:///local/` *is*
                // LocalFolder, so this stays one string and cannot drift from the folder above.
                PhiraMusicUri = "ms-appdata:///local/data/charts/" + PhiraAutoChart + "/" + music;
                AppLog.Write("GamePage: phira music = " + PhiraMusicUri
                    + " (dir " + PhiraChartDir + ")");
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.ResolvePhiraMusic", ex);
            }
            await ResolvePhiraEndingMusicAsync();
        }

        /// <summary>
        /// Locates the res pack's `ending` clip, the music prpr wants once the chart is over.
        ///
        /// Rust resolves that clip for itself - `resource.rs:296` runs `load_clip!`, which walks
        /// `.wav`, `.mp3`, `.ogg` inside the pack's own fs - and the shell never sees the bytes,
        /// so the only way to play it is to know where the file landed. The pack is phira's own
        /// `assets` folder, pushed into `LocalFolder\charts` beside `font.ttf` and therefore under
        /// the same root <see cref="PrprPort.AssetRoot"/> already serves; `ending.ogg` sits at
        /// that root and `respack/ending.ogg` under the sub-folder, so both are tried before
        /// giving up. Failing is silent by design: a missing BGM is a quieter result page, while
        /// falling back to the chart music is the bug being removed.
        /// </summary>
        private static async Task ResolvePhiraEndingMusicAsync()
        {
            PhiraEndingMusicUri = null;
            string[] roots = { "", "respack\\" };
            // `.ogg` deliberately absent. This list is probed in order and `ending.ogg`
            // is present in the seeded asset tree, so it won every time - the result page
            // then asked MediaElement to open a source it cannot decode and played
            // silence, while `ending.mp3`, which it can decode, sat beside it. Ordering
            // cannot help while an undecodable extension is listed at all; only the
            // formats this profile actually plays belong here.
            string[] exts = { ".mp3", ".wav" };
            foreach (string root in roots)
            {
                foreach (string ext in exts)
                {
                    try
                    {
                        Windows.Storage.StorageFolder charts =
                            await Windows.Storage.ApplicationData.Current.LocalFolder
                                .GetFolderAsync("charts");
                        string name = "ending" + ext;
                        if (!string.IsNullOrEmpty(root))
                        {
                            charts = await charts.GetFolderAsync("respack");
                        }
                        await charts.GetFileAsync(name);
                        PhiraEndingMusicUri = "ms-appdata:///local/charts/"
                            + (root.Length == 0 ? "" : "respack/") + name;
                        AppLog.Write("GamePage: phira ending music = " + PhiraEndingMusicUri);
                        return;
                    }
                    catch (Exception)
                    {
                        // Not this root/extension pair - keep going, same order `load_clip!` uses.
                    }
                }
            }
            AppLog.Write("GamePage: no res-pack ending clip; the result page will be silent");
        }

        /// <summary>
        /// Pulls one scalar out of a flat `key: value` YAML document.
        ///
        /// `info.yml` is a straight scalar-per-line file with a couple of list blocks at the end,
        /// and the only field needed from it is one of those scalars. Handing it to a real YAML
        /// parser would mean a dependency for six lines of matching; scanning for a line that
        /// *starts* with `music:` cannot match `backgroundMusic:` or a mention inside `intro:`,
        /// which is the whole of what could go wrong here.
        /// </summary>
        private static string YamlScalar(string text, string key)
        {
            if (string.IsNullOrEmpty(text))
            {
                return null;
            }
            string prefix = key + ":";
            foreach (string raw in text.Split('\n'))
            {
                string line = raw.Trim();
                if (!line.StartsWith(prefix, StringComparison.Ordinal))
                {
                    continue;
                }
                string value = line.Substring(prefix.Length).Trim();
                if (value.Length >= 2
                    && ((value[0] == '\'' && value[value.Length - 1] == '\'')
                        || (value[0] == '"' && value[value.Length - 1] == '"')))
                {
                    value = value.Substring(1, value.Length - 2).Trim();
                }
                return (value.Length == 0 || value == "null") ? null : value;
            }
            return null;
        }

        /// <summary>
        /// True for the engine naming a chart's own music file: `<local_path>/<file>`.
        ///
        /// Matching on shape rather than on the file existing keeps this synchronous. `music_new`
        /// runs inside a Rust callback, so there is no `await` to put a storage probe on, and a
        /// fire-and-forget one would race the `Source` assignment two lines below. A resource name
        /// (`ending`) and an unnamed preview both fail the test and fall through to the duration
        /// check, which is unchanged and still the only answer for a clip the engine opened
        /// without a path.
        /// </summary>
        private static bool IsChartMusicPath(string source)
        {
            if (string.IsNullOrEmpty(source) || source.IndexOf('/') < 0)
            {
                return false;
            }
            string n = source.ToLowerInvariant();
            return n.EndsWith(".mp3") || n.EndsWith(".ogg") || n.EndsWith(".wav")
                || n.EndsWith(".flac") || n.EndsWith(".m4a") || n.EndsWith(".aac");
        }

        /// <summary>Opens the chart's music on the shell's `Player`. See <see cref="PrprPort.IMusicSink"/>.</summary>
        public void MusicNew(string source, double rate, float amp, double loopMix, double clipSecs)
        {
            if (_clock == null)
            {
                _clock = new MusicClock(Player);
                HookMusicStateEvents();
            }
            _clock.Rate = rate > 0.0 ? rate : 1.0;

            // A new Source means a new element lifecycle: the previous intent no longer
            // describes anything. It is re-armed by the `MusicPlay` prpr issues right after
            // `music_new`, and disarming here is what stops the frame guard from starting the
            // track inside the `MediaOpened -> Paused` window - `AutoPlay` is false, so that
            // window looks exactly like the knock-out the guard exists to repair.
            _musicWantPlaying = false;

            // The old element's movement history belongs to the old element. Leaving it in
            // place would compare the new Source's position against the previous track's and
            // open on what looks like a stall.
            _musicLastPos = -1.0;
            _musicStallTick = 0;
            _musicStallStage = 0;

            // Zero on purpose. prpr applies the chart's own offset downstream of this position,
            // so folding the shell's in here too would apply one correction twice - and when the
            // timing looks off there would be no way to tell which of the two moved.
            _clock.Offset = 0.0;
            MusicAmplifier(amp);

            // Which audio this is, answered by duration. `AudioClip` reports id 0 for every clip,
            // so without this the request is ambiguous between the chart's own track and the res
            // pack's `ending` clip - and the shell owns exactly one player, which it used to point
            // back at `PhiraMusicUri` no matter what was asked for. That is what played the song
            // a second time over the result page, right after it had finished.
            //
            // The tolerance is deliberately wide (1 s) and one-sided in effect: a *miss* here
            // costs the whole soundtrack, whereas a hit on a clip that happens to be a second
            // long only plays the wrong BGM. `NaturalDuration` and Rust's `frame_count /
            // sample_rate` come from different readings of the same file and routinely disagree
            // by the encoder padding, so anything tighter would misclassify the chart itself.
            string uri = PhiraMusicUri;
            bool isChart = true;
            if (IsChartMusicPath(source))
            {
                // The engine named this clip's own file, relative to phira's data root - which is
                // exactly where `EnsureBundledChartAsync` and the downloader put them, and exactly
                // how line `PhiraMusicUri` is built for the packaged chart. This is the only
                // branch that works for a chart the shell never resolved itself: a downloaded one
                // measured 274.9 s against the packaged chart's 214.8 s, missed the test below,
                // and played the res pack's `ending` in place of its own music.
                uri = "ms-appdata:///local/data/charts/" + source;
                isChart = true;
            }
            else if (PhiraMusicSeconds > 0.05 && clipSecs > 0.05
                && Math.Abs(clipSecs - PhiraMusicSeconds) > 1.0)
            {
                isChart = false;
                uri = PhiraEndingMusicUri;
            }

            if (string.IsNullOrEmpty(uri))
            {
                // Still free-run. A chart that plays in silence is a diagnosable state; one pinned
                // at zero - which is what `music_position -> 0.0` meant every second - is not.
                _clock.StartFree();
                AppLog.Write("GamePage: music_new secs=" + clipSecs.ToString("0.###")
                    + (isChart ? " chart but no music file" : " not the chart, no ending clip")
                    + " -> silent clock");
                return;
            }

            try
            {
                Player.Source = new Uri(uri);
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.MusicNew.Source(" + uri + ")", ex);
            }

            ActiveMusicUri = uri;
            _mediaReady = false;
            _clock.StartFree();
            AppLog.Write("GamePage: music_new -> " + uri
                + (isChart ? " (chart)" : " (res-pack ending)")
                + " src='" + (source ?? "") + "'"
                + " secs=" + clipSecs.ToString("0.###")
                + " rate=" + rate.ToString("0.###")
                + " amp=" + amp.ToString("0.###")
                + " loop=" + loopMix.ToString("0.###"));
        }

        public void MusicPlay()
        {
            // Recorded before anything else: the intent survives a `Player.Play()` that the
            // platform then silently undoes, which is the whole reason this flag exists.
            _musicWantPlaying = true;
            MusicClock clock = _clock;
            if (clock == null)
            {
                return;
            }
            if (!clock.IsRunning)
            {
                // Re-arms the stopwatch from the frozen position. `Resume` also re-issues a media
                // seek, which a not-yet-open element silently ignores - `OnMediaOpened` puts the
                // position back once it stops ignoring it.
                try
                {
                    clock.Resume();
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.MusicResume", ex);
                }
            }
            try
            {
                Player.Play();
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.MusicPlay", ex);
            }
        }

        public void MusicPause()
        {
            // The engine asked for silence, so the guard must not fight it. Cleared first for
            // the same reason `MusicPlay` sets it first - a throw below must not leave the
            // guard believing music should be running when the engine has moved on.
            _musicWantPlaying = false;
            MusicClock clock = _clock;
            if (clock == null)
            {
                return;
            }
            if (clock.IsRunning)
            {
                // `Pause` freezes the clock before it touches the element, so the chart stops
                // even in the case where the media call is the thing that throws.
                try
                {
                    clock.Pause();
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.MusicPause", ex);
                }
            }
            else
            {
                try
                {
                    Player.Pause();
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.MusicPause(media)", ex);
                }
            }
        }

        /// <summary>
        /// Restarts the music when it leaves `Playing` while the engine still wants it running.
        ///
        /// The bug this replaces ran the rescue *inside* `SfxPlayer.Play`, synchronously after
        /// `m.Play()`, and could not have worked: `Player.CurrentState` still reads `Playing`
        /// at that instant and only flips to `Paused` some hundreds of milliseconds later, once
        /// the sfx voice's own open/seek has finished rebuilding the shared audio session. The
        /// check returned before the thing it was checking for had happened. On device that is
        /// exactly what the log shows - hit 20 logs `resuming`, and the next second logs
        /// `music=Paused`, which then holds for the remaining 80 seconds of the song.
        ///
        /// A frame-driven guard is the only shape that closes the window, because it is the
        /// only one that runs *after* the drop rather than before it. It is driven by intent
        /// (<see cref="_musicWantPlaying"/>) rather than by a hit having just occurred, so a
        /// pause the engine asked for is left alone - restarting that would make pausing
        /// impossible and would be a worse bug than the one it fixes.
        /// </summary>
        private void GuardMusic()
        {
            if (!_musicWantPlaying || _finished || _paused)
            {
                return;
            }

            Windows.UI.Xaml.Media.MediaElementState st;
            try
            {
                st = Player.CurrentState;
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.GuardMusic.state", ex);
                _musicWantPlaying = false;
                return;
            }

            // `Opening` and `Closed` are not knock-outs. A Source that has just been assigned
            // passes through them before it can play at all, and `MusicPlay` has already been
            // issued by then - restarting in that window is what would fire the guard during
            // ordinary loading. `Buffering` is the platform deciding to refill, not to stop.
            //
            // `Playing` is deliberately *not* excluded any more. State only says what the
            // pipeline intends, and on device the element sat in `Playing` with its position
            // frozen for 280 seconds - a guard that trusted it would have reported "nothing
            // wrong" for the whole time the music was inaudible. Both ends of the gap are
            // handled below: not running, and claiming to run without doing so.
            bool stopped = st == Windows.UI.Xaml.Media.MediaElementState.Paused
                || st == Windows.UI.Xaml.Media.MediaElementState.Stopped;
            if (!stopped && st != Windows.UI.Xaml.Media.MediaElementState.Playing)
            {
                return;
            }

            int nowTick = Environment.TickCount;
            if (unchecked(nowTick - _musicGuardLastTry) < 250)
            {
                return;
            }
            _musicGuardLastTry = nowTick;

            if (stopped)
            {
                try
                {
                    Player.Play();
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.GuardMusic.Play", ex);
                    // A Play that throws is not a transient knock-out; stop asking, or this
                    // logs once a quarter second for the rest of the session.
                    _musicWantPlaying = false;
                    return;
                }

                _musicGuardRescues++;
                if (unchecked(nowTick - _musicGuardLogTick) >= 1000)
                {
                    _musicGuardLogTick = nowTick;
                    AppLog.Write("GamePage: GuardMusic resumed from " + st
                        + " n=" + _musicGuardRescues);
                }
                return;
            }

            GuardMusicStall(nowTick);
        }

        /// <summary>
        /// The half of <see cref="GuardMusic"/> for an element that insists it is playing.
        ///
        /// Two attempts, a second and a half apart, because they cost different things: a seek
        /// onto the position the clock says the song should be at is cheap and keeps the song
        /// where it belongs, and the Pause/Play that follows is for the stalls a seek cannot
        /// shift. Both are throttled by the caller to one attempt per 250 ms, and a stall that
        /// clears resets the stage - so a healthy element never does any of this.
        /// </summary>
        private void GuardMusicStall(int nowTick)
        {
            double pos;
            try
            {
                pos = Player.Position.TotalSeconds;
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.GuardMusic.position", ex);
                return;
            }

            if (_musicLastPos < 0.0 || pos + 0.5 < _musicLastPos)
            {
                // First sample, or a seek to an earlier point (a restart, a scrub). Begin a
                // fresh window rather than reading the drop as a stall - otherwise every
                // restart would be followed by a spurious "seek" a second and a half later.
                _musicLastPos = pos;
                _musicStallTick = 0;
                _musicStallStage = 0;
                return;
            }

            if (pos > _musicLastPos + 0.001)
            {
                _musicLastPos = pos;
                _musicStallTick = 0;
                _musicStallStage = 0;
                return;
            }

            if (_musicStallTick == 0)
            {
                _musicStallTick = nowTick;
                return;
            }

            // 1.5 s rather than anything tighter: this platform refreshes `Position` only a few
            // times a second - which is the whole reason `MusicClock` runs a Stopwatch instead
            // of reading it - so a tighter window would fire on an ordinary gap between two
            // refreshes. The measured failure lasted 280 s, so the price of waiting here is a
            // rounding error against the failure it is waiting for.
            if (unchecked(nowTick - _musicStallTick) < 1500)
            {
                return;
            }

            if (_musicStallStage == 0)
            {
                _musicStallStage = 1;
                try
                {
                    // Stop and start, rather than seek. Repositioning is `MusicClock.Resync`'s
                    // job now - it pushes the element up to the clock every 300 ms while it is
                    // behind - and a second writer doing the same thing is how two corrections
                    // end up fighting. On device that seek demonstrably never took: the element
                    // was written onto the clock's position at 10.73 s and eleven seconds later
                    // was still on its own timeline, 1.8 s behind. What a frozen element has not
                    // been offered yet is a stop and a start, which is what this is.
                    Player.Pause();
                    Player.Play();
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.GuardMusic.seek", ex);
                }
                // `_musicLastPos` is deliberately left at the position that *failed to move*.
                // It is not rewritten to a corrected value: doing so would put the baseline
                // ahead of where the element actually is, so the next sample would read as a
                // backwards jump - and the backwards-jump branch above exists for real restarts,
                // where it has to clear the stage. Consumed there instead it would clear the
                // stage on our own correction, so the bounce below could never be reached and
                // this half would repeat for ever while the other one never fired.
                //
                // Left alone the two outcomes separate on their own: an element that starts
                // moving again passes the "is it advancing" test and the stall clears; one that
                // does not stays exactly where it was, `_musicStallTick` keeps ageing, and the
                // 1.5 s window expires into the bounce.
                _musicStalls++;
                AppLog.Write("GamePage: GuardMusic stall at " + pos.ToString("0.###")
                    + "s -> pause/play n=" + _musicStalls);
                _musicStallTick = nowTick;
                return;
            }

            if (unchecked(nowTick - _musicStallTick) < 1500)
            {
                return;
            }

            // Still not moving, so bounce it again. Stays at stage 1 afterwards, which keeps
            // repeating this for as long as the element refuses to move - the alternative is
            // one attempt and then silence for the rest of the song. Every 1.5 s rather than
            // five: a bounce is the cheapest thing this file can do, and the cost of waiting
            // is measured in seconds of a song nobody can hear.
            _musicStallTick = nowTick;
            try
            {
                Player.Pause();
                Player.Play();
            }
            catch (Exception ex)
            {
                AppLog.Write("GamePage.GuardMusic.bounce", ex);
            }
            _musicStalls++;
            AppLog.Write("GamePage: GuardMusic stall persists at " + pos.ToString("0.###")
                + "s -> pause/play n=" + _musicStalls);
        }

        public void MusicSeek(double seconds)
        {
            MusicClock clock = _clock;
            if (clock == null)
            {
                return;
            }
            try
            {
                clock.Seek(seconds);
                // A seek moves the element on purpose. Without this the stall detector would
                // compare the position *before* the jump against the one after it and, for a
                // forward seek of under half a second, read its own correction as a failure.
                _musicLastPos = -1.0;
                _musicStallTick = 0;
                _musicStallStage = 0;
            }
            catch (Exception ex)
            {
                // Caught here rather than by the port: the port would only report "the sink threw",
                // and this is the line that says *which* seek at *which* time could not be applied.
                AppLog.Write("GamePage.MusicSeek(" + seconds.ToString("0.###") + ")", ex);
            }
        }

        public double MusicPosition()
        {
            MusicClock clock = _clock;
            if (clock == null)
            {
                return 0.0;
            }

            // `Resync` only once the media can report a position. Before `MediaOpened` it reads
            // zero, and re-anchoring onto that pins the chart at 0 for as long as the file takes
            // to open - which is precisely the symptom this sink exists to remove.
            if (_mediaReady)
            {
                clock.Resync();
            }
            return clock.Now;
        }

        public bool MusicPaused()
        {
            MusicClock clock = _clock;
            return clock == null || !clock.IsRunning;
        }

        public void MusicAmplifier(float amp)
        {
            // Both factors, and clamped: prpr's amplifier and the user's volume setting are
            // independent, and MediaElement.Volume rejects anything outside 0..1 outright.
            double volume = amp * (_settings == null ? 1.0 : _settings.VolumeMusic);
            Player.Volume = volume < 0.0 ? 0.0 : (volume > 1.0 ? 1.0 : volume);
        }

        // ------------------------------------------------------------------
        // Media events
        // ------------------------------------------------------------------

        private bool _musicStateHooked;

        /// <summary>
        /// Wires the music element's own change notification exactly once per page. The second
        /// `new MusicClock(Player)` on the phira path would otherwise stack a second handler onto
        /// the same element, and every flip would then be logged twice.
        /// </summary>
        private void HookMusicStateEvents()
        {
            if (_musicStateHooked)
            {
                return;
            }
            _musicStateHooked = true;
            Player.CurrentStateChanged += OnMusicStateChanged;
        }

        private void OnMusicStateChanged(object sender, RoutedEventArgs e)
        {
            _musicStateFlips++;
            AppLog.Write("GamePage: music state -> " + Player.CurrentState
                + " flip=" + _musicStateFlips
                + " sfxN=" + (_sfx == null ? 0 : _sfx.Played)
                + " paused=" + _paused + " finished=" + _finished);

            // During play the music is not allowed to leave Playing. The engine is the only
            // legitimate pauser (pause menu, page navigation), and it never goes through the
            // media element directly - so a state that is not Playing while the game is running
            // and not paused is the audio-session glitch, and the fix is to put it back before
            // the next frame notices. The guard on `_clock.IsRunning` is what keeps a pause the
            // engine asked for from being undone here.
            if (!_finished && !_paused && _clock != null && _clock.IsRunning
                && Player.CurrentState != Windows.UI.Xaml.Media.MediaElementState.Playing)
            {
                AppLog.Write("GamePage: forcing music back to Playing (glitch during play)");
                try
                {
                    Player.Play();
                }
                catch (Exception ex)
                {
                    AppLog.Write("GamePage.MusicForcePlay", ex);
                }
            }
        }

        private void OnMediaOpened(object sender, RoutedEventArgs e)
        {
            _mediaReady = true;

            // phira mode: the port's `music_*` callbacks drive this element instead of
            // `_started`/`_paused`, which only the chart path ever sets, so the generic branch
            // below would take the `_clock.PrimeAnchor()` path and leave the stopwatch running
            // away from the media. The clock has been free-running since `music_new` on purpose -
            // `MusicClock.Resync` pins it to a `Position` that reads 0 until the element opens -
            // and this is the first moment that number is real, so anchor onto it now.
            if (_phiraMode && _clock != null)
            {
                // Record the chart track's real length the first time its own file opens. This is
                // the only number the C# side can compare a `music_new` request against, because
                // `AudioClip` reports id 0 for every clip; without it `MusicNew` cannot tell the
                // chart song from the res pack's `ending` clip and the whole clip check is a
                // no-op. Guarded on the URI so the later `ending.ogg` open does not overwrite the
                // chart's length - if it did, the next real chart `music_new` would be classified
                // as a non-chart clip and the soundtrack would be dropped.
                if (ActiveMusicUri == PhiraMusicUri && Player.NaturalDuration.HasTimeSpan)
                {
                    PhiraMusicSeconds = Player.NaturalDuration.TimeSpan.TotalSeconds;
                }
                _clock.Reanchor();
                if (_clock.IsRunning)
                {
                    try
                    {
                        Player.Play();
                    }
                    catch (Exception ex)
                    {
                        AppLog.Write("MediaOpened(phira).Play", ex);
                    }
                }
                AppLog.Write("MediaOpened(phira): state=" + Player.CurrentState
                    + " dur=" + (Player.NaturalDuration.HasTimeSpan
                        ? Player.NaturalDuration.TimeSpan.TotalSeconds.ToString("0.###")
                        : "?")
                    + " chartSecs=" + PhiraMusicSeconds.ToString("0.###")
                    + " clock=" + _clock.Now.ToString("0.###"));
                return;
            }

            // The element is only now able to accept Play(). Re-anchor the clock to its real
            // position (it has been free-running on the stopwatch since StartPlayback) and
            // start it for real.
            if (_clock != null && _started && !_paused && !_finished)
            {
                _clock.Reanchor();
                TryStartMedia();
            }
            else if (_clock != null)
            {
                _clock.PrimeAnchor();
            }
            AppLog.Write("MediaOpened: state=" + Player.CurrentState
                + " dur=" + (Player.NaturalDuration.HasTimeSpan
                    ? Player.NaturalDuration.TimeSpan.TotalSeconds.ToString("0.###")
                    : "?"));
        }

        private void OnMediaFailed(object sender, ExceptionRoutedEventArgs e)
        {
            _mediaReady = false;
            // Into the log rather than `Debug.WriteLine`, which is discarded in a release build.
            // "the music stopped" and "the music element failed" look identical on screen, and
            // this is the only line that separates them - so it is worth the same treatment
            // `SfxPlayer.OnVoiceFailed` already gets.
            AppLog.WriteSync("GamePage: MediaFailed state=" + Player.CurrentState
                + " :: " + (e == null ? "(no args)" : e.ErrorMessage));
        }

        /// <summary>
        /// The clock is a Stopwatch re-anchored to the media, so a track that ends early
        /// (or an audio file shorter than the chart) must not freeze the chart — let the
        /// clock keep running and let the normal duration check finish the play.
        /// </summary>
        private void OnMediaEnded(object sender, RoutedEventArgs e)
        {
            _mediaReady = false;
            // The track ran out on its own. A guard that kept the intent set would call
            // `Play()` on a finished element every frame from here on.
            _musicWantPlaying = false;
        }

        // ------------------------------------------------------------------
        // Pause UI
        // ------------------------------------------------------------------

        private void DoPause()
        {
            if (_paused || _finished)
            {
                return;
            }
            _paused = true;
            _clock.Pause();
            PauseInfoText.Text = "分数 " + _judge.Score.ToString("N0") + "  ·  连击 " + _judge.Combo;
            PauseOverlay.Visibility = Visibility.Visible;
        }

        private void PauseButton_Click(object sender, RoutedEventArgs e)
        {
            DoPause();
        }

        // ------------------------------------------------------------------
        // On-screen log
        // ------------------------------------------------------------------

        private DispatcherTimer _logTimer;
        private int _logVersion = -1;

        /// <summary>Shows or hides the log panel, and runs the refresh timer only while it is up.</summary>
        private void LogToggle_Click(object sender, RoutedEventArgs e)
        {
            bool show = LogOverlay.Visibility != Visibility.Visible;
            LogOverlay.Visibility = show ? Visibility.Visible : Visibility.Collapsed;

            if (show)
            {
                if (_logTimer == null)
                {
                    _logTimer = new DispatcherTimer();
                    // Four times a second. The panel is read by a human, and every tick that finds
                    // the same version costs one integer compare - so the interval only has to be
                    // shorter than the time it takes to read a line, not shorter than a frame.
                    _logTimer.Interval = TimeSpan.FromMilliseconds(250);
                    _logTimer.Tick += LogTimer_Tick;
                }
                _logVersion = -1;
                LogTimer_Tick(null, null);
                _logTimer.Start();
            }
            else if (_logTimer != null)
            {
                _logTimer.Stop();
            }

            // Recorded either way, so the log always shows why the panel appeared or vanished.
            AppLog.WriteSync("GamePage: log overlay " + (show ? "on" : "off"));
        }

        private void LogTimer_Tick(object sender, object e)
        {
            int version;
            string text = AppLog.RecentText(out version);
            if (version != _logVersion)
            {
                _logVersion = version;
                LogOverlayText.Text = text;
            }
        }

        private void ResumeButton_Click(object sender, RoutedEventArgs e)
        {
            if (!_paused)
            {
                return;
            }
            _paused = false;
            PauseOverlay.Visibility = Visibility.Collapsed;
            _clock.Resume();
        }

        private void RetryButton_Click(object sender, RoutedEventArgs e)
        {
            RestartPlayback();
        }

        // ------------------------------------------------------------------
        // Offset calibration (a port of Phira's page/offset.rs)
        //
        // cali.wav is a two second looping metronome. The clock is re-anchored to the media
        // position every frame (MusicClock.Resync), which also absorbs the wrap at the end of
        // each loop. A beat lands whenever the calibrated clock reads 1.0s into the loop, and
        // that is when cali_hit.wav fires and the falling note reaches the white bar.
        //
        // Tapping records (tap time - beat time), wrapped into [-1, +1] seconds: positive
        // means the tap came late. The game computes the chart time as
        //     chartTime = mediaPosition + settings.Offset
        // so a late tap must be compensated by *decreasing* the offset, otherwise the note
        // would be judged even later than the player's tap.
        // ------------------------------------------------------------------

        private const double CaliLoop = 2.0;

        private bool _calibrating;
        private MusicClock _caliClock;
        private bool _caliMediaReady;
        private bool _caliWasBefore;      // were we showing the note on the previous frame?
        private double _caliOffset;       // working copy of GameSettings.Offset
        private readonly List<double> _caliSamples = new List<double>();
        private double _caliTapUntil;     // when the tap marker stops being drawn
        private double _caliTapT;         // the tap's position within the loop

        private void CalibrateButton_Click(object sender, RoutedEventArgs e)
        {
            if (_calibrating || _chart == null || !_paused)
            {
                return;
            }
            _calibrating = true;
            _caliOffset = _settings.Offset;
            _caliSamples.Clear();
            _caliTapUntil = 0.0;
            _caliTapT = 0.0;
            _caliWasBefore = false;
            _caliMediaReady = false;

            CalibOverlay.Visibility = Visibility.Visible;
            LayoutCalibrator();

            // The chart's music is already paused by DoPause; start the metronome on top.
            try
            {
                CaliPlayer.IsLooping = true;
            }
            catch (Exception)
            {
            }
            _caliClock = new MusicClock(CaliPlayer);
            _caliClock.Offset = 0.0;
            _caliClock.MaxDrift = 0.03;
            _caliClock.Start();
            // DoPause stopped the render loop; the calibrator needs it back.
            HookLoop();
            UpdateCalibrationInfo();
        }

        private void EndCalibration()
        {
            _calibrating = false;
            CalibOverlay.Visibility = Visibility.Collapsed;
            try
            {
                CaliPlayer.Pause();
                CaliPlayer.Source = null;
            }
            catch (Exception)
            {
            }
            _caliClock = null;
        }

        private void OnCaliMediaEnded(object sender, RoutedEventArgs e)
        {
            // Belt and braces for devices where IsLooping is ignored: restart the metronome.
            if (!_calibrating)
            {
                return;
            }
            try
            {
                CaliPlayer.Position = TimeSpan.Zero;
                CaliPlayer.Play();
            }
            catch (Exception)
            {
            }
        }

        private void LayoutCalibrator()
        {
            double w = CaliSurface.ActualWidth;
            double h = CaliSurface.ActualHeight;
            if (w < 10) w = Window.Current.Bounds.Width;
            if (h < 10) h = Window.Current.Bounds.Height;
            if (w < 10) w = 800;
            if (h < 10) h = 480;

            double cx = w / 2.0;
            double targetY = h * 0.74;
            Canvas.SetLeft(CaliTarget, cx - CaliTarget.Width / 2.0);
            Canvas.SetTop(CaliTarget, targetY);
            Canvas.SetLeft(CaliTapMarker, cx - CaliTapMarker.Width / 2.0);
            Canvas.SetTop(CaliTapMarker, targetY);
        }

        /// <summary>Wraps a loop position into [0, CaliLoop).</summary>
        private static double WrapLoop(double v)
        {
            double r = v % CaliLoop;
            if (r < 0.0) r += CaliLoop;
            return r;
        }

        /// <summary>Wraps a difference into [-CaliLoop/2, CaliLoop/2).</summary>
        private static double WrapError(double v)
        {
            double r = WrapLoop(v);
            if (r >= CaliLoop / 2.0) r -= CaliLoop;
            return r;
        }

        private void UpdateCalibration()
        {
            if (_caliClock == null)
            {
                return;
            }

            if (!_caliMediaReady)
            {
                // MusicClock needs one real media position before it can drift-correct.
                if (CaliPlayer.CurrentState == MediaElementState.Playing)
                {
                    _caliClock.PrimeAnchor();
                    _caliMediaReady = true;
                }
            }
            else
            {
                _caliClock.Resync();
            }

            LayoutCalibrator();

            double t = _caliClock.Now;
            double vt = WrapLoop(t - _caliOffset);

            // ---- the falling note and the beat ----
            bool before = vt <= 1.0;
            double w = CaliSurface.ActualWidth;
            double h = CaliSurface.ActualHeight;
            if (w < 10) w = Window.Current.Bounds.Width;
            if (h < 10) h = Window.Current.Bounds.Height;
            double targetY = h * 0.74;
            double fall = h * 0.62;

            if (before)
            {
                double y = targetY - (1.0 - vt) * fall;
                Canvas.SetLeft(CaliNote, w / 2.0 - CaliNote.Width / 2.0);
                Canvas.SetTop(CaliNote, y - CaliNote.Height / 2.0);
                CaliNote.Visibility = Visibility.Visible;
            }
            else
            {
                CaliNote.Visibility = Visibility.Collapsed;
            }

            // Crossing into the "after" half means the beat just landed.
            if (!before && _caliWasBefore)
            {
                if (_sfx != null)
                {
                    _sfx.Play("ms-appx:///Assets/game/cali_hit.wav");
                }
            }
            _caliWasBefore = before;

            // ---- the fading marker for the last tap ----
            if (t < _caliTapUntil)
            {
                double p = 1.0 - (t - (_caliTapUntil - 0.8)) / 0.8;
                if (p < 0.0) p = 0.0;
                CaliTapMarker.Opacity = p <= 0.5 ? 1.0 : (1.0 - p) * 2.0;
                Canvas.SetTop(CaliTapMarker, targetY - (1.0 - _caliTapT) * fall);
            }
            else
            {
                CaliTapMarker.Opacity = 0.0;
            }
        }

        private void OnCaliPointerPressed(object sender, PointerRoutedEventArgs e)
        {
            if (!_calibrating || _caliClock == null)
            {
                return;
            }
            double t = _caliClock.Now;
            double vt = WrapLoop(t - _caliOffset);
            // The beat is at vt == 1.
            double err = WrapError(vt - 1.0);
            if (Math.Abs(err) > 0.35)
            {
                // More than 350ms out is a stray touch, not a timing sample.
                return;
            }

            _caliSamples.Add(err);
            if (_caliSamples.Count > 16)
            {
                _caliSamples.RemoveAt(0);
            }
            _caliTapT = vt;
            _caliTapUntil = t + 0.8;
            UpdateCalibrationInfo();
        }

        private double CaliMeanError()
        {
            if (_caliSamples.Count == 0)
            {
                return 0.0;
            }
            double sum = 0.0;
            for (int i = 0; i < _caliSamples.Count; i++)
            {
                sum += _caliSamples[i];
            }
            return sum / _caliSamples.Count;
        }

        private void UpdateCalibrationInfo()
        {
            string mean = _caliSamples.Count == 0
                ? "—"
                : (CaliMeanError() * 1000.0).ToString("+0;-0;0") + " ms";
            CaliInfo.Text = "当前偏移 " + (_caliOffset * 1000.0).ToString("+0;-0;0") + " ms"
                + "　·　采样 " + _caliSamples.Count + " 次"
                + "　·　平均误差 " + mean;
        }

        private void CaliApply_Click(object sender, RoutedEventArgs e)
        {
            if (_caliSamples.Count == 0)
            {
                return;
            }
            _caliOffset -= CaliMeanError();
            _settings.Offset = _caliOffset;
            _settings.Save();
            _caliSamples.Clear();
            UpdateCalibrationInfo();
        }

        private void CaliReset_Click(object sender, RoutedEventArgs e)
        {
            _caliOffset = 0.0;
            _settings.Offset = 0.0;
            _settings.Save();
            _caliSamples.Clear();
            UpdateCalibrationInfo();
        }

        private void CaliMinus5_Click(object sender, RoutedEventArgs e)
        {
            _caliOffset -= 0.005;
            _settings.Offset = _caliOffset;
            _settings.Save();
            UpdateCalibrationInfo();
        }

        private void CaliPlus5_Click(object sender, RoutedEventArgs e)
        {
            _caliOffset += 0.005;
            _settings.Offset = _caliOffset;
            _settings.Save();
            UpdateCalibrationInfo();
        }

        private void CaliBack_Click(object sender, RoutedEventArgs e)
        {
            EndCalibration();
            // Re-arm the chart clock with whatever offset the user settled on, so pressing
            // "继续" resumes the run with it applied. The frame loop is deliberately left
            // hooked: DoPause never stopped it, and ResumeButton_Click does not restart it.
            if (_clock != null && _chart != null)
            {
                _clock.Offset = -StartDelay - _chart.Offset + _settings.Offset;
            }
        }

        private void QuitButton_Click(object sender, RoutedEventArgs e)
        {
            StopLoop();
            if (this.Frame.CanGoBack)
            {
                this.Frame.GoBack();
            }
        }
    }
}
