using System;
using System.Collections.Generic;
using System.Numerics;
using System.Runtime.InteropServices;
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Brushes;
using Microsoft.Graphics.Canvas.Geometry;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Phirura.Core;
using Windows.Foundation;
using Windows.UI;

namespace Phirura.Game
{
    /// <summary>
    /// One hit spark.
    ///
    /// prpr (core/note.rs + judge.rs) calls `emit_at_origin` on a Perfect / Good hit, which
    /// spawns exactly one `hit_fx` particle at the note's *world position*. The particle is
    /// axis-aligned in world space - it is NOT rotated or scaled by the note's matrix - and
    /// plays hit_fx.png as an 8x7 atlas over the resource pack's `hit_fx_duration`, tinted by
    /// the judgement and faded along a white -> 0.7 alpha -> transparent curve.
    /// </summary>
    public sealed class HitFx
    {
        /// <summary>Chart-space position of the note at the moment it was hit.</summary>
        public Vec2 Position;

        public double Time;

        /// <summary>Good (rather than Perfect) - selects the fx_good tint.</summary>
        public bool Good;
    }

    /// <summary>
    /// Draws a chart with Win2D (Direct2D on D3D11) instead of pooled XAML elements.
    ///
    /// The maths is shared with <see cref="ChartRenderer"/> verbatim: the same Mat3
    /// composition feeds the same <see cref="Renderer.Decompose"/>, so the picture is
    /// identical and only the raster path changes. A note stops being a real XAML
    /// element with a CompositeTransform and becomes a DrawImage call on a
    /// CanvasDrawingSession, which removes the per-frame property-system and layout
    /// work that dominated the XAML version.
    ///
    /// This deliberately uses CanvasControl (a UI-thread control redrawn on Invalidate)
    /// rather than CanvasAnimatedControl. CanvasAnimatedControl runs Update/Draw on its
    /// own game-loop thread, but the judge and the HUD must stay on the UI thread:
    /// MusicClock reads MediaElement.Position and UpdateHud writes TextBlocks, neither
    /// of which is legal off-thread. Moving to a dedicated loop is the next step and it
    /// needs those two marshalled first.
    ///
    /// Coordinate convention: "chart units" are prpr's world units. prpr's camera is
    /// orthographic with x in [-1, 1] across the viewport width and y in [-1/ar, 1/ar]
    /// across its height, so one chart unit is `width / 2` pixels in x and
    /// `aspectRatio * height / 2` in y - the same number whenever aspectRatio == width/height.
    /// Everything below is therefore written in chart units and handed to the projection,
    /// which is how prpr does it.
    /// </summary>
    public sealed class Win2DChartRenderer
    {
        /// <summary>How long a "ghost" note (hit outside the BAD window) stays on screen.</summary>
        public const double BadNoteLifetime = 0.5;

        // ---- Resource-pack geometry (assets/respack/info.yml) ----
        //
        // hitFx: [8, 7]        hit_fx.png is an 8-column x 7-row atlas, i.e. 56 frames.
        // holdAtlas: [50, 50]  hold.png is ONE texture; the top 50px is the tail slice, the
        //                      bottom 50px the head slice, and the middle is the body.
        private const int HitFxCols = 8;
        private const int HitFxRows = 7;
        private const int HitFxFrames = HitFxCols * HitFxRows;

        /// <summary>prpr's default hit_fx_duration.</summary>
        private const double HitFxDuration = 0.5;

        /// <summary>prpr's default hold_atlas - the cap height, in pixels of hold.png.</summary>
        private const int HoldAtlasSlice = 50;

        /// <summary>Tint of a PERFECT hit, 0xe1ffec9f ARGB (prpr's default_perfect).</summary>
        private static readonly RgbaF FxPerfect = RgbaF.FromBytes(0xff, 0xec, 0x9f, 0xe1);

        /// <summary>Tint of a GOOD hit, 0xebb4e1ff ARGB (prpr's default_good).</summary>
        private static readonly RgbaF FxGood = RgbaF.FromBytes(0xb4, 0xe1, 0xff, 0xeb);

        private static readonly Color Background = Color.FromArgb(255, 7, 7, 11);

        private readonly CanvasControl _canvas;
        private readonly double _width;
        private readonly double _height;
        private readonly float _aspectRatio;

        private readonly ChartTexture _click;
        private readonly ChartTexture _drag;
        private readonly ChartTexture _flick;
        private readonly ChartTexture _hold;
        private readonly ChartTexture _hitFx;

        /// <summary>
        /// hit_fx.png pre-tinted with fx_perfect / fx_good.
        ///
        /// prpr tints the particle in its shader (`color = colors_curve * base_color`), but
        /// Win2D.win81's CanvasDrawingSession.DrawImage has no colour-tint overload - only
        /// (image, dest, source, opacity) and (image, dest, source, opacity, interpolation) -
        /// and CanvasComposite has no Multiply, so the tint is baked into two copies of the
        /// atlas once, at load. The alpha curve still goes through the opacity argument.
        /// </summary>
        private CanvasBitmap _hitFxPerfect;
        private CanvasBitmap _hitFxGood;

        private readonly Dictionary<string, ChartTexture> _lineTextures = new Dictionary<string, ChartTexture>();

        private readonly List<BadNote> _ghosts = new List<BadNote>();
        private readonly List<HitFx> _sparks = new List<HitFx>();

        private readonly Mat3 _chartToScreen;

        private Chart _chart;
        private double _time;
        private float _globalAlpha = 1f;
        private bool _hasFrame;
        private bool _attached;

        /// <summary>Mirrors the chart vertically; escape hatch if a device's orientation
        /// ends up opposite to the assumed camera convention.</summary>
        public bool FlipVertical;

        /// <summary>Half-width of a note in chart units.</summary>
        public float NoteWidth = (float)Consts.NoteWidthRatioBase;

        /// <summary>
        /// prpr's `ChartInfo::line_length`. A plain judge line is drawn from -LineLength to
        /// +LineLength chart units, so at the default of 6 it spans six screen widths and is
        /// clipped by the viewport. Getting this wrong is what made every judge line collapse
        /// onto the same spot.
        /// </summary>
        public double LineLength = 6.0;

        /// <summary>
        /// Stroke thickness of a plain judge line, in chart units. prpr uses 0.01 normally and
        /// 0.0076 once the line has a non-unit Y scale (`line_scaled` in core/line.rs), so that
        /// a stretched line does not turn into a slab.
        /// </summary>
        public double LineThickness = 0.01;

        /// <summary>prpr's `hit_fx_scale`; the bundled respack sets it to 0.9.</summary>
        public double HitFxScale = 0.9;

        /// <summary>The user's note scale; the hit spark is sized from it, as in prpr.</summary>
        public double NoteScale = 1.0;

        /// <summary>Notes actually submitted last frame (diagnostic; the XAML renderer
        /// reported its live element count here).</summary>
        public int LiveElements;

        /// <summary>
        /// Present watchdog: last tick that handed a frame over (<see cref="DrawPortFrame"/>)
        /// versus last tick that actually drew one (<see cref="OnPortDraw"/>).
        ///
        /// The screen has frozen with the game alive behind it more than once - frames keep
        /// arriving, nothing reaches the canvas, and only leaving the app and coming back
        /// paints what was already there. A one-second timer compares the two; when a fresh
        /// frame has sat undrawn for over 1.5 s it forces an <c>Invalidate</c> and says so.
        /// That line in the log is also the whole diagnosis: it present means draws stopped
        /// while frames continued (present path), its absence during a freeze means the
        /// frames stopped too (engine side). Paused games never trip it - no new frame, no
        /// staleness - and the force itself is one redraw, not a storm.
        /// </summary>
        private int _lastPortFrameTick;
        private int _lastOnDrawTick;
        private Windows.UI.Xaml.DispatcherTimer _presentWatch;
        private bool _watchStarted;
        private int _watchLogTick;

        /// <summary>
        /// True draw cost, which the fps line never saw: it times the store-and-invalidate
        /// stub while the real work (texture uploads, carrier creation, command replay)
        /// happens later inside <see cref="OnPortDraw"/>, on the same UI thread but outside
        /// every measured region. A menu frame reading 120 ms of Rust with 0.9 fps has ~980
        /// ms hiding exactly here. Accumulated per draw, reported and reset with the line
        /// below; same thread on both ends, so no lock.
        /// </summary>
        private long _drawTicks;
        private long _carrierTicks;
        private int _carrierCount;
        private int _drawMeterTick;

        public Win2DChartRenderer(CanvasControl canvas, double width, double height, float aspectRatio,
                                  ChartTexture click, ChartTexture drag, ChartTexture flick,
                                  ChartTexture hold, ChartTexture hitFx)
        {
            _canvas = canvas;
            _width = width;
            _height = height;
            _aspectRatio = aspectRatio <= 0f ? 16f / 9f : aspectRatio;
            _click = click;
            _drag = drag;
            _flick = flick;
            _hold = hold;
            _hitFx = hitFx;
            _chartToScreen = ChartProjection.ChartToScreen(width, height, _aspectRatio);

            CanvasBitmap hitFxBmp = BitmapOf(hitFx);
            if (hitFxBmp != null)
            {
                _hitFxPerfect = Tint(canvas, hitFxBmp, FxPerfect);
                _hitFxGood = Tint(canvas, hitFxBmp, FxGood);
            }

            _canvas.Draw += OnDraw;
            _attached = true;
        }

        /// <summary>
        /// Returns a copy of <paramref name="src"/> whose RGB has been multiplied by
        /// <paramref name="tint"/>; the alpha channel is left alone (its weight is applied
        /// per frame as the draw opacity). Returns null on failure, in which case the caller
        /// falls back to the untinted atlas.
        ///
        /// Win2D.win81 only has the parameterless GetPixelColors() and the 4-argument
        /// CreateFromColors(), and both work in premultiplied alpha - which round-trips, and
        /// for which a per-channel RGB scale is exactly the right tint operation.
        /// </summary>
        private static CanvasBitmap Tint(ICanvasResourceCreator device, CanvasBitmap src, RgbaF tint)
        {
            try
            {
                int w = (int)src.SizeInPixels.Width;
                int h = (int)src.SizeInPixels.Height;
                Color[] px = src.GetPixelColors();
                for (int i = 0; i < px.Length; i++)
                {
                    Color c = px[i];
                    px[i] = Color.FromArgb(
                        c.A,
                        Channel(c.R / 255f * tint.R),
                        Channel(c.G / 255f * tint.G),
                        Channel(c.B / 255f * tint.B));
                }
                return CanvasBitmap.CreateFromColors(device, px, w, h);
            }
            catch (Exception ex)
            {
                AppLog.Write("Win2DChartRenderer.Tint", ex);
                return null;
            }
        }

        /// <summary>Queues a hit spark. Expired sparks are pruned by Render().</summary>
        public void AddHitFx(HitFx fx)
        {
            if (fx != null && _sparks.Count < 64)
            {
                _sparks.Add(fx);
            }
        }

        public void RegisterLineTexture(string path, ChartTexture texture)
        {
            if (!string.IsNullOrEmpty(path))
            {
                _lineTextures[path] = texture;
            }
        }

        public void Clear()
        {
            _chart = null;
            _hasFrame = false;
            _ghosts.Clear();
            _sparks.Clear();
            LiveElements = 0;
        }

        /// <summary>
        /// Unhooks the Draw handler and releases the control's device resources. Call this
        /// from the page's OnNavigatedFrom; a CanvasControl left in the tree keeps its
        /// swap chain alive.
        /// </summary>
        public void Detach()
        {
            if (_attached)
            {
                _canvas.Draw -= OnDraw;
                _attached = false;
            }
            try
            {
                _canvas.RemoveFromVisualTree();
            }
            catch (Exception)
            {
            }
            try
            {
                if (_hitFxPerfect != null) { _hitFxPerfect.Dispose(); _hitFxPerfect = null; }
                if (_hitFxGood != null) { _hitFxGood.Dispose(); _hitFxGood = null; }
                // The port's carriers go with them for the same reason: they are GPU resources
                // holding a whole chart's sprites plus the glyph atlas, and a page that has gone
                // away has no use for either.
                DisposePortTextures();
            }
            catch (Exception)
            {
            }
        }

        // ------------------------------------------------------------------
        // Frame recording
        //
        // Render() and RenderBadNote() only record; the actual drawing happens in
        // OnDraw, which Win2D raises on the UI thread after Present().
        // ------------------------------------------------------------------

        public void Render(Chart chart, double time, float globalAlpha)
        {
            _chart = chart;
            _time = time;
            _globalAlpha = globalAlpha;
            _hasFrame = chart != null;
            _ghosts.Clear();
            // Sparks live on their own clock, so they survive the frame reset and are pruned
            // here instead of being rebuilt by the caller.
            for (int i = _sparks.Count - 1; i >= 0; i--)
            {
                if (time - _sparks[i].Time > HitFxDuration)
                {
                    _sparks.RemoveAt(i);
                }
            }
        }

        public void RenderBadNote(BadNote bn, double time, float aspectRatio)
        {
            if (bn == null)
            {
                return;
            }
            double age = time - bn.Time;
            if (age < 0.0 || age > BadNoteLifetime)
            {
                return;
            }
            _ghosts.Add(bn);
        }

        /// <summary>Schedules the redraw that consumes the frame recorded above.</summary>
        public void Present()
        {
            _canvas.Invalidate();
        }

        // ------------------------------------------------------------------
        // Drawing
        // ------------------------------------------------------------------

        /// <summary>
        /// Whether the canvas's own size has been recorded.
        ///
        /// This is where "the picture is the wrong size" is decided, and nothing above it ever
        /// wrote the numbers down: Win2D draws in DIPs while Rust was told to render at pixel size
        /// through `GameSetScreen(_rustW, _rustH)`, and the two agree only at dpi == 96 - which
        /// this device is not. `GamePage` already logs the pixel side as `screen WxH px @ scale`,
        /// so this records the DIP side; the ratio between them is the whole question. One line,
        /// first draw only.
        /// </summary>
        private bool _dpiLogged;

        private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
        {
            CanvasDrawingSession ds = args.DrawingSession;
            ds.Clear(Background);

            if (!_dpiLogged)
            {
                _dpiLogged = true;
                // `ActualWidth`/`ActualHeight` and nothing else. The first attempt at this line
                // used `sender.Bounds` and `sender.SizeInPixels`, and this Win2D build has
                // neither - CS1061 on all five references. WinRT XAML has no `Bounds` on
                // `FrameworkElement` at all (that is a WPF shape), and `SizeInPixels` is not
                // projected onto `CanvasControl` here. `ActualWidth` is guaranteed, and the pixel
                // figure it needs to be compared against is already on the record.
                double dipW = sender.ActualWidth;
                double dipH = sender.ActualHeight;
                AppLog.WriteSync("Win2D: draw canvas=" + dipW.ToString("0.##") + "x" + dipH.ToString("0.##")
                    + " DIP (pixel size is GamePage's \"screen WxH px @ scale\" line)");
            }

            // The Rust game host takes priority when it is driving: its frame is already projected
            // and is the newer of the two paths. The XAML-chart path below is the fallback for every
            // screen the port does not own (menus, the calibration view, ...).
            if (_portFrame != null)
            {
                // Nothing below this may escape. A Win2D `Draw` handler is unhandled-exception
                // territory, and this is exactly where the cross-factory COMException that ended a
                // run leaving a community chart actually left the code - `OnPortDraw` had no try at
                // all. A draw failure costs one frame; an unhandled one costs the process.
                //
                // No `Invalidate()` in the handlers below, deliberately: the Rust render loop calls
                // `DrawPortFrame` every frame anyway, so a retry happens on its own - and scheduling
                // one from inside a catch is how a permanently failing device becomes a redraw storm
                // that drowns the log.
                try
                {
                    OnPortDraw(ds);
                }
                catch (Exception ex)
                {
                    if (IsDeviceLost(ex))
                    {
                        int carriers = _portTextures.Count;
                        DisposePortTextures();
                        _deviceLost++;
                        AppLog.WriteSync("Win2D: port draw device lost (#" + _deviceLost
                            + "), dropped " + carriers + " carriers", ex);
                    }
                    else
                    {
                        AppLog.WriteSync("Win2D: port draw failed", ex);
                    }
                }
                return;
            }

            if (!_hasFrame || _chart == null)
            {
                return;
            }

            Mat3 toScreen = ScreenMatrix();
            float ar = _aspectRatio;
            double margin = Math.Max(_width, _height) * 0.6;
            double time = _time;
            float globalAlpha = _globalAlpha;
            int drawn = 0;

            for (int oi = 0; oi < _chart.Order.Count; oi++)
            {
                JudgeLine line = _chart.Lines[_chart.Order[oi]];
                line.Obj.SetTime(time);
                line.Height.SetTime(time);
                line.Color.SetTime(time);
                line.Incline.SetTime(time);
                if (line.TextAnim != null) line.TextAnim.SetTime(time);
                if (line.PaintAnim != null) line.PaintAnim.SetTime(time);

                float alpha;
                float lineAlpha = line.Obj.Alpha.TryNow(out alpha) ? alpha : 1f;
                lineAlpha *= globalAlpha;
                if (lineAlpha <= 0f && !_chart.Settings.PeAlphaExtension)
                {
                    continue;
                }

                RgbaF lineColor = line.Color.Now();

                Mat3 lineChart = line.NowTransform(ar, _chart.Lines);
                Mat3 lineToScreen = toScreen * lineChart;

                // ---- the line itself ----
                double lineOp = Math.Max(0.0, lineAlpha) * (lineColor.A <= 0f ? 1.0 : lineColor.A);
                if (line.Kind == JudgeLineKindType.Normal)
                {
                    RgbaF c = lineColor;
                    if (c.A <= 0f) c = new RgbaF(1f, 1f, 1f, 1f);
                    DrawPlainLine(ds, toScreen, lineChart, line, c, lineOp);
                }
                else if (line.Kind == JudgeLineKindType.Texture)
                {
                    ChartTexture tex;
                    if (line.TexturePath != null && _lineTextures.TryGetValue(line.TexturePath, out tex))
                    {
                        Vec2 scale = line.Obj.Scale.NowWithDef(1f, 1f);
                        double sx, sy, rot, tx, ty;
                        Renderer.Decompose(lineToScreen, out sx, out sy, out rot, out tx, out ty);
                        // The XAML version gives the element the texture's natural size and
                        // folds the rest into the transform, so the final on-screen size is
                        // (sx * scale.X, sy * scale.Y).
                        DrawTexture(ds, tex, lineToScreen, sx * scale.X / tex.Width, sy * scale.Y / tex.Height, lineOp);
                        drawn++;
                    }
                }
                else if (line.Kind == JudgeLineKindType.Paint)
                {
                    // "Paint" lines fill the screen with a colour.
                    double size = line.PaintAnim != null ? line.PaintAnim.Now() : 0.0;
                    if (size > 0)
                    {
                        RgbaF c = lineColor;
                        c.A = (float)Math.Min(1.0, lineAlpha * 2.55 * (c.A <= 0f ? 1f : c.A));
                        DrawScreenFill(ds, c);
                    }
                }

                // ---- notes ----
                //
                // Everything that only depends on the LINE is hoisted out of the note loop.
                // It used to be recomputed per note, which for a chart with a few thousand
                // notes meant a few thousand extra animation lookups every frame.
                List<Note> notes = line.Notes;
                double lineHeight = line.Height.Now();
                double ctrlY = line.Ctrl.YOr1();
                float lineObjAlpha = line.Obj.NowAlpha();
                float inclineDeg = line.Incline.TryNow(out inclineDeg) ? inclineDeg : 0f;
                float inclineSin = (float)Math.Sin(inclineDeg * Math.PI / 180.0);

                for (int ni = 0; ni < notes.Count; ni++)
                {
                    Note note = notes[ni];
                    if (note.Judge.Kind == JudgeStatusKind.Judged && note.Kind.Type != NoteKindType.Hold)
                    {
                        continue;
                    }

                    double spd = note.Speed * ctrlY;
                    double baseVal = (note.Height - lineHeight) / ar * spd;

                    // Distance cull. A note is drawn at chart y = baseVal (plus its own
                    // translation), and the visible chart region is only y in
                    // [-1/ar, 1/ar] - under half a chart unit tall. 2.5 units is therefore
                    // far off screen, with room to spare for the note's own translation.
                    // This is the single biggest win on dense charts, because a chart spends
                    // most of its notes in the far future.
                    if (baseVal > 2.5 || baseVal < -2.5)
                    {
                        continue;
                    }

                    double noteAlpha = note.Obj.NowAlpha() * globalAlpha * lineObjAlpha;
                    if (noteAlpha <= 0.003)
                    {
                        continue;
                    }

                    // Notes disappear shortly after their judgement time.
                    if (time - note.Time > 0.16 && note.Kind.Type != NoteKindType.Hold)
                    {
                        continue;
                    }
                    if (note.Kind.Type == NoteKindType.Hold && time >= note.Kind.EndTime)
                    {
                        continue;
                    }

                    note.Obj.SetTime(time);
                    float baseOffset = (float)baseVal;

                    Mat3 noteChart = Renderer.NoteTransform(note, line, baseOffset, inclineSin, ar, false);
                    Mat3 frame = lineChart;
                    if (!note.Above)
                    {
                        frame = frame * Mat3.Scaling(1f, -1f);
                    }
                    Mat3 full = toScreen * frame * noteChart;

                    // Cheap viewport test on the note's origin.
                    Vec2 origin = full.TransformPoint(Vec2.Zero);
                    if (origin.X < -margin || origin.X > _width + margin ||
                        origin.Y < -margin || origin.Y > _height + margin)
                    {
                        continue;
                    }

                    double fade = 1.0;
                    if (note.Kind.Type != NoteKindType.Hold && time > note.Time)
                    {
                        fade = Math.Max(0.0, 1.0 - (time - note.Time) / 0.16);
                    }
                    double op = noteAlpha * fade;
                    if (note.Fake && time >= note.Time)
                    {
                        op = 0.0;
                    }

                    if (note.Kind.Type == NoteKindType.Hold)
                    {
                        DrawHold(ds, toScreen, frame, note, line, spd, op);
                        drawn++;
                        continue;
                    }

                    ChartTexture tex = note.Kind.Type == NoteKindType.Flick ? _flick
                        : note.Kind.Type == NoteKindType.Drag ? _drag : _click;
                    if (tex == null)
                    {
                        continue;
                    }
                    double noteHalfWpx = NoteWidth * (_width / 2.0);
                    double s = noteHalfWpx / (tex.Width / 2.0);
                    DrawTexture(ds, tex, full, s, s, op);
                    drawn++;
                }
            }

            DrawHitFx(ds, toScreen);

            for (int i = 0; i < _ghosts.Count; i++)
            {
                DrawGhost(ds, _ghosts[i]);
            }

            ds.Transform = Matrix3x2.Identity;
            LiveElements = drawn;
        }

