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
