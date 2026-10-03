using System;
using System.Collections.Generic;
using System.Numerics;
using Microsoft.Graphics.Canvas;
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

        private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
        {
            CanvasDrawingSession ds = args.DrawingSession;
            ds.Clear(Background);

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

        /// <summary>Rust texture id -> bitmap, built lazily from the shim's registry.</summary>
        private readonly Dictionary<uint, CanvasBitmap> _portTextures =
            new Dictionary<uint, CanvasBitmap>();

        /// <summary>True once the port's textures have been resolved for the current game.</summary>
        private bool _portTexturesReady;

        /// <summary>Set by <see cref="DrawPortFrame"/>; the next draw replays this instead of the chart.</summary>
        private PrprPort.DrainFrame _portFrame;

        /// <summary>Reported in the diagnostics line so a blank port frame can be told from an unbound texture.</summary>
        public int PortCommands;
        public int PortTexturesBound;

        /// <summary>
        /// Forgets the port's textures. Call when the game is destroyed: the next game allocates its
        /// own ids and reusing these would draw the previous chart's sprites.
        /// </summary>
        public void ResetPortTextures()
        {
            _portTextures.Clear();
            _portTexturesReady = false;
            _portFrame = null;
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
        }

        private void DrawPortTexture(CanvasDrawingSession ds, PrprPort.PortCmd c)
        {
            CanvasBitmap bmp;
            if (!_portTextures.TryGetValue(c.flags, out bmp) || bmp == null)
            {
                // No bitmap for this id - the shell never loaded it, or the registry dropped it.
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

            // Source rect in the bitmap's own pixel space. The flip flags are folded into the
            // destination rectangle instead of the source, because Win2D.win81's DrawImage takes the
            // source as an axis-aligned Rect and cannot mirror it.
            Rect source = new Rect(sx, sy, sw, sh);

            Matrix3x2 saved = ds.Transform;
            if (flipX || flipY)
            {
                float cx = dx + dw / 2f;
                float cy = dy + dh / 2f;
                ds.Transform = Matrix3x2.CreateScale(flipX ? -1f : 1f, flipY ? -1f : 1f)
                             * Matrix3x2.CreateTranslation(cx, cy);
                ds.DrawImage(bmp, new Rect(-dw / 2f, -dh / 2f, dw, dh), source, 1f, CanvasImageInterpolation.Linear);
                ds.Transform = saved;
            }
            else
            {
                ds.DrawImage(bmp, new Rect(dx, dy, dw, dh), source, 1f, CanvasImageInterpolation.Linear);
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
                PrprPort.PortVertex v0 = Vert(frame, c.vstart + frame.Indices[i0]);
                PrprPort.PortVertex v1 = Vert(frame, c.vstart + frame.Indices[i1]);

                if (mode == 0)
                {
                    if (i2 >= frame.Indices.Length) break;
                    PrprPort.PortVertex v2 = Vert(frame, c.vstart + frame.Indices[i2]);
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
        /// Fills one triangle. Textured when the batch names a bitmap the shell has, flat-coloured
        /// otherwise - which is what `prpr`'s untextured note bodies rely on.
        /// </summary>
        private void FillPortTriangle(CanvasDrawingSession ds, PrprPort.PortVertex a,
                                      PrprPort.PortVertex b, PrprPort.PortVertex c,
                                      uint texId, bool hasTex)
        {
            CanvasBitmap bmp;
            if (hasTex && _portTextures.TryGetValue(texId, out bmp) && bmp != null)
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
                ds.DrawImage(bmp,
                    new Rect(minX, minY, maxX - minX, maxY - minY),
                    new Rect(u0 * bmp.SizeInPixels.Width, v0 * bmp.SizeInPixels.Height,
                             (u1 - u0) * bmp.SizeInPixels.Width, (v1 - v0) * bmp.SizeInPixels.Height),
                    1f, CanvasImageInterpolation.Linear);
                return;
            }

            // Vertex colours are per-vertex in the shim; a flat fill uses the first. Averaging would
            // be closer to a gradient, but Win2D.win81 cannot interpolate across a triangle either.
            ds.FillGeometry(PortTriangle(a, b, c), FloatColor(a.r, a.g, a.b, 1f));
        }

        private static CanvasGeometry PortTriangle(PrprPort.PortVertex a, PrprPort.PortVertex b,
                                                    PrprPort.PortVertex c)
        {
            using (CanvasPathBuilder pb = new CanvasPathBuilder(null))
            {
                pb.BeginFigure(a.x, a.y);
                pb.AddLine(b.x, b.y);
                pb.AddLine(c.x, c.y);
                pb.EndFigure(CanvasFigureLoop.Closed);
                return CanvasGeometry.CreatePath(pb);
            }
        }

        /// <summary>
        /// Resolves every id the Rust shim has registered into a <see cref="CanvasBitmap"/>.
        ///
        /// Runs once per game. Ids are allocated where the shim sees pixels, so the only way to learn
        /// them is to ask - see <see cref="PrprPort.ReadTextures"/>.
        /// </summary>
        private void EnsurePortTextures(CanvasDrawingSession ds)
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
                try
                {
                    // Direct3D expects BGRA; the shim holds RGBA. Swapping here rather than on the
                    // Rust side keeps the shim's `from_rgba8` contract intact for any other caller.
                    byte[] bgra = new byte[t.Rgba.Length];
                    for (int p = 0; p + 3 < t.Rgba.Length; p += 4)
                    {
                        bgra[p + 0] = t.Rgba[p + 2];
                        bgra[p + 1] = t.Rgba[p + 1];
                        bgra[p + 2] = t.Rgba[p + 0];
                        bgra[p + 3] = t.Rgba[p + 3];
                    }
                    _portTextures[t.Id] = CanvasBitmap.CreateFromBytes(
                        ds.Device, bgra, (int)t.Width, (int)t.Height,
                        Windows.Graphics.DirectX.DirectXPixelFormat.B8G8R8A8UIntNormalized);
                }
                catch (Exception ex)
                {
                    AppLog.WriteSync("Win2D: port texture " + t.Id + " failed", ex);
                }
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