        /// <summary>Draws the grey ghost marking a note hit outside the BAD window.</summary>
        private void DrawGhost(CanvasDrawingSession ds, BadNote bn)
        {
            ChartTexture tex = bn.Kind.Type == NoteKindType.Flick ? _flick
                : bn.Kind.Type == NoteKindType.Drag ? _drag : _click;
            if (tex == null)
            {
                return;
            }
            double age = _time - bn.Time;
            double op = 1.0 - age / BadNoteLifetime;
            double noteHalfWpx = NoteWidth * (_width / 2.0);
            double s = noteHalfWpx / (tex.Width / 2.0);
            Mat3 full = ScreenMatrix() * bn.Matrix;
            DrawTexture(ds, tex, full, s, s, op);
        }

        /// <summary>
        /// A plain judge line.
        ///
        /// prpr (core/line.rs) draws it as
        ///     with_model(line.now_transform())                 // rotation + translation
        ///     with_model(line.object.now_scale(Vector::default()))   // diag(scale.x, scale.y)
        ///         draw_line(-len, 0, len, 0, thickness, color) // len = info.line_length (6)
        /// i.e. a 2*len chart-unit bar of `thickness` chart units, both measured in chart
        /// space and both scaled by the line's own scale. The projection turns that into
        /// pixels.
        ///
        /// The previous version drew a `_width * 3` *pixel* rectangle and then multiplied it
        /// by the scale Decompose() extracted from the projected matrix - which is on the
        /// order of width/2 - so every line ended up thousands of screens wide and they all
        /// smeared into one bar. That is the "X-moving judge lines stick together" bug.
        /// </summary>
        private void DrawPlainLine(CanvasDrawingSession ds, Mat3 toScreen, Mat3 lineChart,
                                   JudgeLine line, RgbaF color, double opacity)
        {
            if (opacity <= 0.003)
            {
                return;
            }

            Vec2 scale = line.Obj.Scale.NowWithDef(1f, 1f);
            bool lineScaled = Math.Abs(scale.Y - 1f) > 1e-4f;
            double thickness = lineScaled ? 0.0076 : LineThickness;

            Mat3 m = toScreen * lineChart * Mat3.Scaling(scale.X, scale.Y);
            ds.Transform = ToMatrix3x2(m);
            ds.FillRectangle(CenteredRect(LineLength * 2.0, thickness), ColorWithOpacity(color, opacity));
        }

        /// <summary>Fills the whole viewport, used by "Paint" judge lines.</summary>
        private void DrawScreenFill(CanvasDrawingSession ds, RgbaF color)
        {
            if (color.A <= 0.003)
            {
                return;
            }
            ds.Transform = Matrix3x2.Identity;
            ds.FillRectangle(new Rect(0.0, 0.0, _width, _height), ColorWithOpacity(color, 1.0));
        }

        /// <summary>
        /// A hold note, drawn from prpr's single hold.png atlas.
        ///
        /// prpr (core/note.rs, NoteKind::Hold) renders the hold inside
        /// `with_model(now_transform(ctrl_obj, 0., 0.))` - base 0, no incline - and emits
        /// three quads, all `2 * scale` chart units wide:
        ///
        ///     bottom = h - line_height,  h = line_height before the note's time else note height
        ///     top    = end_height - line_height
        ///     body   : y in [bottom, top]                  source = hold_body_rect()
        ///     head   : y in [bottom - 2*caph, bottom]      source = hold_head_rect()   (only before the hit)
        ///     tail   : y in [top, top + 2*caph]            source = hold_tail_rect()
        ///
        /// where `caph = slice / textureWidth * scale`. `line_height`, `height` and
        /// `end_height` are all divided by the aspect ratio and multiplied by the note speed.
        ///
        /// prpr's `draw_tex` flips the sprite vertically, so the sheet's own top edge points
        /// towards +y (up the screen). The transform below carries the same flip.
        /// </summary>
        private void DrawHold(CanvasDrawingSession ds, Mat3 toScreen, Mat3 frame, Note note,
                              JudgeLine line, double spd, double opacity)
        {
            CanvasBitmap bmp = BitmapOf(_hold);
            if (bmp == null || opacity <= 0.003)
            {
                return;
            }

            double ar = _aspectRatio;
            double lineHeight = line.Height.Now() / ar * spd;
            double height = note.Height / ar * spd;
            double endHeight = note.Kind.EndHeight / ar * spd;

            // prpr: before the note's time the hold is anchored on the judge line.
            double h = _time <= note.Time ? lineHeight : height;
            double bottom = h - lineHeight;
            double top = endHeight - lineHeight;
            if (top - bottom < 0.0)
            {
                return;
            }

            double scale = NoteWidth;
            double halfW = scale * 2.0;

            // The atlas slices, in the bitmap's own DIP space.
            double k = bmp.Bounds.Width / bmp.SizeInPixels.Width;
            double slice = Math.Min(HoldAtlasSlice, bmp.SizeInPixels.Height * 0.25);
            double sheetH = bmp.SizeInPixels.Height;
            Rect srcTail = new Rect(0.0, 0.0, bmp.Bounds.Width, slice * k);
            Rect srcHead = new Rect(0.0, (sheetH - slice) * k, bmp.Bounds.Width, slice * k);
            Rect srcBody = new Rect(0.0, slice * k, bmp.Bounds.Width, (sheetH - 2.0 * slice) * k);

            // prpr: hf.y = r.h / r.w * scale * ratio, with ratio = sheetHeight / sheetWidth,
            // which collapses to slice / sheetWidth * scale.
            double capH = slice / (double)bmp.SizeInPixels.Width * scale;

            // Draw in a local y-down frame so Win2D's "source top -> destination top"
            // mapping reproduces prpr's flip: local y = -chart y.
            Mat3 m = toScreen * frame * Mat3.Scaling(1f, -1f);
            ds.Transform = ToMatrix3x2(m);

            float op = Clamp01(opacity);
            ds.DrawImage(bmp, new Rect(-scale, -top, halfW, top - bottom), srcBody, op,
                         CanvasImageInterpolation.Linear);

            // Head, drawn only until the note is hit (prpr's `res.time < self.time`).
            if (_time < note.Time)
            {
                ds.DrawImage(bmp, new Rect(-scale, -bottom, halfW, capH * 2.0), srcHead, op,
                             CanvasImageInterpolation.Linear);
            }

            ds.DrawImage(bmp, new Rect(-scale, -(top + capH * 2.0), halfW, capH * 2.0), srcTail, op,
                         CanvasImageInterpolation.Linear);
        }

        /// <summary>
        /// The hit sparks.
        ///
        /// prpr spawns the particle at the note's world position (axis-aligned, not rotated by
        /// the note) with `size = hit_fx_scale * note_scale / 5`, so the sprite covers
        /// `2 * size` chart units. The atlas frame advances linearly over the particle's
        /// lifetime, the tint comes from the judgement, and the alpha follows
        /// white -> 0.7 -> transparent.
        /// </summary>
        private void DrawHitFx(CanvasDrawingSession ds, Mat3 toScreen)
        {
            CanvasBitmap src = BitmapOf(_hitFx);
            if (src == null || _sparks.Count == 0)
            {
                return;
            }

            double half = HitFxScale * NoteScale / 5.0;
            double sheetW = src.SizeInPixels.Width;
            double sheetH = src.SizeInPixels.Height;
            double k = src.Bounds.Width / sheetW;
            double frameW = sheetW / HitFxCols;
            double frameH = sheetH / HitFxRows;

            ds.Transform = ToMatrix3x2(toScreen);
            for (int i = 0; i < _sparks.Count; i++)
            {
                HitFx fx = _sparks[i];
                double t = (_time - fx.Time) / HitFxDuration;
                if (t < 0.0 || t > 1.0)
                {
                    continue;
                }

                int frame = (int)(t * HitFxFrames);
                if (frame >= HitFxFrames) frame = HitFxFrames - 1;
                int col = frame % HitFxCols;
                int row = frame / HitFxCols;

                // prpr's colours_curve: start (a=1) -> mid (a=0.7) at t=0.5 -> end (a=0) at t=1.
                double curve = t < 0.5
                    ? 1.0 + (0.7 - 1.0) * (t * 2.0)
                    : 0.7 + (0.0 - 0.7) * ((t - 0.5) * 2.0);

                // The RGB tint is baked into the atlas; only the tint's own alpha and the
                // curve are left to apply here.
                CanvasBitmap bmp = (fx.Good ? _hitFxGood : _hitFxPerfect) ?? src;
                float tintAlpha = fx.Good ? FxGood.A : FxPerfect.A;

                Rect source = new Rect(col * frameW * k, row * frameH * k, frameW * k, frameH * k);
                ds.DrawImage(bmp, CenteredRect(half * 2.0, half * 2.0), source,
                             Clamp01(curve * tintAlpha), CanvasImageInterpolation.Linear);
            }
        }

        /// <summary>
        /// Draws a texture as if it were a XAML element of the texture's natural size,
        /// centred on the origin and carrying the CompositeTransform that
        /// <see cref="Renderer.Decompose"/> produced. Decompose is used only for the
        /// rotation and translation; the caller supplies the scale, exactly as the XAML
        /// renderer does.
        /// </summary>
        private void DrawTexture(CanvasDrawingSession ds, ChartTexture tex, Mat3 m,
                                 double scaleX, double scaleY, double opacity)
        {
            CanvasBitmap bmp = BitmapOf(tex);
            if (bmp == null || opacity <= 0.003)
            {
                return;
            }
            double sx, sy, rot, tx, ty;
            Renderer.Decompose(m, out sx, out sy, out rot, out tx, out ty);
            ds.Transform = Compose(scaleX, scaleY, rot, tx, ty);
            ds.DrawImage(bmp, CenteredRect(tex.Width, tex.Height), bmp.Bounds,
                         Clamp01(opacity), CanvasImageInterpolation.Linear);
        }

        private Mat3 ScreenMatrix()
        {
            if (!FlipVertical)
            {
                return _chartToScreen;
            }
            // Mirror chart-space y about 0 before projecting.
            Mat3 flip = Mat3.Scaling(1f, -1f);
            return _chartToScreen * flip;
        }

        // ==================================================================
        // the port path
        //
        // `prpr` itself renders when the Rust game host is driving: `wp81_game_frame` advances the
        // scene and records its draw calls, and these methods replay them. Everything here works in
        // *screen pixels*, because the Rust renderer has already applied the projection - the shim's
        // `screen_width`/`screen_height` are the same numbers the shell reports through
        // `wp81_game_set_screen`, so a coordinate is a coordinate and no matrix is involved.
        //
        // Deliberately separate from the XAML-chart path above: that one walks the C# `Chart` and
        // recomputes every transform, this one replays finished pixels. Mixing them would mean two
        // sources of truth for the camera.
        // ==================================================================

        /// <summary>
        /// One Rust texture and the carrier the shell draws it from.
        ///
        /// The carrier is a <see cref="CanvasRenderTarget"/> for *every* texture, not only for the
        /// atlas. A <see cref="CanvasBitmap"/> is immutable, so patching one glyph would mean
        /// rebuilding the whole image; and keeping both carrier types would double every draw
        /// site's branch for no gain - they cost the same in memory, and both reach the draw calls
        /// through <see cref="ICanvasImage"/>.
        /// </summary>
        private sealed class PortTexture
        {
            /// <summary>The image that is actually drawn - the same object as <see cref="Target"/>.</summary>
            public CanvasRenderTarget Target;

            /// <summary>
            /// The atlas's pixel size, fixed when the carrier is built.
            ///
            /// Read here rather than from the image because uv and the drain's source rectangles
            /// are *atlas* pixels. If the carrier's own metrics ever disagreed (a dpi change, a
            /// device-loss rebuild) this keeps the glyphs correctly sized instead of scaling them
            /// silently, and the mismatch is logged when the carrier is created.
            /// </summary>
            public uint PixelWidth;
            public uint PixelHeight;

            public void Dispose()
            {
                if (Target != null)
                {
                    Target.Dispose();
                    Target = null;
                }
            }
        }

        /// <summary>Rust texture id -> carrier, mirroring the shim's registry one for one.</summary>
        private readonly Dictionary<uint, PortTexture> _portTextures =
            new Dictionary<uint, PortTexture>();

        /// <summary>
        /// Reused across frames so that an idle poll allocates nothing; grows only when the
        /// registry gains ids, which its 128 slots bound.
        /// </summary>
        private uint[] _portIds;

        /// <summary>
        /// Latched when the DLL turns out not to export the incremental pair. Without the latch a
        /// missing symbol would be thrown once per texture per frame - an exception is far more
        /// expensive than the poll it replaces, so the first miss has to end the polling for good.
        /// </summary>
        private bool _portDirtyUnsupported;

        /// <summary>
        /// True once the legacy one-shot full read has run.
        ///
        /// Only the fallback reads this: the incremental path is stateless by design (see
        /// <c>wp81_texture_dirty</c>), and a latch in it would recreate the exact bug this replaces
        /// - a texture that is never read again after the first frame.
        /// </summary>
        private bool _portTexturesReady;

        /// <summary>Set by <see cref="DrawPortFrame"/>; the next draw replays this instead of the chart.</summary>
        private PrprPort.DrainFrame _portFrame;

        /// <summary>Reported in the diagnostics line so a blank port frame can be told from an unbound texture.</summary>
        public int PortCommands;
        public int PortTexturesBound;

        /// <summary>
        /// Forgets the port's textures. Call when the game is destroyed: the next game allocates its
        /// own ids and reusing these would draw the previous chart's sprites.
        ///
        /// Also the point at which the fallback's latch comes off, because the ids the next game
        /// hands out are new to the shell and the full read is what the first frame of a new game is
        /// supposed to do. Clearing the dictionary alone would leave that half of the state behind.
        /// </summary>
        public void ResetPortTextures()
        {
            DisposePortTextures();
            _hopelessPortTextures.Clear();
            _portTexturesReady = false;
            _portFrame = null;
        }

        /// <summary>
        /// Releases the carriers.
        ///
        /// They are D3D resources, not managed byte arrays: dropping the dictionary reference alone
        /// would leave 4 MiB render targets to a finalizer that may never run on a phone, and they
        /// must not outlive the chart that produced them regardless.
        /// </summary>
        private void DisposePortTextures()
        {
            foreach (KeyValuePair<uint, PortTexture> kv in _portTextures)
            {
                kv.Value.Dispose();
            }
            _portTextures.Clear();
            PortTexturesBound = 0;
            DisposePortGradients();
            DisposeTintCache();
        }

        /// <summary>Pre-tinted copies for dark textured draws - see the tint hook in the textured branch.</summary>
        private const int TintCacheMax = 64;
        /// <summary>Refuse to pre-tint anything bigger: a copy this large is a sprite, not a label.</summary>
        private const int TintMaxPixels = 128 * 128;
        private readonly Dictionary<string, CanvasBitmap> _tintCache = new Dictionary<string, CanvasBitmap>();
        private readonly Queue<string> _tintOrder = new Queue<string>();

