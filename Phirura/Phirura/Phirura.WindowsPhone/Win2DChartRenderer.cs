using System;
using System.Collections.Generic;
using System.Numerics;
using Microsoft.Graphics.Canvas;
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
                OnPortDraw(ds);
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
            _canvas.Invalidate();
        }

        /// <summary>
        /// Replays one recorded frame. Called from <see cref="OnDraw"/> in place of the chart path.
        /// </summary>
        private void OnPortDraw(CanvasDrawingSession ds)
        {
            EnsurePortTextures(ds);

            PrprPort.DrainFrame frame = _portFrame;
            if (frame == null || frame.Count == 0)
            {
                PortCommands = 0;
                return;
            }

            PortCommands = frame.Count;
            PrprPort.PortCmd[] cmds = frame.Commands;

            // Geometry census, throttled to ~1 Hz - see the method below.
            PortFrameStats(frame, cmds);

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

            for (int i = 0; i < frame.Count; i++)
            {
                PrprPort.PortCmd c = cmds[i];
                // The kind's high bits carry the flip flags for textures, so mask before dispatching.
                uint kind = c.kind & 0xFFFF;

                switch (kind)
                {
                    case 0: // Texture: dest x/y/w/h, then source x/y/w/h
                        DrawPortTexture(ds, c);
                        break;

                    case 1: // Rect: x/y/w/h then r/g/b/a
                        ds.FillRectangle(
                            c.f0, c.f1, c.f2, c.f3,
                            FloatColor(c.f4, c.f5, c.f6, c.f7));
                        break;

                    case 2: // Line: x1/y1/x2/y2, thickness, then r/g/b
                        ds.DrawLine(
                            c.f0, c.f1, c.f2, c.f3,
                            FloatColor(c.f5, c.f6, c.f7, 1f),
                            c.f4);
                        break;

                    case 3: // Circle: cx/cy/r then r/g/b/a
                        ds.FillCircle(
                            c.f0, c.f1, c.f2,
                            FloatColor(c.f3, c.f4, c.f5, c.f6));
                        break;

                    case 4: // Geometry: vertices carry position, uv and colour
                        DrawPortGeometry(ds, frame, c);
                        break;
                }
            }

            ds.Transform = portTransform;
        }

        /// <summary>Rate gate for <see cref="PortFrameStats"/>.</summary>
        private int _portStatsTick = int.MinValue / 2;

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
        /// Walked once a second rather than per frame: a busy chart has a few hundred commands and
        /// this reads all of them, which is real work to do sixty times over.
        /// </summary>
        private void PortFrameStats(PrprPort.DrainFrame frame, PrprPort.PortCmd[] cmds)
        {
            int now = Environment.TickCount;
            if (unchecked(now - _portStatsTick) < 1000)
            {
                return;
            }
            _portStatsTick = now;

            int nTex = 0, nRect = 0, nLine = 0, nCircle = 0, nGeo = 0;
            double minX = double.MaxValue, minY = double.MaxValue;
            double maxX = double.MinValue, maxY = double.MinValue;
            double vx0 = double.MaxValue, vy0 = double.MaxValue;
            double vx1 = double.MinValue, vy1 = double.MinValue;
            int nVert = 0;

            for (int i = 0; i < frame.Count; i++)
            {
                PrprPort.PortCmd c = cmds[i];
                switch (c.kind & 0xFFFF)
                {
                    case 0:
                        nTex++;
                        break;

                    case 1:
                        nRect++;
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
                            for (int vi = vs; vi < ve; vi++)
                            {
                                float gx = frame.Vertices[vi].x;
                                float gy = frame.Vertices[vi].y;
                                if (gx < vx0) vx0 = gx;
                                if (gy < vy0) vy0 = gy;
                                if (gx > vx1) vx1 = gx;
                                if (gy > vy1) vy1 = gy;
                            }
                            nVert += ve - vs;
                        }
                        break;
                }
            }

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

            double dipW = _canvas == null ? 0.0 : _canvas.ActualWidth;
            double dipH = _canvas == null ? 0.0 : _canvas.ActualHeight;
            string pxToDip = (PrprPort.ScreenPixelWidth > 0f && dipW > 0.0)
                ? (dipW / PrprPort.ScreenPixelWidth).ToString("0.###")
                : "?";

            AppLog.WriteSync("Win2D: port frame n=" + frame.Count
                + " tex=" + nTex + " rect=" + nRect + " line=" + nLine
                + " circ=" + nCircle + " geo=" + nGeo
                + " rectBbox=" + bbox + " vertBbox=" + vertBbox
                + " canvasDip=" + dipW.ToString("0.##") + "x" + dipH.ToString("0.##")
                + " pxToDip=" + pxToDip);
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
                ds.DrawImage(image, new Rect(-dw / 2f, -dh / 2f, dw, dh), source, 1f, CanvasImageInterpolation.Linear);
                ds.Transform = saved;
            }
            else
            {
                ds.DrawImage(image, new Rect(dx, dy, dw, dh), source, 1f, CanvasImageInterpolation.Linear);
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
            bool hasTex = _portTextures.ContainsKey(c.flags);

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
                    PrprPort.PortVertex v2 = Vert(frame, (int)c.vstart + (int)frame.Indices[i2]);
                    FillPortTriangle(ds, v0, v1, v2, c.flags, hasTex);
                }
                else
                {
                    // Lines / line strips: the shim uses these for hold bodies, which are plain
                    // strokes rather than filled geometry.
                    ds.DrawLine(v0.x, v0.y, v1.x, v1.y,
                        FloatColor(v0.r, v0.g, v0.b, 1f), 2f);
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
        private void FillPortTriangle(CanvasDrawingSession ds, PrprPort.PortVertex a,
                                      PrprPort.PortVertex b, PrprPort.PortVertex c,
                                      uint texId, bool hasTex)
        {
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
                ICanvasImage image = tex.Target;
                ds.DrawImage(image,
                    new Rect(minX, minY, maxX - minX, maxY - minY),
                    new Rect(u0 * tex.PixelWidth, v0 * tex.PixelHeight,
                             (u1 - u0) * tex.PixelWidth, (v1 - v0) * tex.PixelHeight),
                    1f, CanvasImageInterpolation.Linear);
                return;
            }

            // Vertex colours are per-vertex in the shim; a flat fill uses the first. Averaging would
            // be closer to a gradient, but Win2D.win81 cannot interpolate across a triangle either.
            ds.FillGeometry(PortTriangle(ds, a, b, c), FloatColor(a.r, a.g, a.b, 1f));
        }

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
        private void EnsurePortTextures(CanvasDrawingSession ds)
        {
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

            try
            {
                int total = PrprPort.TextureCount();
                if (total > 0)
                {
                    if (_portIds == null || _portIds.Length < total)
                    {
                        // Grows with the registry and never per frame.
                        _portIds = new uint[total];
                    }
                    PrprPort.CopyTextureIds(_portIds, total);

                    for (int i = 0; i < total; i++)
                    {
                        uint id = _portIds[i];
                        if (id != 0)
                        {
                            PollPortTexture(ds, id);
                        }
                    }
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
        /// One id's contribution to this frame: nothing at all, a delta, or a first full read.
        /// </summary>
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
                uint w, h;
                if (!PrprPort.TextureSize(id, out w, out h))
                {
                    return;
                }
                pt = CreatePortTexture(ds, id, w, h);
                if (pt == null)
                {
                    return;
                }
                _portTextures[id] = pt;

                // The whole texture, whatever the dirty read said. For a fresh registration the two
                // agree by contract; where they do not - the flag was drained somewhere else first -
                // only a full read leaves a carrier that is not half blank, and either way this runs
                // once per texture.
                SyncPortTexture(ds, id, pt, 0, 0, w, h, null);
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
                    return;
                }
                pt = CreatePortTexture(ds, id, w, h);
                if (pt == null)
                {
                    return;
                }
                _portTextures[id] = pt;
                SyncPortTexture(ds, id, pt, 0, 0, w, h, null);
                return;
            }

            // The incremental case: a rectangle that is usually a few hundred pixels of glyph.
            SyncPortTexture(ds, id, pt, rect.X, rect.Y, rect.W, rect.H, null);
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
                CanvasRenderTarget target = new CanvasRenderTarget(ds.Device, w, h, 96f);
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
        private void SyncPortTexture(CanvasDrawingSession ds, uint id, PortTexture pt,
                                     uint x, uint y, uint w, uint h, byte[] preload)
        {
            if (w == 0 || h == 0 || pt.Target == null)
            {
                return;
            }

            bool full = x == 0 && y == 0 && w == pt.PixelWidth && h == pt.PixelHeight;
            long bytes = 0;
            // The content probe runs on the first band of a full upload only: it is the pixel data
            // itself that decides whether "white" means "the texture really is white" or "the
            // texture is fine and something else is hiding it", and re-probing every delta would
            // pay for an answer that cannot change.
            bool firstBand = full;
            System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
            try
            {
                using (CanvasDrawingSession target = pt.Target.CreateDrawingSession())
                {
                    uint row = 0;
                    while (row < h)
                    {
                        uint band = preload != null ? h : BandRows(w, h - row);
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
                                return;
                            }
                        }
                        bytes += rgba.Length;
                        if (firstBand)
                        {
                            firstBand = false;
                            AppLog.WriteSync("Win2D: port texture " + id + " band0 "
                                + w + "x" + band + " " + BandSummary(rgba));
                        }
                        PaintPortBand(ds.Device, target, x, y + row, w, band, rgba);
                        row += band;
                    }
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
                AppLog.WriteSync("Win2D: port texture " + id + " sync failed", ex);
            }
            finally
            {
                clock.Stop();
                AppLog.WriteSync("Win2D: port texture " + id
                    + (full ? " init" : " delta")
                    + " rect=(" + x + "," + y + "," + w + "," + h + ")"
                    + " bytes=" + bytes
                    + " ms=" + clock.ElapsedMilliseconds);
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
                pixels[p] = Color.FromArgb(rgba[o + 3], rgba[o + 0], rgba[o + 1], rgba[o + 2]);
            }

            using (CanvasBitmap patch = CanvasBitmap.CreateFromColors(dev, pixels, (int)w, (int)h))
            {
                target.DrawImage(patch,
                    new Rect(x, y, w, h),
                    patch.Bounds,
                    1f,
                    CanvasImageInterpolation.NearestNeighbor);
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
                SyncPortTexture(ds, t.Id, pt, 0, 0, t.Width, t.Height, t.Rgba);
            }
            PortTexturesBound = _portTextures.Count;
        }

        /// <summary>Packs 0..1 floats into a Win2D colour, clamping out-of-range input.</summary>
        private static Color FloatColor(float r, float g, float b, float a)
        {
            return Color.FromArgb(Channel(a), Channel(r), Channel(g), Channel(b));
        }

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
