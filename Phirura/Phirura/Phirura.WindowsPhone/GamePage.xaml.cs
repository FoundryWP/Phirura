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
    /// Gameplay. Owns the chart, the judge, the renderer and the music clock, and drives
    /// them from CompositionTarget.Rendering (WP8.1 has no XAML-native game loop).
    /// </summary>
    public sealed partial class GamePage : Page
    {
        public static PlayResult LastResult;
        public static string LastChartTitle = string.Empty;

        private const double StartDelay = 1.6;

        private ChartEntry _entry;
        private Chart _chart;
        private Judge _judge;
        private Win2DChartRenderer _renderer;
        private MusicClock _clock;
        private GameSettings _settings;
        private SfxPlayer _sfx;

        private readonly List<BadNote> _badNotes = new List<BadNote>();
        private readonly List<Judge.JudgementEvent> _events = new List<Judge.JudgementEvent>();
        private readonly Dictionary<uint, TouchInput> _active = new Dictionary<uint, TouchInput>();
        private readonly HashSet<uint> _justStarted = new HashSet<uint>();
        private readonly List<uint> _toRemove = new List<uint>();
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

                _judge = new Judge(_chart);
                _judge.AspectRatio = _aspectRatio;

                _clock = new MusicClock(Player);
                _clock.Offset = -StartDelay - _chart.Offset + _settings.Offset;
                Player.Volume = _settings.VolumeMusic;

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

        private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
        {
            if (_paused || _finished || !_started)
            {
                return;
            }
            uint id = e.Pointer.PointerId;
            _active[id] = MakeTouch(id, TouchPhase.Started, e.GetCurrentPoint(GameSurface).Position);
            _justStarted.Add(id);
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
                return;
            }
            TouchInput next = MakeTouch(id, TouchPhase.Ended, e.GetCurrentPoint(GameSurface).Position);
            next.Time = cur.Time;
            _active[id] = next;
            _toRemove.Add(id);
        }

        private void OnPointerCanceled(object sender, PointerRoutedEventArgs e)
        {
            OnPointerReleased(sender, e);
        }

        private void OnPointerCaptureLost(object sender, PointerRoutedEventArgs e)
        {
            uint id = e.Pointer.PointerId;
            TouchInput cur;
            if (_active.TryGetValue(id, out cur))
            {
                cur.Phase = TouchPhase.Cancelled;
                _active[id] = cur;
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

            // Snapshot active touches for this frame.
            _frameTouches.Clear();
            foreach (KeyValuePair<uint, TouchInput> kv in _active)
            {
                TouchInput touch = kv.Value;
                if (_justStarted.Contains(kv.Key))
                {
                    touch.Phase = TouchPhase.Started;
                }
                else if (touch.Phase == TouchPhase.Started)
                {
                    touch.Phase = TouchPhase.Stationary;
                }
                _frameTouches.Add(touch);
            }
            _justStarted.Clear();

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

            _frameCount++;
            if (_frameCount % 4 == 0)
            {
                UpdateHud(t);
            }

            if (t > _chart.Duration + 1.2)
            {
                Finish();
            }
        }

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

        private void Finish()
        {
            if (_finished)
            {
                return;
            }
            _finished = true;
            StopLoop();
            try
            {
                Player.Pause();
            }
            catch (Exception)
            {
            }
            LastResult = _judge.Result();
            LastChartTitle = _entry.Title + "  ·  " + _entry.LevelDisplay;
            this.Frame.Navigate(typeof(ResultPage));
        }

        // ------------------------------------------------------------------
        // Media events
        // ------------------------------------------------------------------

        private void OnMediaOpened(object sender, RoutedEventArgs e)
        {
            _mediaReady = true;
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
            System.Diagnostics.Debug.WriteLine("MediaElement failed: " + e.ErrorMessage);
        }

        /// <summary>
        /// The clock is a Stopwatch re-anchored to the media, so a track that ends early
        /// (or an audio file shorter than the chart) must not freeze the chart — let the
        /// clock keep running and let the normal duration check finish the play.
        /// </summary>
        private void OnMediaEnded(object sender, RoutedEventArgs e)
        {
            _mediaReady = false;
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