        /// <summary>
        /// A copy of <paramref name="src"/> out of <paramref name="texId"/> multiplied by the
        /// given tint, or null when it cannot be built (too big, unreadable, device gone).
        ///
        /// Cached by texture, rect and tint byte: button labels are static, so the second frame
        /// with the same label hits. FIFO-evicted past <see cref="TintCacheMax"/>, and dropped
        /// with the carriers - the bitmaps belong to the same device.
        /// Stored premultiplied, like every other carrier: the source is straight alpha, so the
        /// tint multiplies first and the alpha folds after, exactly as the upload path does.
        /// </summary>
        private CanvasBitmap TintedCopy(ICanvasResourceCreator dev, uint texId, Rect src,
                                        float r, float g, float b)
        {
            int sx = (int)src.X;
            int sy = (int)src.Y;
            int sw = (int)src.Width;
            int sh = (int)src.Height;
            if (sw <= 0 || sh <= 0 || (long)sw * sh > TintMaxPixels)
            {
                return null;
            }
            string key = texId + ":" + sx + "," + sy + "," + sw + "x" + sh + ":"
                + ((int)(r * 255f)) + "," + ((int)(g * 255f)) + "," + ((int)(b * 255f));
            CanvasBitmap hit;
            if (_tintCache.TryGetValue(key, out hit) && hit != null)
            {
                return hit;
            }
            PrprPort.TexRect rect;
            rect.X = (uint)sx;
            rect.Y = (uint)sy;
            rect.W = (uint)sw;
            rect.H = (uint)sh;
            byte[] rgba;
            try
            {
                rgba = PrprPort.ReadTextureRect(texId, rect);
            }
            catch (Exception)
            {
                return null;
            }
            if (rgba == null || rgba.Length < sw * sh * 4)
            {
                return null;
            }
            Color[] pixels = new Color[sw * sh];
            for (int p = 0; p < pixels.Length; p++)
            {
                int o = p * 4;
                int a = rgba[o + 3];
                pixels[p] = Color.FromArgb((byte)a,
                    (byte)((rgba[o + 0] * r * a + 127) / 255),
                    (byte)((rgba[o + 1] * g * a + 127) / 255),
                    (byte)((rgba[o + 2] * b * a + 127) / 255));
            }
            CanvasBitmap bmp;
            try
            {
                bmp = CanvasBitmap.CreateFromColors(dev, pixels, sw, sh);
            }
            catch (Exception)
            {
                return null;
            }
            if (_tintCache.Count >= TintCacheMax)
            {
                string old = _tintOrder.Dequeue();
                CanvasBitmap victim;
                if (_tintCache.TryGetValue(old, out victim))
                {
                    _tintCache.Remove(old);
                    try { victim.Dispose(); }
                    catch (Exception) { }
                }
            }
            _tintCache[key] = bmp;
            _tintOrder.Enqueue(key);
            return bmp;
        }

        /// <summary>Releases the pre-tinted copies. See <see cref="DisposePortTextures"/>.</summary>
        private void DisposeTintCache()
        {
            foreach (KeyValuePair<string, CanvasBitmap> kv in _tintCache)
            {
                try { kv.Value.Dispose(); }
                catch (Exception) { }
            }
            _tintCache.Clear();
            _tintOrder.Clear();
        }

        /// <summary>
        /// One brush per distinct pair of end colours, reused across frames and across triangles.
        ///
        /// The shadow's eight triangles all share a single pair - transparent edge to shadow
        /// colour - so the whole ring costs one native brush rather than eight a frame, and the
        /// palette a chart can produce is small enough that the dictionary stays flat. The start
        /// and end points are rewritten on every use (they are the only per-triangle part), so a
        /// hit is a property set and nothing else.
        ///
        /// Disposed alongside the carriers because both are D2D resources created from a device
        /// that can go away: `DisposePortTextures` is what runs on a device loss and on game
        /// teardown, and a brush outliving its device is the same failure as a render target
        /// outliving it.
        /// </summary>
        private readonly Dictionary<ulong, CanvasLinearGradientBrush> _portGradients =
            new Dictionary<ulong, CanvasLinearGradientBrush>();

        private void DisposePortGradients()
        {
            foreach (KeyValuePair<ulong, CanvasLinearGradientBrush> kv in _portGradients)
            {
                kv.Value.Dispose();
            }
            _portGradients.Clear();
        }

        /// <summary>
        /// The brush for a two-colour gradient, creating it on the first request for that pair.
        ///
        /// Keyed on the two packed colours rather than on the geometry: the colours are what the
        /// brush *is*, and the endpoints are set by the caller each time. Ordered, so a shadow
        /// fading outwards and one fading inwards are different brushes and neither reuses the
        /// other's direction.
        /// </summary>
        private CanvasLinearGradientBrush PortGradientBrush(CanvasDrawingSession ds, Color c0, Color c1)
        {
            uint k0 = PackColor(c0);
            uint k1 = PackColor(c1);
            ulong key = ((ulong)k0 << 32) | k1;
            CanvasLinearGradientBrush brush;
            if (!_portGradients.TryGetValue(key, out brush) || brush == null)
            {
                brush = new CanvasLinearGradientBrush(ds, c0, c1);
                _portGradients[key] = brush;
            }
            return brush;
        }

        private static uint PackColor(Color c)
        {
            return (uint)((c.R << 24) | (c.G << 16) | (c.B << 8) | c.A);
        }

        /// <summary>
        /// Hands the renderer a frame recorded by the Rust game and schedules a redraw.
        ///
        /// The frame is only stored here; the actual drawing happens in <see cref="OnPortDraw"/> on
        /// the UI thread, because a Win2D drawing session is not valid outside the draw callback.
        /// </summary>
        public void DrawPortFrame(PrprPort.DrainFrame frame)
        {
            _portFrame = frame;
            _lastPortFrameTick = Environment.TickCount;
            StartPresentWatch();
            _canvas.Invalidate();
        }

        /// <summary>
        /// Starts the present watchdog once, on the UI thread that owns the canvas (this is
        /// only ever called from <see cref="DrawPortFrame"/>, which runs there).
        /// </summary>
        private void StartPresentWatch()
        {
            if (_watchStarted || _canvas == null)
            {
                return;
            }
            _watchStarted = true;
            _lastOnDrawTick = Environment.TickCount;
            Windows.UI.Xaml.DispatcherTimer timer = new Windows.UI.Xaml.DispatcherTimer();
            timer.Interval = TimeSpan.FromSeconds(1);
            timer.Tick += delegate(object sender, object e)
            {
                int now = Environment.TickCount;
                // Frames arriving with no draws behind them. Unchecked: TickCount wraps, and
                // a wrap turning stale into fresh for one interval beats the alternative.
                if (_portFrame != null
                    && unchecked(now - _lastOnDrawTick) > 1500
                    && unchecked(_lastPortFrameTick - _lastOnDrawTick) > 1500)
                {
                    try
                    {
                        _canvas.Invalidate();
                    }
                    catch (Exception)
                    {
                    }
                    if (unchecked(now - _watchLogTick) >= 10000)
                    {
                        _watchLogTick = now;
                        AppLog.Write("Win2D: forced present, draws " +
                            unchecked(now - _lastOnDrawTick) + "ms behind frames");
                    }
                }
            };
            _presentWatch = timer;
            timer.Start();
        }

        /// <summary>
        /// Replays one recorded frame. Called from <see cref="OnDraw"/> in place of the chart path.
        /// </summary>
        private void OnPortDraw(CanvasDrawingSession ds)
        {
            _lastOnDrawTick = Environment.TickCount;
            long drawT0 = System.Diagnostics.Stopwatch.GetTimestamp();
            EnsurePortTextures(ds);
            long tAfterPoll = System.Diagnostics.Stopwatch.GetTimestamp();
            ReportUploadBacklog();

            PrprPort.DrainFrame frame = _portFrame;
            if (frame == null || frame.Count == 0)
            {
                PortCommands = 0;
                _drawTicks += System.Diagnostics.Stopwatch.GetTimestamp() - drawT0;
                SlowDrawLog(drawT0, tAfterPoll, System.Diagnostics.Stopwatch.GetTimestamp(),
                    System.Diagnostics.Stopwatch.GetTimestamp(), 0,
                    0, 0, 0, 0, 0, 0);
                DrawMeter();
                return;
            }

            PortCommands = frame.Count;
            PrprPort.PortCmd[] cmds = frame.Commands;

            // px -> DIP, the one conversion nobody notices until it is missing.
            //
            // Rust projects in the device pixels `GameSetScreen` handed it; a Win2D drawing session
            // is in DIPs. Measured on the Lumia: `screen 1280x768 px @ 2.0` against a
            // `draw canvas=640x384 DIP` surface, so without this the whole frame landed at exactly
            // twice its intended size - the visible part was the top-left quarter, and the judge
            // line, which sits below the middle of the screen, was never on the canvas at all.
            // That is the "resolution is wrong" symptom, and it is also why no note was ever seen.
            float pxToDip = 1f;
            if (PrprPort.ScreenPixelWidth > 0f && _canvas != null && _canvas.ActualWidth > 0.0)
            {
                pxToDip = (float)(_canvas.ActualWidth / PrprPort.ScreenPixelWidth);
            }

            // Composed rather than assigned so an inherited transform still applies afterwards; see
            // `DrawPortTexture`, which puts the result back when it is done.
            Matrix3x2 portTransform = ds.Transform;
            ds.Transform = Matrix3x2.CreateScale(pxToDip) * portTransform;

            long kindTex = 0, kindRect = 0, kindLine = 0, kindCirc = 0, kindGeo = 0, kindClip = 0;
            for (int i = 0; i < frame.Count; i++)
            {
                // Recorded before the dispatch, not after: a command that takes the process down
                // never reaches the next line, and the census lines emitted inside `DrawPortTexture`
                // / `DrawPortGeometry` read this field to report which slot they were replayed from.
                _portCmdIdx = i;
                PrprPort.PortCmd c = cmds[i];
                // The kind's high bits carry the flip flags for textures, so mask before dispatching.
                uint kind = c.kind & 0xFFFF;

                // Per-kind cost accumulators for the slow-draw line below. Two timestamp reads
                // per command - the only way to separate "clip layers are slow" from
                // "geometries are slow" from "lines are slow", which are three different fixes.
                long kt0 = System.Diagnostics.Stopwatch.GetTimestamp();
                switch (kind)
                {
                    case 0: // Texture: dest x/y/w/h, then source x/y/w/h
                        DrawPortTexture(ds, c);
                        kindTex += System.Diagnostics.Stopwatch.GetTimestamp() - kt0;
                        break;

                    case 1: // Rect: x/y/w/h then r/g/b/a
                        {
                            ds.FillRectangle(
                                c.f0, c.f1, c.f2, c.f3,
                                FloatColor(c.f4, c.f5, c.f6, c.f7));
                            kindRect += System.Diagnostics.Stopwatch.GetTimestamp() - kt0;
                            // A solid bar is the simplest shape this dispatcher can produce,
                            // so a reported-missing one cannot be at fault in its geometry or
                            // its texture - only in where it was drawn. Once a second, with
                            // the box it was allowed to land in.
                            if (unchecked(Environment.TickCount - _rectLogTick) >= 1000)
                            {
                                _rectLogTick = Environment.TickCount;
                                AppLog.WriteSync("Win2D: rect (" + c.f0.ToString("0.#") + ","
                                    + c.f1.ToString("0.#") + "," + c.f2.ToString("0.#") + ","
                                    + c.f3.ToString("0.#") + ") a=" + c.f7.ToString("0.##")
                                    + ClipTag() + " i=" + _portCmdIdx);
                            }
                            break;
                        }

                    case 2: // Line: x1/y1/x2/y2, thickness, then r/g/b, and the alpha in `vstart`
                        ds.DrawLine(
                            c.f0, c.f1, c.f2, c.f3,
                            FloatColor(c.f5, c.f6, c.f7, AsFloat(c.vstart)),
                            c.f4);
                        kindLine += System.Diagnostics.Stopwatch.GetTimestamp() - kt0;
                        break;

                    case 3: // Circle: cx/cy/r then r/g/b/a
                        ds.FillCircle(
                            c.f0, c.f1, c.f2,
                            FloatColor(c.f3, c.f4, c.f5, c.f6));
                        kindCirc += System.Diagnostics.Stopwatch.GetTimestamp() - kt0;
                        break;

                    case 4: // Geometry: vertices carry position, uv and colour
                        DrawPortGeometry(ds, frame, c);
                        kindGeo += System.Diagnostics.Stopwatch.GetTimestamp() - kt0;
                        break;

                    case 5: // Scissor: state rather than a draw - everything after it is clipped
                        ApplyPortClip(ds, c, pxToDip);
                        kindClip += System.Diagnostics.Stopwatch.GetTimestamp() - kt0;
                        break;
                }
            }

            // `prpr::ui::scissor` restores whatever was in force before it on the way out of every
            // region it opens, so this is normally already false. Popping here is what stops a frame
            // that ever ended mid-clip from carrying that clip into the next one - which would hide
            // part of the next frame outright rather than merely draw it wrong.
            if (_portClipLayer != null)
            {
                _portClipLayer.Dispose();
                _portClipLayer = null;
            }

            ds.Transform = portTransform;
            long tAfterLoop = System.Diagnostics.Stopwatch.GetTimestamp();

            // Geometry census, throttled to ~1 Hz - see the method below.
            //
            // Deliberately *after* the replay loop instead of before it, which is where it sat
            // until the 00:49:41 crash. This line is `WriteSync`, so it is on disk the moment it
            // returns - but while it ran first it could only prove the frame had been *reached*.
            // A process that died inside the loop therefore left `port frame n=77` looking
            // exactly like the last line of a healthy frame, which is why that death could not be
            // read off the log at all. Positioned here, the line's presence *is* the proof that
            // all `n` commands replayed, and `untext`/`texdraw` emitted from inside the loop with
            // no census behind them proves the opposite while naming the last command reached via
            // `i=`. Same single line per second, same cost - only later in the same method.
            PortFrameStats(frame, cmds);
            long tEnd = System.Diagnostics.Stopwatch.GetTimestamp();
            _drawTicks += tEnd - drawT0;
            SlowDrawLog(drawT0, tAfterPoll, tAfterLoop, tEnd, frame.Count,
                kindTex, kindRect, kindLine, kindCirc, kindGeo, kindClip);
            DrawMeter();
        }

        /// <summary>
        /// Names the part when one draw takes longer than half a second: texture polling and
        /// uploading (<c>poll</c>), command replay (<c>loop</c>), or the tail after it
        /// (<c>tail</c>: clip-layer dispose, the frame census, the meter). The per-kind split
        /// says which command family to batch: clip layers, geometries, lines, or textures.
        /// Quiet otherwise - the per-second meter already covers healthy draws. The Gen2
        /// figure is the GC-alibi check: a 500 ms draw with a climbing Gen2 is the collector,
        /// not the replay loop.
        /// </summary>
        private void SlowDrawLog(long t0, long tPoll, long tLoop, long tEnd, int cmds,
            long kindTex, long kindRect, long kindLine, long kindCirc, long kindGeo, long kindClip)
        {
            double perMs = 1000.0 / (double)System.Diagnostics.Stopwatch.Frequency;
            double total = (tEnd - t0) * perMs;
            if (total < 500.0)
            {
                return;
            }
            int gc2 = 0;
            try { gc2 = System.GC.CollectionCount(2); }
            catch (Exception) { }
            AppLog.WriteSync("Win2D: slow draw total=" + total.ToString("0")
                + "ms poll=" + ((tPoll - t0) * perMs).ToString("0")
                + "ms loop=" + ((tLoop - tPoll) * perMs).ToString("0")
                + "ms tail=" + ((tEnd - tLoop) * perMs).ToString("0")
                + "ms cmds=" + cmds + " gc2=" + gc2
                + " ktex=" + (kindTex * perMs).ToString("0")
                + " krect=" + (kindRect * perMs).ToString("0")
                + " kline=" + (kindLine * perMs).ToString("0")
                + " kcirc=" + (kindCirc * perMs).ToString("0")
                + " kgeo=" + (kindGeo * perMs).ToString("0")
                + " kclip=" + (kindClip * perMs).ToString("0"));
        }

        /// <summary>
        /// The draw-cost line the fps line was missing: wall milliseconds spent inside
        /// <see cref="OnPortDraw"/> (uploads, carrier creation, replay) plus how much of it
        /// was carrier creation alone and how many carriers that built. Once a second, then
        /// the accumulators reset. Queued, not sync: this is a meter, not a crash marker.
        /// </summary>
        private void DrawMeter()
        {
            int now = Environment.TickCount;
            if (unchecked(now - _drawMeterTick) < 1000)
            {
                return;
            }
            _drawMeterTick = now;
            double perMs = 1000.0 / (double)System.Diagnostics.Stopwatch.Frequency;
            AppLog.Write("Win2D: draw ms=" + (_drawTicks * perMs).ToString("0.0")
                + " carrierMs=" + (_carrierTicks * perMs).ToString("0.0")
                + " carrierN=" + _carrierCount);
            _drawTicks = 0;
            _carrierTicks = 0;
            _carrierCount = 0;
        }

        /// <summary>
        /// The clip currently open on the session, or null.
        ///
        /// Win2D.win81 has no <c>PushAxisAlignedClip</c> - a clip is a *layer*, an object that has
        /// to be disposed to end it - so the flag that used to say "one is pushed" is now the layer
        /// itself. Holding the layer rather than a bool is what makes the release safe: there is no
        /// way to forget which clip to end, because the thing that ends it is the field.
        /// </summary>
        private IDisposable _portClipLayer;

        /// <summary>
        /// Applies one <c>PortKind.Scissor</c> record: <c>flags</c> 1 to clip, 0 to release.
        ///
        /// Two things this has to get right, neither of which is visible when it is wrong - it just
        /// looks like the list ignores its box, which is the bug this fixes:
        ///
        /// * *Units.* The rectangle arrives in Rust pixels and Win2D wants DIPs. The transform is
        ///   dropped to identity across the push so the px -&gt; DIP scale cannot be applied twice:
        ///   with the scale installed the clip would be scaled by whatever convention this Win2D
        ///   build uses on top of the conversion already done here. Identity makes both
        ///   conventions agree, so the result does not depend on which one it is.
        /// * *Replace, do not accumulate.* `prpr::ui::scissor` already intersects a nested clip
        ///   with its parent before emitting it, so opening a second layer on top of the first
        ///   would clip twice - and `glScissor` (what this replaces) never stacked in the first
        ///   place. Every entry starts by closing the previous layer, so at most one is live.
        ///
        /// An empty rectangle means "clip everything", which is what prpr emits when a region has
        /// scrolled fully out of view; negatives are clamped because the tuple is a difference of
        /// two coordinates and an intersection can invert it.
        /// </summary>
        private void ApplyPortClip(CanvasDrawingSession ds, PrprPort.PortCmd c, float pxToDip)
        {
            if (_portClipLayer != null)
            {
                _portClipLayer.Dispose();
                _portClipLayer = null;
            }
            _clipRectTag = "";

            float w = c.f2 < 0f ? 0f : c.f2;
            float h = c.f3 < 0f ? 0f : c.f3;

            // Logged on both edges. `clipOn`/`clipOff` on the frame summary only say that a
            // layer was opened; what it was opened *to* is the number that decides whether
            // anything inside it can appear, and an empty or far-off-screen rectangle makes
            // every such draw vanish while that summary still reads healthy. Change-triggered
            // and capped, because sixty frames a second of the same two lines would bury the
            // rest of the log.
            string sig = (c.flags == 0 ? "off" : "on") + " (" + c.f0.ToString("0.#") + ","
                + c.f1.ToString("0.#") + "," + w.ToString("0.#") + "," + h.ToString("0.#") + ")";
            int now = Environment.TickCount;
            if (unchecked(now - _clipLogTick) >= 1000)
            {
                _clipLogTick = now;
                _clipLogCount = 0;
                _clipSig = "";
            }
            if (_clipLogCount < 8 && sig != _clipSig)
            {
                _clipLogCount++;
                _clipSig = sig;
                AppLog.Write("Win2D: clip " + sig);
            }

            if (c.flags == 0)
            {
                return;
            }

            Matrix3x2 saved = ds.Transform;
            ds.Transform = Matrix3x2.Identity;
            _portClipLayer = ds.CreateLayer(
                1f,
                new Rect(c.f0 * pxToDip, c.f1 * pxToDip, w * pxToDip, h * pxToDip));
            ds.Transform = saved;
            _clipRectTag = sig.Substring(3);
        }

        /// <summary>
        /// Where the draws on this line were allowed to land, or `none`.
        ///
        /// Appended to every probe that describes a draw, so those numbers are read against the
        /// box they were drawn inside rather than on their own. A glyph can have an on-screen
        /// destination, a source inside the atlas, a non-zero alpha and a carrier with content,
        /// and still paint nothing if that box does not contain it.
        /// </summary>
        private string ClipTag()
        {
            return _portClipLayer == null ? " clp=none" : " clp=" + _clipRectTag;
        }

        /// <summary>Rate gate for <see cref="PortFrameStats"/>.</summary>
        private int _portStatsTick = int.MinValue / 2;

        /// <summary>Last composition signature logged by <see cref="PortFrameStats"/>.</summary>
        private string _lastStatsSig = "";

        /// <summary>
        /// One line per second describing what the frame actually contained.
        ///
        /// It separates the three ways a frame can look wrong, none of which the command *count* -
        /// the only thing recorded before this existed - is able to tell apart:
        ///
        /// * `geo=0` while the chart should be on screen: the geometry was never emitted, so no
        ///   amount of fixing the renderer would help;
        /// * a `rectBbox` far larger than the canvas while the screen shows a cropped picture: the
        ///   px -> DIP scale is missing, which is the "wrong resolution" symptom rather than a
        ///   missing picture - it also hides whatever sits below or right of the middle;
        /// * a sane `vertBbox` while the screen shows nothing: the transform is right and the fault
        ///   is in the texture content instead - which is what a white sprite looks like from the
        ///   outside, because it has the correct geometry and the wrong pixels.
        ///
        /// The `canvasDip`/`pxToDip` pair is printed with it because those two are the whole
        /// calculation: the projection is in pixels, the canvas is in DIPs, and a mismatch between
        /// them is invisible until the numbers are side by side.
        ///
        /// Walked at most once a second rather than per frame: a busy chart has a few hundred
        /// commands and this reads all of them, which is real work to do sixty times over.
        /// Change-gated on top: only a new composition logs, so a static scene costs the walk
        /// and nothing else - the string, the broker calls and the file write all ride on
        /// change, which is what keeps this off the hitch list.
        /// </summary>
        private void PortFrameStats(PrprPort.DrainFrame frame, PrprPort.PortCmd[] cmds)
        {
            int now = Environment.TickCount;
            if (unchecked(now - _portStatsTick) < 1000)
            {
                return;
            }
            _portStatsTick = now;

            int nTex = 0, nRect = 0, nLine = 0, nCircle = 0, nGeo = 0, nClipOn = 0, nClipOff = 0;
            // Command indices, not counts: everything above counts *how many* of each kind the
            // frame had, and a frame can have the right counts in the wrong order. The ending
            // page shows its cover and hides the panel and glyphs drawn on top of it, which is
            // only contradictory if the cover is first - so the indices settle it outright.
            int iTex = -1, iRect = -1, iGeoFirst = -1, iGeoLast = -1, iClipFirst = -1;
            string rectColor = "-";
            double minX = double.MaxValue, minY = double.MaxValue;
            double maxX = double.MinValue, maxY = double.MinValue;
            double vx0 = double.MaxValue, vy0 = double.MaxValue;
            double vx1 = double.MinValue, vy1 = double.MinValue;
            int nVert = 0;

            // Texture coverage, which the command counts cannot show. A geometry batch whose texture
            // the shell has no carrier for is *skipped*, so `geo` still counts it and the frame
            // still looks busy while nothing appears - and "the notes and the UI are missing" is
            // exactly that, seen from the outside. These two numbers name it.
            int nGeoTex = 0, nGeoTexMissing = 0;
            // How many batches each texture id accounted for. `geoTexId` below names only the
            // *first* textured batch, so "the note sprites are emitted" and "the note sprites are
            // never emitted" produce the same line - a chart with 406 notes draws every one of
            // them every frame, and one id beside a total cannot say which sprite that id is, or
            // whether any note sprite is in the frame at all. On this target the four note
            // textures are 77=click 78=hold 79=flick 80=drag, so this one field answers the
            // question directly: if 80 carries commands and 77 carries none, Rust is emitting
            // drags and nothing else; if 77 carries them too, the draws reach the shell and the
            // fault is downstream of here.
            Dictionary<uint, int> geoTexCounts = new Dictionary<uint, int>();
            // uv extents across every batch that actually names a texture, plus one such id.
            // `noCarrier` cannot see this class of fault: the carrier is present and the alpha
            // is non-zero and nothing is skipped, yet the draw still yields nothing, because
            // the source rectangle is derived from these numbers and lands off the atlas.
            float uvMinU = float.MaxValue, uvMinV = float.MaxValue;
            float uvMaxU = float.MinValue, uvMaxV = float.MinValue;
            int nUv = 0;
            uint geoTexId = 0;
            // Where the textured batches actually sit, and whether any coordinate is NaN.
            // `vertBbox` cannot answer either question: it is dominated by the background
            // rect, which spans the whole screen whatever the text does, and a NaN
            // coordinate is skipped by the `<` comparisons rather than showing up as an
            // extreme - so a frame full of broken glyph positions still reads as a normal
            // full-screen bbox. These two close that gap.
            float tgx0 = float.MaxValue, tgy0 = float.MaxValue;
            float tgx1 = float.MinValue, tgy1 = float.MinValue;
            int nTGeo = 0, nTGeoBad = 0;
            // The first geometry vertex's colour and how many of the frame's vertices are fully
            // transparent. A stride mismatch between the shim's record and this reader shows up here
            // as an all-zero colour long before it shows up as a blank screen.
            string firstVtx = "-";
            int nAlphaZero = 0;

            for (int i = 0; i < frame.Count; i++)
            {
                PrprPort.PortCmd c = cmds[i];
                switch (c.kind & 0xFFFF)
                {
                    case 0:
                        nTex++;
                        if (iTex < 0) iTex = i;
                        break;

                    case 1:
                        nRect++;
                        if (iRect < 0)
                        {
                            iRect = i;
                            rectColor = c.f4.ToString("0.##") + "," + c.f5.ToString("0.##")
                                + "," + c.f6.ToString("0.##") + "," + c.f7.ToString("0.##");
                        }
                        if (c.f0 < minX) minX = c.f0;
                        if (c.f1 < minY) minY = c.f1;
                        if (c.f0 + c.f2 > maxX) maxX = c.f0 + c.f2;
                        if (c.f1 + c.f3 > maxY) maxY = c.f1 + c.f3;
                        break;

                    case 2:
                        nLine++;
                        break;

                    case 3:
                        nCircle++;
                        break;

                    case 4:
                        nGeo++;
                        if (iGeoFirst < 0) iGeoFirst = i;
                        iGeoLast = i;
                        // The positions live in the vertex arena, not in `c.f0..f5`: for kind 4
                        // `f0` is the draw mode (0 = triangles, 1 = lines) and f1..f5 are unused.
                        // The first version of this logged those fields and duly reported
                        // `firstGeo=0,0,0,0,0,0` for every frame ever drawn, which said nothing
                        // except that triangles are triangles. Where the geometry lands is the
                        // question worth asking, and only the vertices answer it.
                        {
                            int vs = (int)c.vstart;
                            int ve = vs + (int)c.vcount;
                            if (vs < 0) vs = 0;
                            if (frame.Vertices != null && ve > frame.Vertices.Length)
                            {
                                ve = frame.Vertices.Length;
                            }
                            if (c.flags != 0)
                            {
                                nGeoTex++;
                                if (geoTexId == 0)
                                {
                                    geoTexId = c.flags;
                                }
                                if (!_portTextures.ContainsKey(c.flags))
                                {
                                    nGeoTexMissing++;
                                }
                                int nOfTex;
                                geoTexCounts.TryGetValue(c.flags, out nOfTex);
                                geoTexCounts[c.flags] = nOfTex + 1;
                            }
                            for (int vi = vs; vi < ve; vi++)
                            {
                                if (c.flags != 0)
                                {
                                    float u = frame.Vertices[vi].u;
                                    float v = frame.Vertices[vi].v;
                                    if (u < uvMinU) uvMinU = u;
                                    if (u > uvMaxU) uvMaxU = u;
                                    if (v < uvMinV) uvMinV = v;
                                    if (v > uvMaxV) uvMaxV = v;
                                    nUv++;
                                }
                                float gx = frame.Vertices[vi].x;
                                float gy = frame.Vertices[vi].y;
                                if (c.flags != 0)
                                {
                                    if (float.IsNaN(gx) || float.IsInfinity(gx)
                                        || float.IsNaN(gy) || float.IsInfinity(gy))
                                    {
                                        nTGeoBad++;
                                    }
                                    else
                                    {
                                        if (gx < tgx0) tgx0 = gx;
                                        if (gx > tgx1) tgx1 = gx;
                                        if (gy < tgy0) tgy0 = gy;
                                        if (gy > tgy1) tgy1 = gy;
                                        nTGeo++;
                                    }
                                }
                                if (gx < vx0) vx0 = gx;
                                if (gy < vy0) vy0 = gy;
                                if (gx > vx1) vx1 = gx;
                                if (gy > vy1) vy1 = gy;
                                if (frame.Vertices[vi].a <= 0.003f)
                                {
                                    nAlphaZero++;
                                }
                            }
                            if (firstVtx == "-" && ve > vs)
                            {
                                PrprPort.PortVertex fv = frame.Vertices[vs];
                                firstVtx = fv.r.ToString("0.##") + "," + fv.g.ToString("0.##")
                                    + "," + fv.b.ToString("0.##") + "," + fv.a.ToString("0.##");
                            }
                            nVert += ve - vs;
                        }
                        break;

                    case 5:
                        // `flags` says whether this opens or closes a clip, so the two counts
                        // together are the check: a frame that opens more than it closes (or the
                        // reverse) is a prpr-side imbalance, and a frame with `clipOn=0` while a
                        // scrollable list is on screen means the scissor never reached here.
                        if (c.flags != 0)
                        {
                            nClipOn++;
                            if (iClipFirst < 0) iClipFirst = i;
                        }
                        else
                        {
                            nClipOff++;
                        }
                        break;
                }
            }

            int lastKind = frame.Count > 0
                ? (int)(cmds[frame.Count - 1].kind & 0xFFFF)
                : -1;

            string bbox = nRect == 0
                ? "-"
                : "(" + minX.ToString("0.#") + "," + minY.ToString("0.#") + ","
                    + (maxX - minX).ToString("0.#") + "x" + (maxY - minY).ToString("0.#") + ")";

            // The vertex span is what the px -> DIP scale is judged against: it is in Rust pixels,
            // and the canvas figure right beside it is the DIPs those pixels have to fit inside.
            string vertBbox = nVert == 0
                ? "-"
                : "(" + vx0.ToString("0.#") + "," + vy0.ToString("0.#") + ")-("
                    + vx1.ToString("0.#") + "," + vy1.ToString("0.#") + ")n=" + nVert;

            string tgSummary = nTGeo == 0
                ? "-"
                : "(" + tgx0.ToString("0.#") + "," + tgy0.ToString("0.#") + ")-("
                  + tgx1.ToString("0.#") + "," + tgy1.ToString("0.#") + ")";

            string uvSummary = nUv == 0
                ? "-"
                : uvMinU.ToString("0.###") + "," + uvMinV.ToString("0.###")
                  + "-" + uvMaxU.ToString("0.###") + "," + uvMaxV.ToString("0.###");

            // `id x batch-count`, ids ascending. Sorted so two frames can be diffed by eye, and
            // only built on the second the line is written - this method already pays for the
            // dictionary once a second, not once a frame.
            string geoTexSummary = "-";
            if (geoTexCounts.Count > 0)
            {
                List<uint> texIds = new List<uint>(geoTexCounts.Keys);
                texIds.Sort();
                geoTexSummary = "";
                for (int k = 0; k < texIds.Count; k++)
                {
                    if (k > 0)
                    {
                        geoTexSummary += ",";
                    }
                    geoTexSummary += texIds[k] + "x" + geoTexCounts[texIds[k]];
                }
            }

            double dipW = _canvas == null ? 0.0 : _canvas.ActualWidth;
            double dipH = _canvas == null ? 0.0 : _canvas.ActualHeight;
            string pxToDip = (PrprPort.ScreenPixelWidth > 0f && dipW > 0.0)
                ? (dipW / PrprPort.ScreenPixelWidth).ToString("0.###")
                : "?";

            // Change-gated: a static scene repeats the identical frame for minutes, and the
            // walk above plus the three broker calls plus the 500 ms-bounded `WriteSync` below
            // cost ~500 ms on this phone - every second, on the UI thread, which is exactly
            // the periodic hitch in the report. Unchanged composition logs nothing; a new
            // composition still gets the full line. Crash-tail duty moved to the slow-draw
            // and stall lines, which fire on the event rather than on the clock.
            string sig = frame.Count + "/" + nTex + "/" + nRect + "/" + nLine + "/" + nCircle
                + "/" + nGeo + "/" + nClipOn + "/" + nClipOff + "/" + lastKind;
            if (sig == _lastStatsSig)
            {
                return;
            }
            _lastStatsSig = sig;

            AppLog.Write("Win2D: port frame n=" + frame.Count
                + " tex=" + nTex + " rect=" + nRect + " line=" + nLine
                + " circ=" + nCircle + " geo=" + nGeo
                + " clipOn=" + nClipOn + " clipOff=" + nClipOff
                + " particles=" + PrprPort.SpriteQuads()
                + " pd=" + PrprPort.ParticleDiag(0) + "/" + PrprPort.ParticleDiag(1)
                + " geoTex=" + nGeoTex + " noCarrier=" + nGeoTexMissing
                // The one number none of the fields above can show. A textured batch is drawn
                // by mapping [u0,u1]x[v0,v1] off the atlas onto the destination quad; if the uv
                // is not normalised to the atlas, that source rectangle falls outside the
                // image and `DrawImage` produces nothing at all - no exception, no missing
                // carrier, no alpha0, just silence. That is exactly the report: gradient bars
                // (untextured geometry) and the background (its own texture command) draw,
                // while every glyph and sprite does not.
                + " geoUv=" + uvSummary + " geoTexId=" + geoTexId
                // The rest of that id's company. One id proves *a* textured batch survived; this
                // is every texture that carried batches this frame and how many each one got.
                + " geoTexCounts=" + geoTexSummary
                // Paint order, which is the only reading the counts above cannot give. A
                // cover drawn last hides every panel and glyph the page put in front of it
                // while leaving itself perfectly visible - the exact reported picture, and
                // the only fault the rest of this line is unable to see.
                + " ord=t" + iTex + " r" + iRect + " g" + iGeoFirst + ".." + iGeoLast
                + " c" + iClipFirst + " last=" + lastKind + " rc=" + rectColor
                + " geoBbox=" + tgSummary + " geoNaN=" + nTGeoBad
                + " alpha0=" + nAlphaZero + "/" + nVert + " vtx0=" + firstVtx
                + " rectBbox=" + bbox + " vertBbox=" + vertBbox
                + " canvasDip=" + dipW.ToString("0.##") + "x" + dipH.ToString("0.##")
                + " pxToDip=" + pxToDip
                // The registry state travels with the frame line because it is the other half of
                // the same question: a texture the registry refused never becomes a carrier, and a
                // carrier that is missing is what `noCarrier` above counts. Printing only the
                // latter says a sprite is missing without saying why.
                + " | " + PrprPort.TextureSummary());
        }

        private void DrawPortTexture(CanvasDrawingSession ds, PrprPort.PortCmd c)
        {
            PortTexture tex;
            if (!_portTextures.TryGetValue(c.flags, out tex) || tex == null || tex.Target == null)
            {
                // No carrier for this id - the shell never loaded it, or the registry dropped it.
                // Skipping is right: an untextured quad is noise, not a missing-no warning.
                return;
            }

            float dx = c.f0, dy = c.f1, dw = c.f2, dh = c.f3;
            float sx = c.f4, sy = c.f5, sw = c.f6, sh = c.f7;
            if (dw <= 0f || dh <= 0f || sw <= 0f || sh <= 0f)
            {
                return;
            }

            bool flipX = (c.kind & (1u << 16)) != 0;
            bool flipY = (c.kind & (1u << 17)) != 0;

            // Source rect in the carrier's own pixel space. The flip flags are folded into the
            // destination rectangle instead of the source, because Win2D.win81's DrawImage takes the
            // source as an axis-aligned Rect and cannot mirror it.
            Rect source = new Rect(sx, sy, sw, sh);
            ICanvasImage image = tex.Target;

            // The tint's alpha, carried in `icount` - see the port's `PortCmd` doc. Only the alpha
            // can be applied: Win2D.win81's `DrawImage` takes an opacity but no tint, and the
            // tinted cases `prpr` actually uses (judge-line textures, the `hit_fx` judgement tint)
            // are white-tinted in the common path. The RGB is decoded anyway so the gap is visible
            // here rather than discovered as "the effect looks washed out".
            float opacity = AsFloat(c.icount);
            if (opacity <= 0.003f)
            {
                return;
            }
            if (opacity > 1f)
            {
                opacity = 1f;
            }

            Matrix3x2 saved = ds.Transform;
            if (flipX || flipY)
            {
                float cx = dx + dw / 2f;
                float cy = dy + dh / 2f;
                // `* saved` is what keeps the px -> DIP scale installed by `OnPortDraw` in force:
                // the mirror and the centring are both expressed in Rust pixels, and `saved` is the
                // thing that turns those pixels into DIPs. Dropping it put flipped sprites at twice
                // their intended position on a 2x screen, i.e. half of them off the canvas.
                ds.Transform = Matrix3x2.CreateScale(flipX ? -1f : 1f, flipY ? -1f : 1f)
                             * Matrix3x2.CreateTranslation(cx, cy)
                             * saved;
                ds.DrawImage(image, new Rect(-dw / 2f, -dh / 2f, dw, dh), source, opacity, CanvasImageInterpolation.Linear);
                ds.Transform = saved;
            }
            else
            {
                ds.DrawImage(image, new Rect(dx, dy, dw, dh), source, opacity, CanvasImageInterpolation.Linear);
            }
        }

        private void DrawPortGeometry(CanvasDrawingSession ds, PrprPort.DrainFrame frame,
                                      PrprPort.PortCmd c)
        {
            if (c.vcount == 0 || c.icount == 0)
            {
                return;
            }

            int mode = (int)c.f0;
            bool hasTex = c.flags != 0 && _portTextures.ContainsKey(c.flags);

            // A geometry batch carries the texture that was bound when prpr pushed it
            // (`macroquad::draw::set_texture`, recorded as `rec.flags`) and 0 for "none". A
            // non-zero id with no carrier means the shell has not built that carrier yet - the
            // glyph atlas alone takes ~700 ms, and the first frames run 350 ms each - and
            // `FillPortTriangle` reads "no carrier" as "untextured" and flat-fills the triangle
            // with the vertex colour. prpr writes a glyph's *shape* into the atlas and only the
            // fade into the vertex alpha (`prpr/src/ui/text.rs:349`), so that fill paints every
            // glyph quad solid white. This is the white-block report: a race between the first
            // frames and texture construction, not a font, an asset or an alpha problem.
            //
            // Skipped rather than waited for, and gated on the registry rather than on the shell.
            //
            // The old rule here - "an id the registry no longer holds is stale, so keep
            // flat-filling" - is what the device falsified. prpr's untextured note bodies and
            // judge-line strokes carry `c.flags == 0` and never reach this branch at all, so a
            // non-zero id that nothing can produce pixels for is *only ever* a textured draw the
            // shell has lost: flat-filling it is not a fallback, it is the wrong picture. And the
            // wrong picture is not subtle - prpr keeps a glyph's shape in the atlas and puts
            // nothing but the fade in the vertex colour (`prpr/src/ui/text.rs:349`), so every
            // affected quad becomes one solid white rectangle.
            //
            // There are two ways to get here and both are refused:
            //   pending - the registry still holds the id; the carrier is still being built and
            //             the next frame or three draws normally.
            //   miss    - the registry does not hold it, which `text.rs:367` does on purpose:
            //             growing the glyph atlas calls `cache_texture.delete()` on an id that
            //             this very frame's batches still name. Those quads are unrecoverable by
            //             definition - prpr pushes the next frame against the new id - so drawing
            //             nothing is the entire recovery. On the device this was not a frame or
            //             three: `noCarrier=161` stood for 101 seconds, because a miss is stable
            //             until prpr rebinds, and flat-filling is what made it a screen of white.
            bool carrierPending = false;
            bool registryMiss = false;
            if (c.flags != 0 && !hasTex)
            {
                uint pendingW, pendingH;
                carrierPending = PrprPort.TextureSize(c.flags, out pendingW, out pendingH);
                registryMiss = !carrierPending;
            }

            // The duplicate-quad memo is per command: it exists to fold the two triangles of one
            // quad together, and carrying it across commands would drop a legitimately repeated
            // sprite.
            _hasLastQuad = false;

            // The vertices arrive as a flat array; the index list is re-based to `c.istart` already,
            // so the shell reads the shared arena through it.
            for (int t = 0; t + 2 < (int)c.icount || (mode != 0 && t + 1 < (int)c.icount); t += (mode == 0 ? 3 : 2))
            {
                int i0 = (int)c.istart + t;
                int i1 = (int)c.istart + t + 1;
                int i2 = (int)c.istart + t + 2;

                if (i0 >= frame.Indices.Length) break;
                PrprPort.PortVertex v0 = Vert(frame, (int)c.vstart + (int)frame.Indices[i0]);
                PrprPort.PortVertex v1 = Vert(frame, (int)c.vstart + (int)frame.Indices[i1]);

                if (mode == 0)
                {
                    if (i2 >= frame.Indices.Length) break;

                    // A prpr sprite is a quad emitted as two triangles, and the bounding-box
                    // approximation below cannot express a rotated one - it comes out stretched,
                    // which is exactly what a vertical note looked like on the device. Try the exact
                    // affine path first; it consumes six indices (both triangles) when it succeeds.
                    if (hasTex && TryDrawPortQuad(ds, frame, c, t))
                    {
                        t += 3;
                        continue;
                    }

                    if (carrierPending || registryMiss)
                    {
                        // Named a texture this shell cannot draw textured, so draw nothing at all
                        // rather than a solid white block in its place. The two reasons differ only
                        // in when the picture comes back, never in whether flat-filling was right.
                        //
                        // Counted, because the point of this branch is that it stops being taken:
                        // a count that never moves says the race is gone, and one that keeps
                        // climbing says a carrier is being built and lost again, which would be a
                        // different bug wearing the same symptom. The reason rides along - "pending"
                        // and "registry miss" are different bugs and this is the only place that
                        // can tell them apart from the log.
                        _carrierPendingSkipped++;
                        // Every 5000th, not every 64th. A miss happens once per textured batch
                        // per frame - about 1400/second here - so a 64-wide stride logged ~23
                        // lines/second and rolled the 256 KB log every three minutes, taking the
                        // music and sfx lines with it. The per-frame `noCarrier=` on the `port
                        // frame` line already tracks this continuously; this one only has to
                        // prove the reason and that the count still moves.
                        if (_carrierPendingSkipped == 1 || (_carrierPendingSkipped % 5000) == 0)
                        {
                            AppLog.WriteSync("Win2D: geometry id=" + c.flags
                                + " skipped, " + (carrierPending ? "carrier pending" : "registry miss")
                                + " (#" + _carrierPendingSkipped + ")");
                        }
                        continue;
                    }

                    PrprPort.PortVertex v2 = Vert(frame, (int)c.vstart + (int)frame.Indices[i2]);
                    FillPortTriangle(ds, v0, v1, v2, c.flags, hasTex);
                }
                else
                {
                    // Lines / line strips: the shim uses these for hold bodies, which are plain
                    // strokes rather than filled geometry. The vertex alpha is the hold's fade, so
                    // it is carried rather than forced opaque.
                    ds.DrawLine(v0.x, v0.y, v1.x, v1.y,
                        FloatColor(v0.r, v0.g, v0.b, v0.a), 2f);
                }
            }
        }

        private static PrprPort.PortVertex Vert(PrprPort.DrainFrame frame, int index)
        {
            if (index < 0 || index >= frame.Vertices.Length)
            {
                return new PrprPort.PortVertex();
            }
            return frame.Vertices[index];
        }

        /// <summary>
        /// Fills one triangle. Textured when the batch names a carrier the shell has, flat-coloured
        /// otherwise - which is what `prpr`'s untextured note bodies rely on.
        ///
        /// The image arrives as <see cref="ICanvasImage"/> rather than as either concrete type
        /// because both <see cref="CanvasBitmap"/> and <see cref="CanvasRenderTarget"/> satisfy it
        /// and the fill needs nothing beyond "draw this, from that source rectangle". The size does
        /// *not* come from the image: uv is relative to the atlas, whose pixel size was fixed when
        /// the carrier was built, and asking the image how big it is would silently re-derive that
        /// from its metrics the moment the two ever disagreed.
        /// </summary>
        /// <summary>
        /// Draws the triangle pair at <paramref name="t"/> as one textured quad, exactly.
        ///
        /// <see cref="FillPortTriangle"/> can only approximate a sprite by its axis-aligned bounding
        /// box, which is right for an upright note and visibly wrong for a rotated one - a note on a
        /// tilted judge line came out stretched, because the box of a tilted quad is larger than the
        /// quad and the texture is stretched to fill it. Win2D has no arbitrary-triangle texture
        /// fill, but it does have an affine draw transform, and a prpr sprite is always a
        /// rotated-and-scaled rectangle - a *parallelogram*, which an affine map represents exactly.
        ///
        /// The four corners are found by their uv extremes rather than by de-duplicating positions:
        /// there is no tolerance to tune and it cannot be confused by two vertices landing on the
        /// same pixel. Anything that is not a parallelogram (or has no carrier) is left to the
        /// caller's fallback.
        /// </summary>
        private bool TryDrawPortQuad(CanvasDrawingSession ds, PrprPort.DrainFrame frame,
                                     PrprPort.PortCmd c, int t)
        {
            if (t + 5 >= (int)c.icount || frame.Indices == null)
            {
                return false;
            }

            // Exactly four distinct vertices, or this is not a quad. Six indices that name five or
            // six vertices mean the window straddles two shapes (a hold's body and its head are
            // batched together), and taking the uv extremes across them builds an affine map from
            // one shape's corners to another's - which draws as a skewed, stretched sprite behind
            // the real one. That is the "a short hold suddenly grows a slanted copy" report.
            int i0 = (int)frame.Indices[(int)c.istart + t];
            int i1 = (int)frame.Indices[(int)c.istart + t + 1];
            int i2 = (int)frame.Indices[(int)c.istart + t + 2];
            int i3 = (int)frame.Indices[(int)c.istart + t + 3];
            int i4 = (int)frame.Indices[(int)c.istart + t + 4];
            int i5 = (int)frame.Indices[(int)c.istart + t + 5];
            int unique = 1;
            int[] seen4 = { i0, -1, -1, -1 };
            for (int k = 0; k < 6; k++)
            {
                int v = k == 0 ? i0 : (k == 1 ? i1 : (k == 2 ? i2 : (k == 3 ? i3 : (k == 4 ? i4 : i5))));
                bool known = false;
                for (int j = 0; j < unique; j++)
                {
                    if (seen4[j] == v)
                    {
                        known = true;
                        break;
                    }
                }
                if (known)
                {
                    continue;
                }
                if (unique >= 4)
                {
                    // A fifth distinct vertex: not a quad.
                    return false;
                }
                seen4[unique] = v;
                unique++;
            }
            if (unique != 4)
            {
                return false;
            }

            PortTexture tex;
            if (!_portTextures.TryGetValue(c.flags, out tex) || tex == null || tex.Target == null)
            {
                return false;
            }

            int istart = (int)c.istart;
            int vbase = (int)c.vstart;
            _quadVerts[0] = Vert(frame, vbase + i0);
            _quadVerts[1] = Vert(frame, vbase + i1);
            _quadVerts[2] = Vert(frame, vbase + i2);
            _quadVerts[3] = Vert(frame, vbase + i3);
            _quadVerts[4] = Vert(frame, vbase + i4);
            _quadVerts[5] = Vert(frame, vbase + i5);

            float minU = _quadVerts[0].u, maxU = _quadVerts[0].u;
            float minV = _quadVerts[0].v, maxV = _quadVerts[0].v;
            for (int i = 1; i < 6; i++)
            {
                float u = _quadVerts[i].u, v = _quadVerts[i].v;
                if (u < minU) minU = u;
                if (u > maxU) maxU = u;
                if (v < minV) minV = v;
                if (v > maxV) maxV = v;
            }
            if (maxU - minU <= 0f || maxV - minV <= 0f)
            {
                return false;
            }

            // One vertex per uv corner: (min,min) (max,min) (max,max) (min,max).
            int ia = -1, ib = -1, ic = -1, id = -1;
            float sa = float.MaxValue, sb = float.MaxValue, sc = float.MaxValue, sd = float.MaxValue;
            for (int i = 0; i < 6; i++)
            {
                float u = _quadVerts[i].u, v = _quadVerts[i].v;
                float da = (u - minU) + (v - minV);
                float db = (maxU - u) + (v - minV);
                float dc = (maxU - u) + (maxV - v);
                float dd = (u - minU) + (maxV - v);
                if (da < sa) { sa = da; ia = i; }
                if (db < sb) { sb = db; ib = i; }
                if (dc < sc) { sc = dc; ic = i; }
                if (dd < sd) { sd = dd; id = i; }
            }
            if (ia < 0 || ib < 0 || ic < 0 || id < 0)
            {
                return false;
            }

            PrprPort.PortVertex a = _quadVerts[ia];
            PrprPort.PortVertex b = _quadVerts[ib];
            PrprPort.PortVertex cc = _quadVerts[ic];
            PrprPort.PortVertex d = _quadVerts[id];

            // Affine only if the fourth corner follows from the other three; a perspective quad would
            // need the 4x4 overload and prpr never emits one.
            float ex = (a.x + cc.x) - (b.x + d.x);
            float ey = (a.y + cc.y) - (b.y + d.y);
            float qw = Math.Abs(b.x - a.x) + Math.Abs(d.x - a.x);
            float qh = Math.Abs(b.y - a.y) + Math.Abs(d.y - a.y);
            float tol = 0.01f * (qw + qh + 1f);
            if (Math.Abs(ex) > tol || Math.Abs(ey) > tol)
            {
                return false;
            }

            // Axis-aligned quads deliberately go back to the caller's bounding-box path, which is
            // *exact* for them. This function exists for the case that path cannot express - a
            // rotated sprite, which came out stretched - and sending the common case (every glyph,
            // every UI rectangle) through an affine transform as well was needless risk: a wrong
            // matrix there is not a stretched sprite but garbled text, which is precisely what
            // appeared once the judgement word started drawing on the first touch.
            if (Math.Abs(b.y - a.y) <= tol && Math.Abs(d.x - a.x) <= tol)
            {
                return false;
            }

            float sx = minU * tex.PixelWidth;
            float sy = minV * tex.PixelHeight;
            float sw = (maxU - minU) * tex.PixelWidth;
            float sh = (maxV - minV) * tex.PixelHeight;
            if (sw <= 0f || sh <= 0f)
            {
                return false;
            }

            float alpha = (a.a + b.a + cc.a + d.a) / 4f;
            if (alpha > 1f)
            {
                alpha = 1f;
            }
            if (alpha <= 0.003f)
            {
                // Consumed: the quad is real but invisible, and letting the caller fall back would
                // draw its two triangles as boxes.
                return true;
            }

            // Maps the source rectangle onto the quad. `* saved` keeps the px -> DIP scale that
            // `OnPortDraw` installs, exactly as the flip path above does.
            float m11 = (b.x - a.x) / sw;
            float m12 = (b.y - a.y) / sw;
            float m21 = (d.x - a.x) / sh;
            float m22 = (d.y - a.y) / sh;
            Matrix3x2 quadMap = new Matrix3x2(
                m11, m12, m21, m22,
                a.x - m11 * sx - m21 * sy,
                a.y - m12 * sx - m22 * sy);

            Matrix3x2 saved = ds.Transform;
            ds.Transform = quadMap * saved;
            // Drawn at the source rectangle in pre-transform space, so the map lands it on the quad.
            ds.DrawImage(tex.Target, new Rect(sx, sy, sw, sh), new Rect(sx, sy, sw, sh),
                         alpha, CanvasImageInterpolation.Linear);
            ds.Transform = saved;
            return true;
        }

        private void FillPortTriangle(CanvasDrawingSession ds, PrprPort.PortVertex a,
                                      PrprPort.PortVertex b, PrprPort.PortVertex c,
                                      uint texId, bool hasTex)
        {
            // The batch's alpha. `prpr` fades a note, a glyph or a particle by writing the same
            // alpha into all four of its vertices, so the mean is that value; averaging rather than
            // taking `a.a` only matters for a genuinely graded quad, where it is the closer answer.
            float alpha = (a.a + b.a + c.a) / 3f;
            if (alpha <= 0.003f)
            {
                // Fully transparent: the shape is emitted (prpr keeps it in the batch) but there is
                // nothing to draw, and skipping is what keeps a fadeful of invisible quads cheap.
                return;
            }
            if (alpha > 1f)
            {
                alpha = 1f;
            }

            PortTexture tex;
            if (hasTex && _portTextures.TryGetValue(texId, out tex) && tex != null && tex.Target != null)
            {
                // Win2D.win81 has no arbitrary-triangle texture fill, so the batch is approximated by
                // its bounding quad. `prpr`'s note sprites are axis-aligned quads in practice, which
                // makes the approximation exact for them.
                float minX = Math.Min(a.x, Math.Min(b.x, c.x));
                float minY = Math.Min(a.y, Math.Min(b.y, c.y));
                float maxX = Math.Max(a.x, Math.Max(b.x, c.x));
                float maxY = Math.Max(a.y, Math.Max(b.y, c.y));
                if (maxX - minX <= 0f || maxY - minY <= 0f)
                {
                    return;
                }
                float u0 = Math.Min(a.u, Math.Min(b.u, c.u));
                float v0 = Math.Min(a.v, Math.Min(b.v, c.v));
                float u1 = Math.Max(a.u, Math.Max(b.u, c.u));
                float v1 = Math.Max(a.v, Math.Max(b.v, c.v));
                Rect quad = new Rect(minX, minY, maxX - minX, maxY - minY);
                Rect src = new Rect(u0 * tex.PixelWidth, v0 * tex.PixelHeight,
                                    (u1 - u0) * tex.PixelWidth, (v1 - v0) * tex.PixelHeight);

                // The two triangles of an axis-aligned quad give the same bounding rectangle and the
                // same source rectangle, so the second draw simply repeats the first - and under
                // alpha blending a repeat is not a no-op: it darkens every semi-transparent note and
                // particle. Folding them halves the texture draws in a gameplay frame (which is most
                // of that frame's cost) and is what lets a half-faded note be the colour it was
                // meant to be rather than the square of it.
                if (_hasLastQuad && quad == _lastQuad && src == _lastSrc)
                {
                    return;
                }
                _hasLastQuad = true;
                _lastQuad = quad;
                _lastSrc = src;

                // What this draw was really asked to do. The two numbers that decide whether
                // anything can appear are the destination quad (did it land on screen) and
                // the source rectangle (is it inside the atlas's *used* region). A flipped
                // v axis hides best of all: the uv still reads 0..1 and the carrier is still
                // present, but the source rows are the never-written bottom of the atlas, so
                // the draw succeeds and paints transparency. The glyph rows are known from
                // the `port texture ... delta` lines, so `src.Y` settles it.
                _texDraws++;
                // Census rather than sample. The old gate - first three, then every
                // four-thousandth - walks past whatever a static page is showing and keeps
                // reporting the same few shapes, which is how a page can be measured in
                // detail while the element in the report is never logged. Change-triggered,
                // so every distinct destination lands within about a second, with a budget so
                // a scrolling list still cannot flood the file.
                float tdw = maxX - minX, tdh = maxY - minY;
                bool tFresh = minX != _lastTdX || minY != _lastTdY
                    || tdw != _lastTdW || tdh != _lastTdH;
                int tNow = Environment.TickCount;
                if (unchecked(tNow - _tdLogTick) >= 1000)
                {
                    _tdLogTick = tNow;
                    _tdLogBudget = 8;
                    _lastTdX = float.MinValue;
                    tFresh = true;
                }
                if (tFresh && _tdLogBudget > 0)
                {
                    _tdLogBudget--;
                    _lastTdX = minX; _lastTdY = minY; _lastTdW = tdw; _lastTdH = tdh;
                    AppLog.Write("Win2D: texdraw id=" + texId
                        + " dst=(" + minX.ToString("0.#") + "," + minY.ToString("0.#")
                        + "," + (maxX - minX).ToString("0.#") + "," + (maxY - minY).ToString("0.#") + ")"
                        + " src=(" + src.X.ToString("0.#") + "," + src.Y.ToString("0.#")
                        + "," + src.Width.ToString("0.#") + "," + src.Height.ToString("0.#") + ")"
                        + " a=" + alpha.ToString("0.##")
                        + " atlas=" + tex.PixelWidth + "x" + tex.PixelHeight
                        + ClipTag() + " i=" + _portCmdIdx);
                }
                // Tint. Win2D.win81's DrawImage carries opacity but no RGB, so a textured draw
                // lands white whatever prpr asked for - desktop multiplies the vertex colour in
                // the shader, here nothing does. Dark vertex colours (button labels, the
                // selected tab) therefore came out white-on-white. A dark draw re-reads its
                // own source rect, multiplies it by the vertex colour on the CPU and draws the
                // tinted copy instead; static labels converge into the cache on the first frame
                // and cost one extra draw afterwards. Only dark tints take this path (every
                // channel under half): luminous sprite tints stay on the fast white path, and
                // oversized rects fall back rather than allocate megabytes. The rotated-quad
                // path above has no hook - dark rotated quads are vanishingly rare, and notes
                // (its only real traffic) are white-tinted.
                float tintR = (a.r + b.r + c.r) / 3f;
                float tintG = (a.g + b.g + c.g) / 3f;
                float tintB = (a.b + b.b + c.b) / 3f;
                ICanvasImage image = tex.Target;
                Rect imageSrc = src;
                if (tintR < 0.5f && tintG < 0.5f && tintB < 0.5f)
                {
                    CanvasBitmap tinted = TintedCopy(ds.Device, texId, src, tintR, tintG, tintB);
                    if (tinted != null)
                    {
                        image = tinted;
                        imageSrc = new Rect(0, 0, src.Width, src.Height);
                    }
                }
                ds.DrawImage(image, quad, imageSrc, alpha, CanvasImageInterpolation.Linear);
                return;
            }

            // Vertex colours are per-vertex in the shim; a flat fill uses the first. Averaging would
            // be closer to a gradient, but Win2D.win81 cannot interpolate across a triangle either.
            //
            // Disposed, and that is not tidiness. This runs once per *untextured* triangle per
            // frame - prpr draws most of a chart's note bodies and every judge-line stroke this way
            // - and a CanvasGeometry owns a native D2D resource. Leaking one per triangle leaked
            // thousands a second: frame time climbed steadily over a session (78 fps in the first
            // minute, 26 by minute ten) until the render thread was saturated enough that touches
            // were no longer serviced.
            // The untextured half of this method has never had a probe. `geoTex` counts only
            // the batches that name a texture, so a gradient, a panel or a button body can be
            // emitted, drawn and clipped away while the frame summary reads entirely healthy -
            // and the ending page's half-panel is exactly such a shape. One line a second says
            // where it landed, at what alpha and colour, and whether a layer was over it. The
            // box is computed only when the line is due: this runs per untextured triangle per
            // frame, and the arithmetic is not worth doing sixty times over to be thrown away.
            float ux0 = Math.Min(a.x, Math.Min(b.x, c.x));
            float uy0 = Math.Min(a.y, Math.Min(b.y, c.y));
            float ux1 = Math.Max(a.x, Math.Max(b.x, c.x));
            float uy1 = Math.Max(a.y, Math.Max(b.y, c.y));
            // Same census as `texdraw`: the once-a-second gate always caught the first
            // untextured shape in the frame, so a panel or gradient drawn later in the same
            // frame was structurally invisible to this line - which is the shape of the
            // reported missing background.
            bool uFresh = ux0 != _lastUnX || uy0 != _lastUnY
                || (ux1 - ux0) != _lastUnW || (uy1 - uy0) != _lastUnH;
            int uNow = Environment.TickCount;
            if (unchecked(uNow - _unLogTick) >= 1000)
            {
                _unLogTick = uNow;
                _unLogBudget = 8;
                _lastUnX = float.MinValue;
                uFresh = true;
            }
            // `cv*` rather than `v*`: the textured half above already owns `v0`/`v1` as UV bounds,
            // and C# forbids reusing the name anywhere later in the same method body. Computed ahead
            // of the census line instead of after it so that line can say which of the two fills
            // below this triangle is about to run: a death on a flat fill and a death on the
            // gradient fill leave the same surviving line otherwise, and only one of the two
            // implicates the brush cache.
            Color cv0 = FloatColor(a.r, a.g, a.b, a.a);
            Color cv1 = FloatColor(b.r, b.g, b.b, b.a);
            Color cv2 = FloatColor(c.r, c.g, c.b, c.a);
            bool uFlat = cv0 == cv1 && cv1 == cv2;
            bool uLogged = uFresh && _unLogBudget > 0;
            if (uLogged)
            {
                _unLogBudget--;
                _lastUnX = ux0; _lastUnY = uy0;
                _lastUnW = ux1 - ux0; _lastUnH = uy1 - uy0;
                AppLog.Write("Win2D: untext dst=(" + ux0.ToString("0.#") + "," + uy0.ToString("0.#")
                    + "," + (ux1 - ux0).ToString("0.#") + "," + (uy1 - uy0).ToString("0.#")
                    + ") a=" + alpha.ToString("0.##")
                    + " rgb=" + a.r.ToString("0.##") + "," + a.g.ToString("0.##") + "," + a.b.ToString("0.##")
                    + ClipTag() + " i=" + _portCmdIdx + " g=" + (uFlat ? 0 : 1));
            }
            if (uFlat)
            {
                // Genuinely flat - a solid panel, a note body, a judge-line stroke. One fill, and
                // the shape the census above has been measuring all along.
                using (CanvasGeometry tri = PortTriangle(ds, a, b, c))
                {
                    ds.FillGeometry(tri, FloatColor(a.r, a.g, a.b, alpha));
                }
                // Bisect probe. The census line above fires *before* the fill, so a native death
                // inside `PortTriangle`/`FillGeometry` and a death in the command after it leave the
                // exact same surviving line. This one fires after it: a log ending on `untext dst=`
                // means the fill never returned, one ending on `fill-ok` means it did and the fault
                // is downstream. Emitted only when the census above was, so it costs no extra lines.
                // Queued: this fires per fill when the gate is open, and a sync wait per
                // fill is what turned menu draws into seconds. Crash-tail precision drops by
                // the queue depth; the sync frame census still bounds the death to the frame.
                if (uLogged)
                {
                    AppLog.Write("Win2D: untext fill-ok i=" + _portCmdIdx);
                }
                return;
            }

            // The vertices disagree, so this is a real gradient, and filling it with one colour is
            // what turned `rect_shadow`'s outer-edge-to-inner-edge fade into eight flat patches
            // whose alphas alternated 0.33 / 0.67 - the visible seams and horizontal banding in the
            // report. Barycentric vertex colours form an affine field over the triangle, and an
            // affine field is linear along any segment of it, so a linear brush spanning the two
            // vertices whose colours differ most traces prpr's gradient instead of averaging it
            // away. prpr's gradients are one-dimensional by construction (the two outer vertices of
            // a shadow band share a colour), which is the case where this is exact rather than
            // merely closer.
            float d01 = ColorDist(cv0, cv1);
            float d02 = ColorDist(cv0, cv2);
            float d12 = ColorDist(cv1, cv2);
            Color e0, e1;
            float ax, ay, bx, by;
            if (d01 >= d02 && d01 >= d12)
            {
                e0 = cv0; ax = a.x; ay = a.y; e1 = cv1; bx = b.x; by = b.y;
            }
            else if (d02 >= d12)
            {
                e0 = cv0; ax = a.x; ay = a.y; e1 = cv2; bx = c.x; by = c.y;
            }
            else
            {
                e0 = cv1; ax = b.x; ay = b.y; e1 = cv2; bx = c.x; by = c.y;
            }
            CanvasLinearGradientBrush grad = PortGradientBrush(ds, e0, e1);
            grad.StartPoint = new Vector2(ax, ay);
            grad.EndPoint = new Vector2(bx, by);
            using (CanvasGeometry tri = PortTriangle(ds, a, b, c))
            {
                ds.FillGeometry(tri, grad);
            }
            // Same bisect probe as the flat branch: the census fires before this fill too, so a
            // death in the brush cache and a death after it are otherwise indistinguishable.
            if (uLogged)
            {
                AppLog.Write("Win2D: untext fill-ok (grad) i=" + _portCmdIdx);
            }
        }

        private static float ColorDist(Color x, Color y)
        {
            float dr = x.R - y.R, dg = x.G - y.G, db = x.B - y.B, da = x.A - y.A;
            return dr * dr + dg * dg + db * db + da * da;
        }

        /// <summary>
        /// How many textured triangles have been drawn, for rate-limiting the `texdraw` line.
        /// Strided rather than per-frame: this runs a few thousand times a second in play.
        /// </summary>
        private int _texDraws;

        /// <summary>
        /// Builds the solid-colour triangle the textured path cannot cover.
        ///
        /// The resource creator is not optional: `new CanvasPathBuilder(null)` is E_INVALIDARG on
        /// Win2D.win81 and surfaces as `ArgumentException: Value does not fall within the expected
        /// range` at the constructor, which took down every untextured triangle in the chart - the
        /// note lines, which is most of a frame. `ds` is also the only correct argument, not just
        /// a non-null one: a `CanvasGeometry` is bound to the device that built it, so
        /// `ds.FillGeometry` below can consume it only because both came from the same session.
        /// </summary>
        private static CanvasGeometry PortTriangle(CanvasDrawingSession ds, PrprPort.PortVertex a,
                                                   PrprPort.PortVertex b, PrprPort.PortVertex c)
        {
            using (CanvasPathBuilder pb = new CanvasPathBuilder(ds))
            {
                pb.BeginFigure(a.x, a.y);
                pb.AddLine(b.x, b.y);
                pb.AddLine(c.x, c.y);
                pb.EndFigure(CanvasFigureLoop.Closed);
                return CanvasGeometry.CreatePath(pb);
            }
        }

        /// <summary>
        /// Largest patch one sync step builds, in pixels.
        ///
        /// Not a speed optimisation - the work is the same either way - but a *peak* one. A whole
        /// 1024x1024 atlas is 1,048,576 pixels, and building that as one Color[] (4 MiB) beside the
        /// byte[] copy (4 MiB) and the intermediate patch bitmap (4 MiB) borrows 12 MiB all at once
        /// on a phone whose budget is counted in tens. Banding keeps the very same upload path and
        /// only asks for a slice of that; a delta smaller than the band never notices it is there.
        /// </summary>
        private const uint MaxSyncBandPixels = 262144;

        /// <summary>
        /// Brings the shell's copy of the Rust textures up to date. Called once per frame.
        ///
        /// It used to run once and latch. That is right for chart art - bytes uploaded once and
        /// never touched again - and wrong for the glyph atlas, because `prpr` rasterises a
        /// judgement word's glyph the first time that word is *shown*: the latch is exactly why the
        /// text appearing during play never drew. Polling costs one `count`, one `id_at` and one
        /// `dirty` per texture and copies nothing unless something moved, so a frame on which no new
        /// glyph appeared pays a few dozen native calls and not one byte.
        ///
        /// First sight of an id = full initialisation; every later hit = a delta over the rectangle
        /// Rust reports. Both go through the same <see cref="SyncPortTexture"/>, so a delta that
        /// happens to cover the whole texture *is* the initialisation - there is no second
        /// implementation to keep in step, and the one only the phone runs is never the one that
        /// rots.
        /// </summary>
        /// <summary>Scratch set for <see cref="SweepPortTextures"/>; cleared, never reallocated.</summary>
        private readonly HashSet<uint> _liveIds = new HashSet<uint>();

        /// <summary>Poll counter driving the periodic half of the sweep.</summary>
        private int _sweepTick;

        /// <summary>
        /// Disposes the carriers whose id the registry has released.
        ///
        /// The registry is no longer append-only - `Texture2D::delete` now returns its bytes - so an
        /// id can leave, and this side would otherwise hold a `CanvasRenderTarget` for it until
        /// `ResetPortTextures`. Those are D3D resources, not managed byte arrays: dropping the
        /// dictionary entry alone leaves them to a finalizer that may never run, and the whole
        /// point of freeing the Rust side was to make room on a phone.
        ///
        /// Safe because a released id is never drawn again: `prpr` dropped the handle before the
        /// registry entry went (that is what releasing *means*), so the only thing that can still
        /// mention it is this dictionary.
        /// </summary>
        private void SweepPortTextures(int total)
        {
            if (_portTextures.Count == 0)
            {
                return;
            }
            _liveIds.Clear();
            for (int i = 0; i < total; i++)
            {
                if (_portIds[i] != 0)
                {
                    _liveIds.Add(_portIds[i]);
                }
            }

            // Hopeless entries for ids the registry has since dropped stop meaning
            // anything; without this the gate's subtraction would drift low over charts.
            if (_hopelessPortTextures.Count > 0)
            {
                List<uint> stale = null;
                foreach (uint hid in _hopelessPortTextures.Keys)
                {
                    if (!_liveIds.Contains(hid))
                    {
                        if (stale == null)
                        {
                            stale = new List<uint>();
                        }
                        stale.Add(hid);
                    }
                }
                if (stale != null)
                {
                    for (int i = 0; i < stale.Count; i++)
                    {
                        _hopelessPortTextures.Remove(stale[i]);
                    }
                }
            }

            List<uint> gone = null;
            foreach (uint id in _portTextures.Keys)
            {
                if (!_liveIds.Contains(id))
                {
                    if (gone == null)
                    {
                        gone = new List<uint>();
                    }
                    gone.Add(id);
                }
            }
            if (gone == null)
            {
                return;
            }

            for (int i = 0; i < gone.Count; i++)
            {
                PortTexture pt;
                if (_portTextures.TryGetValue(gone[i], out pt))
                {
                    _portTextures.Remove(gone[i]);
                    if (pt != null)
                    {
                        pt.Dispose();
                    }
                }
            }
            PortTexturesBound = _portTextures.Count;
            _sweepReleased += gone.Count;
            AppLog.Write("Win2D: released " + gone.Count + " port carriers (total "
                + _sweepReleased + ", live=" + total + ")");
        }

        /// <summary>How many carriers the sweep has released this session, for the log line.</summary>
        private int _sweepReleased;

        /// <summary>
        /// How many geometry batches named a texture whose carrier did not exist yet this session.
        ///
        /// The white-block fix is a *skip*, and a skip is invisible by construction - so without
        /// this the fix and a silently broken text path would look identical on the device.
        /// </summary>
        private int _carrierPendingSkipped;

        /// <summary>How many times the device has gone away this session.</summary>
        private int _deviceLost;

        /// <summary>
        /// Bytes of full-texture upload allowed per poll, and what is still left of it.
        ///
        /// Measured throughput on this phone is single-digit MB/s (a 1280x720 illustration
        /// reads back at ~6 MB/s), so one unconstrained full upload is a multi-hundred-ms
        /// hitch sitting inside a 16 ms frame - the menu-scroll and chart-load freezes. The
        /// budget spreads a big texture over polls: covers land over one or two polls, an
        /// illustration over a dozen, and no single poll ever pays for a whole one. Glyph
        /// deltas are exempt (see PollPortTexture): they are small, and text that arrives a
        /// poll late tears visibly while a cover that arrives a poll late does not. 96 KB is
        /// ~10-35 ms at the measured rate: the menu frame is already Rust-bound past 100 ms,
        /// so a bigger slice would only add rarer, taller hitches on top of it, while a
        /// smaller one starves the queue faster than scrolling refills it. The `ms=` on every
        /// upload line says whether it needs to move.
        /// </summary>
        private const long UploadBytesPerPoll = 98304;
        private long _uploadBudgetLeft;
        /// <summary>Whether the self-tuning budget is currently boosted. Logged on change.</summary>
        private bool _uploadBoosted;
        /// <summary>
        /// Full uploads deferred for lack of budget, oldest first, with the row they resume
        /// at. Drained twice per poll - half the fresh budget at the head (so the backlog
        /// always moves, fonts included) and the remainder after the poll loop's new textures -
        /// so a big texture can neither starve nor be starved by small ones arriving behind it.
        /// </summary>
        private readonly Queue<uint> _uploadQueue = new Queue<uint>();
        private readonly Dictionary<uint, uint> _uploadResume = new Dictionary<uint, uint>();

        /// <summary>
        /// Textures whose carrier cannot be built (zero extent, oversize, device OOM), with
        /// the poll index at which they may be retried. Retried periodically, not dropped:
        /// device memory frees up, and a dropped id would never come back. Crucially, the
        /// loading gate does not count these - a texture that can never complete would hold
        /// it through its whole timeout on every load - and the create path does not hammer
        /// them every poll (each attempt costs broker calls plus a failing `WriteSync`).
        /// Pruned against the registry on every sweep; cleared on reset.
        /// </summary>
        private readonly Dictionary<uint, int> _hopelessPortTextures = new Dictionary<uint, int>();
        /// <summary>Draw polls so far; drives the hopeless retry stamps above.</summary>
        private int _pollIndex;

        private void EnsurePortTextures(CanvasDrawingSession ds)
        {
            _pollIndex++;
            if (_portDirtyUnsupported)
            {
                try
                {
                    LoadPortTexturesOnce(ds);
                }
                catch (Exception ex)
                {
                    // Reached only if the DLL itself is unusable - the fallback calls nothing but
                    // exports that already existed - and `LoadPortTexturesOnce` latches before it
                    // reads anything, so this reports the failure once instead of throwing on every
                    // frame after it.
                    AppLog.WriteSync("Win2D: port texture fallback failed", ex);
                }
                return;
            }

            // Every frame while the registry is still growing, every third frame once it is stable.
            //
            // Gating it unconditionally was wrong, and it showed immediately: during a load the first
            // frames are seconds apart, so a texture registered on frame N had no carrier for up to
            // three frames - and a geometry batch whose texture has no carrier falls back to a *flat
            // fill*, which is a white block. That is the "white blocks at startup, text appears a
            // moment later" report. The saving is only worth having once the set has stopped
            // changing, which is the case this was written for.
            int live = (int)PrprPort.TextureCount();
            bool changed = live != _texPollTotal;
            if (changed)
            {
                _texPollTotal = live;
                _texPollFrame = 0;
            }
            else
            {
                _texPollFrame++;
                if ((_texPollFrame % 3) != 0)
                {
                    return;
                }
            }
            // A delete is only *guaranteed* to move the count when nothing was added alongside it,
            // and it is not worth a per-frame pass over every key to be sure. Every change plus a
            // slow periodic catch-up bounds how long a freed texture can keep a render target.
            _sweepTick++;
            bool sweep = changed || (_sweepTick % 120 == 0);

            try
            {
                // Fresh budget every poll, split old/new half and half. New textures going
                // first alone starved everything registered earlier for as long as the registry
                // kept growing - which is the whole boot, so fonts and icons stayed blank until
                // it ended (a black screen that looked hung rather than loading). Oldest-first
                // alone would stall covers just scrolled into view behind an illustration's
                // remainder. Half and half keeps both moving: the latest demand (a cover just
                // scrolled into view, sprites for a chart about to start) and the backlog behind
                // it, including font atlases the whole UI waits on.
                //
                // Self-tuning size: a deep backlog (chart load registering megabytes at once)
                // gets 1 MB a poll instead of 96 KB, because at 96 KB a 25 MB illustration
                // set costs ~90 s of polls - longer than the loading gate's whole timeout, so
                // the gate could never pass with real work outstanding and notes popped in
                // mid-song. A deep backlog in gameplay is itself the emergency, so spending
                // more per poll there is also right; the steady state (shallow queue) keeps the
                // small budget and its small hitches. Logged on transitions only.
                long pollBudget = _uploadQueue.Count > 8 ? UploadBytesPerPoll * 32 / 3 : UploadBytesPerPoll;
                if ((pollBudget > UploadBytesPerPoll) != _uploadBoosted)
                {
                    _uploadBoosted = pollBudget > UploadBytesPerPoll;
                    AppLog.Write("Win2D: upload budget " + (_uploadBoosted ? "boosted" : "normal")
                        + " (queue=" + _uploadQueue.Count + ")");
                }
                _uploadBudgetLeft = pollBudget;
                long queueShare = pollBudget / 2;
                DrainUploadQueue(ds, ref queueShare);
                _uploadBudgetLeft -= (pollBudget / 2 - queueShare);

                int total = live;
                if (total > 0)
                {
                    if (_portIds == null || _portIds.Length < total)
                    {
                        // Grows with the registry and never per frame.
                        _portIds = new uint[total];
                    }
                    PrprPort.CopyTextureIds(_portIds, total);

                    if (sweep)
                    {
                        SweepPortTextures(total);
                    }

                    for (int i = 0; i < total; i++)
                    {
                        uint id = _portIds[i];
                        if (id != 0)
                        {
                            PollPortTexture(ds, id);
                        }
                    }

                    // Deferred full uploads take whatever budget the new textures above left.
                    // Same poll, same budget, opposite end: what is still queued afterwards
                    // simply waits for the next poll, which resets the budget above.
                    DrainUploadQueue(ds, ref _uploadBudgetLeft);
                }
                else if (sweep && _portTextures.Count > 0)
                {
                    // The registry emptied: nothing below runs, so the loop would never reach it.
                    SweepPortTextures(0);
                }
            }
            catch (Exception ex)
            {
                if (!MissingExport(ex))
                {
                    AppLog.WriteSync("Win2D: port texture poll failed", ex);
                    return;
                }

                // The two incremental exports are still being added to the DLL. Swallowing that once
                // and switching to the full read keeps the app running with today's behaviour; not
                // latching it would instead throw once per texture per frame, and an exception is
                // far more expensive than the poll it replaces.
                AppLog.WriteSync("Win2D: incremental texture sync is not exported yet ("
                    + ex.GetType().Name + "); using the one-shot full read");
                DisposePortTextures();
                _portTexturesReady = false;
                _portDirtyUnsupported = true;
                return;
            }

            PortTexturesBound = _portTextures.Count;
        }

        /// <summary>
        /// True for "the DLL does not export that symbol", which is a scheduling problem rather than
        /// a bug: the Rust side is landing those two exports, and until it does the shell has to
        /// degrade instead of failing. Everything else is a real failure and is logged as one.
        ///
        /// Matched on the type's *name* because `EntryPointNotFoundException` is not in the
        /// WindowsPhoneApp 8.1 reference set - naming it does not compile, even though it is what
        /// the CLR throws for a P/Invoke with no matching export. `DllNotFoundException` is in the
        /// set but goes through the same loop so that one mechanism covers both, and the walk over
        /// `InnerException` costs nothing on the only path where it runs.
        /// </summary>
        /// <summary>
        /// True when the failure means "this device is gone, build another one".
        ///
        /// Both strings are from the phone's own log, not from a header:
        ///
        /// * `0x8899000C presentation error` - `0x8899000C` is `D2DERR_RECREATE_TARGET`. The HRESULT
        ///   is in the *message* because `AppLog.Describe` composes `Type: Message` and never reads
        ///   `Exception.HResult`.
        /// * `Objects used together must be created from the same factory instance` - the
        ///   consequence, one line later. Carriers built on the dead device were still in
        ///   `_portTextures`, so a bitmap from one factory was drawn into a session from another.
        ///   That is what escaped `OnDraw`, which had no try at all, and an exception out of a Win2D
        ///   `Draw` handler is unhandled: the process died with no managed message beside it.
        ///
        /// Matched on text rather than on `Exception.HResult` for the same reason
        /// <see cref="MissingExport"/> matches on a type *name*: this reference set is a subset, and
        /// a member missing from it fails to compile rather than failing at run time. Three specific
        /// strings rather than "any COMException" because the recovery throws every carrier away -
        /// a reset triggered by an unrelated COMException would cost a full re-read of every texture
        /// for nothing, and would be indistinguishable in the log from the real thing.
        /// </summary>
        private static bool IsDeviceLost(Exception ex)
        {
            for (Exception e = ex; e != null; e = e.InnerException)
            {
                if (e.GetType().Name != "COMException")
                {
                    continue;
                }
                string m = e.Message ?? "";
                if (m.IndexOf("8899000C", StringComparison.OrdinalIgnoreCase) >= 0
                    || m.IndexOf("presentation error", StringComparison.OrdinalIgnoreCase) >= 0
                    || m.IndexOf("same factory instance", StringComparison.OrdinalIgnoreCase) >= 0)
                {
                    return true;
                }
            }
            return false;
        }

        private static bool MissingExport(Exception ex)
        {
            for (Exception e = ex; e != null; e = e.InnerException)
            {
                string name = e.GetType().Name;
                if (name == "EntryPointNotFoundException" || name == "DllNotFoundException")
                {
                    return true;
                }
            }
            return false;
        }

        /// <summary>
        /// Whether the backlog-push export exists on this DLL. Latched off on the first
        /// missing-export throw: without it every draw would pay an exception for a call
        /// whose absence only means the loading gate falls back to its timeout.
        /// </summary>
        private bool _uploadPushSupported = true;

        /// <summary>Rate gate for the stuck-upload line below.</summary>
        private int _stuckLogTick = int.MinValue / 2;

        /// <summary>
        /// Tells the engine how many textures still need their first upload: queued
        /// resumes plus registered ids with no carrier yet, minus hopeless ones (zero
        /// extent, oversize, device OOM - they can never complete, so counting them would
        /// hold the loading gate through its timeout on every load). The loading scene
        /// waits on this before transitioning, so notes never pop in mid-song.
        /// Best-effort: any failure only delays the gate to its timeout, never the draw.
        /// </summary>
        private void ReportUploadBacklog()
        {
            if (!_uploadPushSupported)
            {
                return;
            }
            try
            {
                uint pending = (uint)_uploadQueue.Count + (uint)_uploadResume.Count;
                int missing = (int)PrprPort.TextureCount() - _portTextures.Count - _hopelessPortTextures.Count;
                // Hopeless entries for ids the registry has since dropped are pruned on the
                // next sweep; until then the clamp keeps the count sane in the other direction.
                if (missing < 0)
                {
                    missing = 0;
                }
                if (missing > 0)
                {
                    pending += (uint)missing;
                }
                PrprPort.ReportUploadPending(pending);
                // Names the stuck ones, once a second while anything is stuck: the count
                // alone cannot say whether the gate is waiting on a 5 MB illustration that
                // is nearly done or on an id that will never complete - and those need
                // opposite fixes. Capped at a glance (three ids); the queue beyond that is
                // backlog, not suspects.
                if (pending > 0 && unchecked(Environment.TickCount - _stuckLogTick) >= 1000)
                {
                    _stuckLogTick = Environment.TickCount;
                    System.Text.StringBuilder sb = new System.Text.StringBuilder("Win2D: upload stuck n=");
                    sb.Append(pending).Append(" queue=");
                    int shown = 0;
                    foreach (uint qid in _uploadQueue)
                    {
                        if (shown >= 3) { break; }
                        if (shown > 0) { sb.Append(','); }
                        uint row = 0;
                        _uploadResume.TryGetValue(qid, out row);
                        sb.Append(qid).Append('@').Append(row);
                        shown++;
                    }
                    sb.Append(" missing=").Append(missing);
                    AppLog.Write(sb.ToString());
                }
            }
            catch (Exception ex)
            {
                if (MissingExport(ex))
                {
                    _uploadPushSupported = false;
                }
            }
        }

        /// <summary>
        /// One id's contribution to this frame: nothing at all, a delta, or a first full read.
        /// </summary>
        /// <summary>
        /// Resumes full-texture uploads deferred for lack of budget, oldest first, against
        /// the given allowance (not the field: the head-of-poll call passes half the fresh
        /// budget so the backlog always moves, the tail call passes whatever the poll loop's
        /// new textures left over).
        ///
        /// Bounded per call: at most the entries present on entry are attempted, so a texture
        /// that never fits cannot spin this into an infinite loop.
        /// </summary>
        private void DrainUploadQueue(CanvasDrawingSession ds, ref long budgetLeft)
        {
            int entries = _uploadQueue.Count;
            for (int k = 0; k < entries && budgetLeft > 0; k++)
            {
                uint id = _uploadQueue.Dequeue();
                uint resume;
                if (!_uploadResume.TryGetValue(id, out resume))
                {
                    continue;
                }
                PortTexture pt;
                if (!_portTextures.TryGetValue(id, out pt) || pt == null || pt.Target == null)
                {
                    // Swept (or never built) since deferral: drop the resume, do not re-queue.
                    _uploadResume.Remove(id);
                    continue;
                }
                uint rw, rh;
                if (!PrprPort.TextureSize(id, out rw, out rh) || rw != pt.PixelWidth || rh != pt.PixelHeight)
                {
                    // The registry's extent moved under the carrier (glyph-cache rebuild). The
                    // poll loop below re-creates it through the budgeted full path, so dropping
                    // here hands over rather than losing the upload.
                    pt.Dispose();
                    _portTextures.Remove(id);
                    _uploadResume.Remove(id);
                    continue;
                }
                uint w = pt.PixelWidth;
                uint h = pt.PixelHeight;
                if (resume >= h)
                {
                    _uploadResume.Remove(id);
                    continue;
                }
                uint next;
                SyncPortTexture(ds, id, pt, 0, resume, w, h - resume, null, ref budgetLeft, out next, true);
                if (next < h)
                {
                    _uploadResume[id] = next;
                    _uploadQueue.Enqueue(id);
                }
                else
                {
                    _uploadResume.Remove(id);
                }
            }
        }

        /// <summary>
        /// A full upload through the per-poll budget: what fits goes now, the rest is queued
        /// for the head of the next polls. Deltas never come here (see PollPortTexture) - a
        /// full upload is the only shape big enough to hitch a frame.
        /// </summary>
        private void UploadFull(CanvasDrawingSession ds, uint id, PortTexture pt, uint w, uint h)
        {
            if (w == 0 || h == 0)
            {
                _uploadResume.Remove(id);
                _hopelessPortTextures[id] = _pollIndex + 120;
                return;
            }
            if (_uploadBudgetLeft <= 0)
            {
                // No budget left this poll: queue the resume silently instead of running a
                // zero-byte SyncPortTexture. That call uploads nothing (the band loop breaks
                // before the first read) but still costs a drawing session and a blocking log
                // line - and with animated giants churning, every poll ended with a tail of
                // these, hundreds of log lines a second that starved the real uploads of the
                // UI thread they both share. Queued here, drained at the next poll's head.
                if (!_uploadResume.ContainsKey(id))
                {
                    _uploadQueue.Enqueue(id);
                }
                _uploadResume[id] = 0;
                return;
            }
            uint next;
            SyncPortTexture(ds, id, pt, 0, 0, w, h, null, ref _uploadBudgetLeft, out next, false);
            if (next < h)
            {
                if (!_uploadResume.ContainsKey(id))
                {
                    _uploadQueue.Enqueue(id);
                }
                _uploadResume[id] = next;
            }
            else
            {
                _uploadResume.Remove(id);
            }
        }

        private void PollPortTexture(CanvasDrawingSession ds, uint id)
        {
            // Asked first, on every path, and for its side effect rather than its answer: an id the
            // shell has never built must still consume its pending rectangle, because Rust reports a
            // fresh registration as fully dirty and leaving the flag standing would ask for the same
            // full read again on the very next frame.
            PrprPort.TexRect rect;
            bool changed = PrprPort.TextureDirty(id, out rect);

            PortTexture pt;
            if (!_portTextures.TryGetValue(id, out pt) || pt == null || pt.Target == null)
            {
                int retryAt;
                if (_hopelessPortTextures.TryGetValue(id, out retryAt) && _pollIndex < retryAt)
                {
                    // Carrier creation failed recently for this id; the device is not going
                    // to grow memory this poll, so skip the broker calls and the failing log.
                    return;
                }
                uint w, h;
                if (!PrprPort.TextureSize(id, out w, out h))
                {
                    // The registry does not hold this id (a freed slot the count still
                    // includes). Parked like any other hopeless id: without this, dead ids
                    // kept the loading gate's missing count above zero forever and every load
                    // waited out the full timeout while the real backlog had long drained.
                    // Shorter stamp than a failed creation: a size miss can be transient
                    // (an id enumerated mid-teardown), so it re-probes sooner.
                    _hopelessPortTextures[id] = _pollIndex + 30;
                    return;
                }
                pt = CreatePortTexture(ds, id, w, h);
                if (pt == null)
                {
                    // Zero extent, oversize, or device OOM - persistent for this id, retried
                    // periodically, and invisible to the loading gate (see the field's note).
                    _hopelessPortTextures[id] = _pollIndex + 120;
                    return;
                }
                _hopelessPortTextures.Remove(id);
                _portTextures[id] = pt;

                // The whole texture, whatever the dirty read said. For a fresh registration the two
                // agree by contract; where they do not - the flag was drained somewhere else first -
                // only a full read leaves a carrier that is not half blank, and either way this runs
                // once per texture.
                UploadFull(ds, id, pt, w, h);
                return;
            }

            if (!changed || rect.W == 0 || rect.H == 0)
            {
                return;
            }

            if ((ulong)rect.X + rect.W > pt.PixelWidth || (ulong)rect.Y + rect.H > pt.PixelHeight)
            {
                // The registry's extent for this id no longer fits the carrier. `prpr` rebuilds its
                // glyph cache larger when glyph-brush runs out of room (`BrushError::TextureTooSmall`),
                // so this is a real path rather than a defensive one. Patching is impossible when
                // the geometry itself changed, so the carrier is replaced and re-read whole - the one
                // case where a large rebuild during play cannot be avoided, and precisely the rebuild
                // the dirty rectangle exists to prevent everywhere else. The log line below is what
                // will say whether it ever happens.
                uint w, h;
                pt.Dispose();
                _portTextures.Remove(id);
                if (!PrprPort.TextureSize(id, out w, out h))
                {
                    _hopelessPortTextures[id] = _pollIndex + 30;
                    return;
                }
                pt = CreatePortTexture(ds, id, w, h);
                if (pt == null)
                {
                    _hopelessPortTextures[id] = _pollIndex + 120;
                    return;
                }
                _hopelessPortTextures.Remove(id);
                _portTextures[id] = pt;
                UploadFull(ds, id, pt, w, h);
                return;
            }

            // The incremental case: a rectangle that is usually a few hundred pixels of glyph.
            // Deltas bypass the budget (unbounded): they are small, and holding one back tears
            // text while holding back a cover only delays it by a poll.
            long unbounded = long.MaxValue;
            uint done;
            SyncPortTexture(ds, id, pt, rect.X, rect.Y, rect.W, rect.H, null, ref unbounded, out done, false);
        }

        /// <summary>
        /// Builds the empty carrier a texture will be patched into.
        ///
        /// 96 dpi is pinned rather than inherited. The drain hands over *pixel* coordinates, and the
        /// uv maths multiplies by <see cref="PortTexture.PixelWidth"/>; Win2D measures a source
        /// rectangle in the image's dips, so the two readings only coincide when one dip is one
        /// pixel. A carrier built at the device's dpi would scale every glyph quad by the dpi ratio
        /// and nothing would fail loudly enough to say why.
        /// </summary>
        private PortTexture CreatePortTexture(CanvasDrawingSession ds, uint id, uint w, uint h)
        {
            if (w == 0 || h == 0)
            {
                return null;
            }

            try
            {
                long c0 = System.Diagnostics.Stopwatch.GetTimestamp();
                CanvasRenderTarget target = new CanvasRenderTarget(ds.Device, w, h, 96f);
                _carrierTicks += System.Diagnostics.Stopwatch.GetTimestamp() - c0;
                _carrierCount++;
                if (target.SizeInPixels.Width != w || target.SizeInPixels.Height != h)
                {
                    // Logged rather than assumed: there is no device to check this on while the code
                    // is being written, and a carrier whose pixel size is not the atlas's is the one
                    // mistake that shows up only as text that is subtly the wrong size.
                    AppLog.WriteSync("Win2D: port texture " + id + " carrier is "
                        + target.SizeInPixels.Width + "x" + target.SizeInPixels.Height
                        + ", atlas is " + w + "x" + h);
                }

                PortTexture pt = new PortTexture();
                pt.Target = target;
                pt.PixelWidth = w;
                pt.PixelHeight = h;
                return pt;
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("Win2D: port texture " + id + " carrier " + w + "x" + h + " failed", ex);
                return null;
            }
        }

        /// <summary>
        /// Copies one rectangle of a texture from the Rust registry into its carrier - in bands - and
        /// records what that cost.
        ///
        /// This is the only upload path. An initial full read hands it the whole texture, a delta
        /// hands it the rectangle Rust named, and banding simply means "one call covering the whole
        /// texture" is implemented as "several calls that add up to it". One path is the point: two
        /// would drift, and the drifting one is always the version only the phone runs.
        ///
        /// <paramref name="preload"/> passes in bytes the caller already holds (the legacy full read)
        /// so that path never copies the same pixels twice.
        ///
        /// The log line is the entire evidence for whether the dirty-rectangle design earns its keep.
        /// A delta that stays small is the design working; a delta that comes back full-size during
        /// play means Rust is unioning more than one frame's uploads; a `ms` that climbs means the
        /// band budget needs revisiting. Nothing else in the system can answer those questions.
        /// </summary>
        /// <paramref name="budgetLeft"/> caps what this call may upload; <paramref name="nextRow"/>
        /// comes back as the absolute row the next call must resume at (<c>y + h</c> when the
        /// rectangle completed). <paramref name="resumed"/> marks a continuation: it only moves
        /// the log labels, so a resumed chunk does not read as a new upload.
        /// </summary>
        private void SyncPortTexture(CanvasDrawingSession ds, uint id, PortTexture pt,
                                     uint x, uint y, uint w, uint h, byte[] preload,
                                     ref long budgetLeft, out uint nextRow, bool resumed)
        {
            nextRow = y;
            if (w == 0 || h == 0 || pt.Target == null)
            {
                // Nothing to upload: report the rectangle complete rather than resume it
                // forever. The zero-size guard in UploadFull keeps fresh registrations out;
                // this covers the rebuild path racing a freed carrier the same way.
                nextRow = y + h;
                return;
            }

            bool full = x == 0 && y == 0 && w == pt.PixelWidth && h == pt.PixelHeight;
            long bytes = 0;
            // The content probe runs on the first band of a full upload only: it is the pixel data
            // itself that decides whether "white" means "the texture really is white" or "the
            // texture is fine and something else is hiding it", and re-probing every delta would
            // pay for an answer that cannot change.
            // Probed on every upload, full or delta - and a delta is where this question
            // actually lives.
            //
            // The band below is patched in with `CanvasBlend.Copy`, which makes the
            // destination region *become* the source, transparent pixels included. So an
            // empty delta is not "adds nothing": it erases the rectangle it names. Every
            // byte of text reaches a glyph atlas through exactly that path - a full read
            // happens once at registration, before most glyphs exist - which means the
            // content of a delta is the one reading the probe could get wrong without
            // anything else in the frame changing.
            //
            // Restricted to the first band so an upload still costs one line: a full read
            // is four bands of the same atlas, and the answer does not vary within it.
            bool firstBand = true;
            System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
            try
            {
                using (CanvasDrawingSession target = pt.Target.CreateDrawingSession())
                {
                    uint row = 0;
                    while (row < h)
                    {
                        uint band = preload != null ? h : BandRows(w, h - row);
                        if (preload == null)
                        {
                            // Cap the band by what the budget still holds. A band bigger
                            // than the whole budget (a 1024-wide atlas band is 1 MB against
                            // a 256 KB budget) is cut down instead of skipped: skipping it
                            // defers at row 0 on every poll and the texture stays blank
                            // forever, which is exactly how covers and glyphs went missing.
                            // Deferring with an empty budget is safe - the budget refreshes
                            // every poll and the queue is first-in first-out, so a texture
                            // that finds nothing left this poll is head of the line on the
                            // next one. Bands are written whole, so rows already painted
                            // stay valid and the continuation neither tears nor repeats.
                            long fit = budgetLeft / ((long)w * 4L);
                            if (fit < 1)
                            {
                                break;
                            }
                            if ((long)band > fit)
                            {
                                band = (uint)fit;
                            }
                        }
                        byte[] rgba = preload;
                        if (rgba == null)
                        {
                            PrprPort.TexRect bandRect;
                            bandRect.X = x;
                            bandRect.Y = y + row;
                            bandRect.W = w;
                            bandRect.H = band;
                            rgba = PrprPort.ReadTextureRect(id, bandRect);
                            if (rgba == null)
                            {
                                // Already logged by the reader. Returning rather than carrying on
                                // leaves the rest of the rectangle stale but intact, which reads as
                                // old glyphs instead of half a word.
                                nextRow = y + row;
                                return;
                            }
                        }
                        bytes += rgba.Length;
                        budgetLeft -= rgba.Length;
                        if (firstBand && !resumed)
                        {
                            firstBand = false;
                            AppLog.Write("Win2D: port texture " + id + " band0 "
                                + w + "x" + band + " @" + x + "," + y
                        + " " + BandSummary(rgba));
                        }
                        PaintPortBand(ds.Device, target, x, y + row, w, band, rgba);
                        row += band;
                    }
                    nextRow = y + row;
                }
            }
            catch (Exception ex)
            {
                if (MissingExport(ex))
                {
                    // Re-thrown so the poll above turns it into the legacy path *once*, instead of
                    // here once per frame.
                    throw;
                }
                if (IsDeviceLost(ex))
                {
                    // Every carrier in here was built on a device that no longer exists. Leaving
                    // them in the dictionary is worse than having none: the next call draws one of
                    // them into a session created on the replacement device, which is the
                    // cross-factory COMException - and *that* one is thrown outside any try on this
                    // path. Dropping them makes the next poll build fresh ones from whatever device
                    // is current; the registry's pixels are untouched, so the recovery costs one
                    // full re-read per texture and nothing else.
                    int carriers = _portTextures.Count;
                    DisposePortTextures();
                    _deviceLost++;
                    AppLog.WriteSync("Win2D: port texture " + id + " device lost (#" + _deviceLost
                        + "), dropped " + carriers + " carriers", ex);
                    return;
                }
                AppLog.WriteSync("Win2D: port texture " + id + " sync failed", ex);
            }
            finally
            {
                clock.Stop();
                // Zero-progress resumes poll every frame while a big texture waits for budget;
                // each costs a blocking file chain for no signal, and they were the bulk of the
                // log during play. Completions, real chunks, inits, deltas, errors and the band0
                // probe above all keep their lines - only "nothing moved" goes quiet.
                if (bytes != 0 || !resumed)
                {
                    AppLog.Write("Win2D: port texture " + id
                        + (full ? " init" : (resumed ? "resume" : "delta"))
                        + " rect=(" + x + "," + y + "," + w + "," + h + ")"
                        + " bytes=" + bytes
                        + " ms=" + clock.ElapsedMilliseconds);
                }
            }
        }

        /// <summary>
        /// A four-number read of what a band of a texture actually contains.
        ///
        /// Two textures that both report "uploaded 78876 bytes" look identical in the log yet one
        /// can be a crisp sprite and the other a solid white rectangle, which is the difference
        /// this needs to settle. `first` catches a source that was never written (all-zero or
        /// all-0xFF at the very start); the percentages catch the case the first pixel cannot -
        /// a sprite that is genuinely uniform, and a glyph atlas that is all background.
        ///
        /// `white` counts pixels whose *colour* is 255,255,255 whatever the alpha says, because
        /// that is the shape "renders white" actually takes: an RGB channel that went to full
        /// white while alpha still varies draws as an opaque white blob with the right silhouette.
        ///
        /// Sampled with a stride across the whole band rather than over its first 4096 bytes. The
        /// head-only version reported `zero=100%` for eight 256x256 note atlases - four rows of a
        /// sprite whose content starts below a padded top - so it read as proof of empty textures
        /// while describing only the padding. Coverage is the point of this call; the sample count
        /// is already bounded, and a stride keeps it that way.
        /// </summary>
        private static string BandSummary(byte[] px)
        {
            int pixels = px.Length >> 2;
            if (pixels < 1)
            {
                return "band too short (" + px.Length + " bytes)";
            }

            int step = pixels / 4096;
            if (step < 1) step = 1;

            int taken = 0, ff = 0, z = 0, white = 0;
            for (int p = 0; p < pixels; p += step)
            {
                int i = p << 2;
                byte r = px[i], g = px[i + 1], b = px[i + 2], a = px[i + 3];
                taken++;
                if (r == 0xFF && g == 0xFF && b == 0xFF) white++;
                if (r == 0xFF) ff++; else if (r == 0) z++;
                if (g == 0xFF) ff++; else if (g == 0) z++;
                if (b == 0xFF) ff++; else if (b == 0) z++;
                if (a == 0xFF) ff++; else if (a == 0) z++;
            }

            int sampled = taken * 4;
            return "first=" + px[0] + "," + px[1] + "," + px[2] + "," + px[3]
                + " ff=" + (ff * 100 / sampled) + "% zero=" + (z * 100 / sampled) + "%"
                + " white=" + (white * 100 / taken) + "%";
        }

        /// <summary>How many rows a band may hold so it stays inside <see cref="MaxSyncBandPixels"/>.</summary>
        private static uint BandRows(uint width, uint rowsWanted)
        {
            uint rows = width == 0 ? rowsWanted : MaxSyncBandPixels / width;
            if (rows == 0)
            {
                // A texture wider than the band budget still has to be read, one row at a time.
                rows = 1;
            }
            return rows > rowsWanted ? rowsWanted : rows;
        }

        /// <summary>
        /// Blends one band's pixels into the carrier at its atlas coordinates.
        ///
        /// The source rectangle is the patch's own <c>Bounds</c> rather than its pixel size: source
        /// rectangles are measured in the *image's* dips, and the patch is an intermediate whose dpi
        /// the caller does not control. The destination is measured in the carrier's dips, and the
        /// carrier is pinned to 96 dpi - so the atlas pixel coordinates the drain and the uv maths
        /// produce are dips there too, and the two halves agree without either side converting.
        /// Nearest-neighbour rather than linear because this is a byte-for-byte copy at 1:1 and any
        /// filtering would invent pixels the Rust side never wrote.
        ///
        /// CreateFromColors rather than CreateFromBytes: Win2D's WP8.1 fork only exposes
        /// CreateFromBytes with a DirectXPixelFormat argument, and that enum lives in
        /// Windows.Graphics.DirectX, which does not exist on WP8.1 (no winmd declares it).
        /// </summary>
        private static void PaintPortBand(ICanvasResourceCreator dev, CanvasDrawingSession target,
                                          uint x, uint y, uint w, uint h, byte[] rgba)
        {
            int pixelCount = (int)((long)w * h);
            Color[] pixels = new Color[pixelCount];
            for (int p = 0; p < pixelCount; p++)
            {
                int o = p * 4;
                if (o + 3 >= rgba.Length)
                {
                    break;
                }

                // Premultiply, because that is the format CreateFromColors stores: it hands the
                // bytes straight to a premultiplied bitmap and D2D converts nothing. Rust sends
                // straight alpha - a white glyph is 255,255,255,a - so feeding it in untouched
                // wrote an impossible premultiplied pixel: RGB at full strength underneath a
                // partial alpha. Compositing that gives ~white however thin the coverage, so
                // every half-covered pixel became opaque and each glyph on the loading screen
                // rendered as a solid white block. It is the same defect as "some images render
                // white", and it only ever touched pixels whose alpha is neither 0 nor 255.
                byte a = rgba[o + 3];
                pixels[p] = Color.FromArgb(a,
                    (byte)((rgba[o + 0] * a + 127) / 255),
                    (byte)((rgba[o + 1] * a + 127) / 255),
                    (byte)((rgba[o + 2] * a + 127) / 255));
            }

            using (CanvasBitmap patch = CanvasBitmap.CreateFromColors(dev, pixels, (int)w, (int)h))
            {
                // Replace rather than composite. This is an upload, not a draw: source-over would
                // blend the band into whatever is underneath, and bands are written whole - the
                // glyph atlas re-emits its top rows every time it grows - so every pixel the new
                // band leaves transparent would keep the stale glyph that was there before it.
                //
                // Copy is the blend that means "the destination region becomes exactly this
                // source", transparent pixels included, and it is the only way to do it here: this
                // Win2D fork exposes only Clear(Color)/Clear(Vector4), with no rectangle overload,
                // so clearing the band first would wipe the whole carrier - atlas and every port
                // texture on it. The session is created per upload and its blend defaults to
                // SourceOver, which is restored below.
                target.Blend = CanvasBlend.Copy;
                target.DrawImage(patch,
                    new Rect(x, y, w, h),
                    patch.Bounds,
                    1f,
                    CanvasImageInterpolation.NearestNeighbor);
                target.Blend = CanvasBlend.SourceOver;
            }
        }

        /// <summary>
        /// The one-shot full read, kept for the single case where the DLL does not export the
        /// incremental pair yet.
        ///
        /// Every reason it is no longer the main path is in <see cref="EnsurePortTextures"/>: it
        /// copies the whole registry into managed memory (24 MiB on the 190 chart) and it can never
        /// see a byte that arrives afterwards, which is the whole of where text comes from. It is
        /// still the right answer when there is nothing to poll with, and it runs at most once -
        /// <see cref="_portTexturesReady"/> exists so that the fallback cannot degenerate into a
        /// per-frame full copy, which would be far worse than the bug it replaces.
        ///
        /// It reuses the same carrier and the same <see cref="SyncPortTexture"/> as the polling path,
        /// handing the bytes it already read in as <c>preload</c> so they are not copied a second time.
        /// </summary>
        private void LoadPortTexturesOnce(CanvasDrawingSession ds)
        {
            if (_portTexturesReady)
            {
                return;
            }
            _portTexturesReady = true;

            List<PrprPort.TextureData> textures = PrprPort.ReadTextures();
            for (int i = 0; i < textures.Count; i++)
            {
                PrprPort.TextureData t = textures[i];
                if (t.Width == 0 || t.Height == 0 || t.Rgba == null)
                {
                    continue;
                }
                PortTexture pt = CreatePortTexture(ds, t.Id, t.Width, t.Height);
                if (pt == null)
                {
                    continue;
                }
                _portTextures[t.Id] = pt;
                long onceUnbounded = long.MaxValue;
                uint onceDone;
                SyncPortTexture(ds, t.Id, pt, 0, 0, t.Width, t.Height, t.Rgba, ref onceUnbounded, out onceDone, false);
            }
            PortTexturesBound = _portTextures.Count;
        }

        /// <summary>Packs 0..1 floats into a Win2D colour, clamping out-of-range input.</summary>
        private static Color FloatColor(float r, float g, float b, float a)
        {
            return Color.FromArgb(Channel(a), Channel(r), Channel(g), Channel(b));
        }

        /// <summary>Reinterprets a raw bit pattern as a float.</summary>
        ///
        /// The port's `Line` and `Texture` records have no free `f` slot left for their missing
        /// alpha, so it travels in an arena field the kind does not otherwise use - as `f32` bits,
        /// because those fields are `u32`. An explicit-layout union rather than
        /// `BitConverter.ToSingle(GetBytes(..))`: this runs per line per frame and that overload
        /// allocates an array every call.
        [StructLayout(LayoutKind.Explicit)]
        private struct FloatBits
        {
            [FieldOffset(0)] public uint Bits;
            [FieldOffset(0)] public float Value;
        }

        private static float AsFloat(uint bits)
        {
            FloatBits f = new FloatBits();
            f.Bits = bits;
            return f.Value;
        }

        // The previous textured triangle's destination and source rectangles, for folding the two
        // triangles of one quad into a single draw. Reset per geometry command - see
        // `DrawPortGeometry` - because across commands a repeated rectangle is legitimate.
        private bool _hasLastQuad;
        private Rect _lastQuad;
        private Rect _lastSrc;

        // What the clip layer in force is clipping to, empty when there is none. `texdraw`
        // proves a glyph was drawn on screen out of a carrier whose content is there; it says
        // nothing about whether a layer was covering it. The ending page is the only scene in
        // active use that opens one, so without this the two readings "the text is drawn
        // wrongly" and "the text is drawn correctly and then cut away" are the same line.
        private string _clipRectTag = "";
        private string _clipSig = "";
        private int _clipLogTick = int.MinValue / 2;
        private int _clipLogCount;
        private int _rectLogTick = int.MinValue / 2;
        // Last destination logged by each census, as floats so the hot path can compare
        // without building a string, plus its budget for the current second.
        private float _lastTdX = float.MinValue, _lastTdY, _lastTdW, _lastTdH;
        private float _lastUnX = float.MinValue, _lastUnY, _lastUnW, _lastUnH;
        private int _tdLogTick = int.MinValue / 2, _tdLogBudget;
        private int _unLogTick = int.MinValue / 2, _unLogBudget;
        // Index into the current frame's command list of the command being replayed, kept in a
        // field because `DrawPortTexture`/`DrawPortGeometry` are called from the replay loop but
        // declared far from it. The process dies inside the loop, so the last line that survives
        // is the census line of the faulting command; without this number that line says what
        // shape was being drawn but not whether it was command 14 of 57 or the very last one,
        // and those two cases point at completely different code.
        private int _portCmdIdx = -1;

        // Scratch for the six vertices of one quad, reused rather than allocated: `TryDrawPortQuad`
        // runs once per sprite per frame, and a fresh array there would be the same per-frame garbage
        // the drain buffers were changed to avoid.
        private readonly PrprPort.PortVertex[] _quadVerts = new PrprPort.PortVertex[6];

        /// <summary>Frame counter for the texture-poll gate - see `EnsurePortTextures`.</summary>
        private int _texPollFrame;

        /// <summary>Registry size at the last sweep; a change forces a sweep on the next frame.</summary>
        private int _texPollTotal = -1;

        // ------------------------------------------------------------------
        // Small helpers
        // ------------------------------------------------------------------

        private static CanvasBitmap BitmapOf(ChartTexture tex)
        {
            return tex == null ? null : tex.Tag as CanvasBitmap;
        }

        private static Rect CenteredRect(double w, double h)
        {
            return new Rect(-w / 2.0, -h / 2.0, w, h);
        }

        /// <summary>
        /// Mat3 and Matrix3x2 are both row-vector conventions with the translation in the
        /// third row, so the conversion is a straight copy of the six used entries.
        /// </summary>
        private static Matrix3x2 ToMatrix3x2(Mat3 m)
        {
            return new Matrix3x2(m.M11, m.M12, m.M21, m.M22, m.M13, m.M23);
        }

        /// <summary>
        /// Builds the Matrix3x2 equivalent of XAML's CompositeTransform: scale, then
        /// rotate, then translate (both use the same row-vector convention).
        /// </summary>
        private static Matrix3x2 Compose(double scaleX, double scaleY, double rotationDegrees, double tx, double ty)
        {
            float rad = (float)(rotationDegrees * Math.PI / 180.0);
            return Matrix3x2.CreateScale((float)scaleX, (float)scaleY)
                 * Matrix3x2.CreateRotation(rad)
                 * Matrix3x2.CreateTranslation((float)tx, (float)ty);
        }

        private static float Clamp01(double v)
        {
            if (v <= 0.0) return 0f;
            if (v >= 1.0) return 1f;
            return (float)v;
        }

        private static byte Channel(float v)
        {
            int i = (int)(v * 255f);
            if (i < 0) return 0;
            if (i > 255) return 255;
            return (byte)i;
        }

        private static Color ColorWithOpacity(RgbaF c, double opacity)
        {
            float a = (float)(c.A * opacity);
            return Color.FromArgb(Channel(a), Channel(c.R), Channel(c.G), Channel(c.B));
        }
    }
}
