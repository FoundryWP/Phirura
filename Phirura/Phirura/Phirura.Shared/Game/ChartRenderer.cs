using System;
using System.Collections.Generic;
using Phirura.Core;
using Windows.Foundation;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media;
using Windows.UI.Xaml.Media.Imaging;
using Windows.UI.Xaml.Shapes;

namespace Phirura.Game
{
    /// <summary>A note / line texture plus its natural pixel size.</summary>
    public sealed class ChartTexture
    {
        public string Path;
        public BitmapImage Source;
        public double Width = 1;
        public double Height = 1;

        /// <summary>
        /// Head-specific native resource for the same image. The XAML renderer uses
        /// <see cref="Source"/> (a BitmapImage); the Win2D renderer stores the CanvasBitmap
        /// here instead, so one loader can feed either backend.
        /// </summary>
        public object Tag;

        public double Aspect
        {
            get { return Height <= 0 ? 1.0 : Width / Height; }
        }
    }

    /// <summary>
    /// Maps chart space onto the screen.
    ///
    /// prpr renders the chart with the model matrix `diag(1, -1)` under a camera whose
    /// zoom is `(1, -aspectRatio)`, and every vertical quantity in the chart
    /// (`Object::now_translation`, `line_height`, note `base`) is divided by the aspect
    /// ratio. The visible chart region is therefore x in [-1, 1] and
    /// y in [-1/ar, 1/ar], which is exactly what this class implements:
    ///
    ///     screenX = (x + 1) / 2 * W
    ///     screenY = (1 - y * ar) / 2 * H
    ///
    /// so chart +y points *up* the screen, and a note with a larger `height`
    /// (i.e. further in the future) sits above the judge line.
    /// </summary>
    public static class ChartProjection
    {
        public static Mat3 ChartToScreen(double width, double height, float aspectRatio)
        {
            Mat3 m = new Mat3();
            m.M11 = (float)(width / 2.0);
            m.M12 = 0f;
            m.M13 = (float)(width / 2.0);
            m.M21 = 0f;
            m.M22 = (float)(-aspectRatio * height / 2.0);
            m.M23 = (float)(height / 2.0);
            m.M31 = 0f;
            m.M32 = 0f;
            m.M33 = 1f;
            return m;
        }
    }

    /// <summary>
    /// Port of the transform maths in prpr's Note::now_transform / JudgeLine::render.
    /// </summary>
    public static class Renderer
    {
        public const float RpeHeight = 900f;

        /// <summary>
        /// The note's transform inside its line's chart frame. `baseOffset` is
        /// `(note.height - line.height) / aspectRatio * speed` — the distance the note
        /// still has to travel before it reaches the line.
        /// </summary>
        public static Mat3 NoteTransform(Note note, JudgeLine line, float baseOffset, float inclineSin)
        {
            return NoteTransform(note, line, baseOffset, inclineSin, 16f / 9f, false);
        }

        public static Mat3 NoteTransform(Note note, JudgeLine line, float baseOffset, float inclineSin, float aspectRatio, bool noteUniformScale)
        {
            if (aspectRatio <= 0f) aspectRatio = 16f / 9f;

            float tyRaw = note.Obj.Translation.Y.TryNow(out tyRaw) ? tyRaw : 0f;
            float inclineVal = 1f - inclineSin * (baseOffset * aspectRatio + tyRaw) * RpeHeight / 2f / 360f;

            Vec2 tr = note.Obj.Translation.Now();
            tr.Y /= aspectRatio;
            bool isHold = note.Kind.Type == NoteKindType.Hold;
            tr.X *= isHold ? 1f : inclineVal * line.Ctrl.PosOr1();
            tr.Y += baseOffset;

            Vec2 scale = note.Obj.Scale.NowWithDef(1f, 1f);
            scale.X *= line.Ctrl.SizeOr1();
            if (noteUniformScale)
            {
                scale.Y *= line.Ctrl.SizeOr1();
            }
            else
            {
                scale.Y = 1f;
            }

            Mat3 m = Mat3.RotationDegrees(note.Obj.Rotation.Now());
            m = m.AppendNonuniformScaling(scale);
            m = m.AppendTranslation(tr);
            return m;
        }

        /// <summary>Decomposes an affine 2x2 + translation into XAML's CompositeTransform form.</summary>
        public static void Decompose(Mat3 m, out double scaleX, out double scaleY, out double rotationDeg, out double tx, out double ty)
        {
            tx = m.M13;
            ty = m.M23;
            double a = m.M11, b = m.M12, c = m.M21, d = m.M22;
            double sx = Math.Sqrt(a * a + c * c);
            if (sx < 1e-9)
            {
                scaleX = 0; scaleY = 0; rotationDeg = 0;
                return;
            }
            double cos = a / sx;
            double sin = c / sx;
            rotationDeg = Math.Atan2(sin, cos) * 180.0 / Math.PI;
            double sy = d * cos - b * sin;
            scaleX = sx;
            scaleY = sy;
        }
    }

    /// <summary>
    /// Draws a chart into a XAML Canvas using pooled elements.
    ///
    /// WP8.1 has no GPU SpriteBatch available to C#, so every note is a real XAML
    /// element whose CompositeTransform is updated each frame. Elements are pooled per
    /// texture and only notes whose transformed position falls near the viewport are
    /// realised, which keeps the live element count in the low hundreds.
    /// </summary>
    public sealed class ChartRenderer
    {
        private sealed class PooledElement
        {
            public Image Image;
            public CompositeTransform Transform;
            public bool Used;
        }

        private readonly Canvas _root;
        private readonly double _width;
        private readonly double _height;
        private readonly float _aspectRatio;

        private readonly ChartTexture _click;
        private readonly ChartTexture _drag;
        private readonly ChartTexture _flick;
        private readonly ChartTexture _holdHead;
        private readonly ChartTexture _holdBody;
        private readonly ChartTexture _holdTail;
        private readonly Dictionary<string, ChartTexture> _lineTextures = new Dictionary<string, ChartTexture>();

        private readonly Dictionary<ChartTexture, List<PooledElement>> _pools = new Dictionary<ChartTexture, List<PooledElement>>();
        private readonly List<PooledElement> _all = new List<PooledElement>();

        private readonly List<PooledElement> _lines = new List<PooledElement>();
        private readonly List<Rectangle> _plainLines = new List<Rectangle>();

        private Mat3 _chartToScreen;

        public int LiveElements
        {
            get { return _all.Count; }
        }

        /// <summary>Flipping this mirrors the chart vertically; useful if a device's
        /// orientation ends up opposite to the assumed camera convention.</summary>
        public bool FlipVertical;

        public ChartRenderer(Canvas root, double width, double height, float aspectRatio,
                             ChartTexture click, ChartTexture drag, ChartTexture flick,
                             ChartTexture holdHead, ChartTexture holdBody, ChartTexture holdTail)
        {
            _root = root;
            _width = width;
            _height = height;
            _aspectRatio = aspectRatio <= 0f ? 16f / 9f : aspectRatio;
            _click = click;
            _drag = drag;
            _flick = flick;
            _holdHead = holdHead;
            _holdBody = holdBody;
            _holdTail = holdTail;
            _chartToScreen = ChartProjection.ChartToScreen(width, height, _aspectRatio);
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
            _root.Children.Clear();
            _pools.Clear();
            _all.Clear();
            _lines.Clear();
            _plainLines.Clear();
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

        private PooledElement Rent(ChartTexture tex)
        {
            List<PooledElement> pool;
            if (!_pools.TryGetValue(tex, out pool))
            {
                pool = new List<PooledElement>();
                _pools[tex] = pool;
            }
            for (int i = 0; i < pool.Count; i++)
            {
                if (!pool[i].Used)
                {
                    pool[i].Used = true;
                    pool[i].Image.Visibility = Visibility.Visible;
                    return pool[i];
                }
            }
            Image img = new Image();
            img.Source = tex.Source;
            img.Width = tex.Width;
            img.Height = tex.Height;
            img.Stretch = Stretch.Fill;
            img.RenderTransformOrigin = new Point(0.5, 0.5);
            img.IsHitTestVisible = false;
            Canvas.SetLeft(img, -tex.Width / 2.0);
            Canvas.SetTop(img, -tex.Height / 2.0);
            CompositeTransform tr = new CompositeTransform();
            img.RenderTransform = tr;
            PooledElement el = new PooledElement();
            el.Image = img;
            el.Transform = tr;
            el.Used = true;
            _root.Children.Add(img);
            pool.Add(el);
            _all.Add(el);
            return el;
        }

        private void ReleaseAll()
        {
            for (int i = 0; i < _all.Count; i++)
            {
                _all[i].Used = false;
                _all[i].Image.Visibility = Visibility.Collapsed;
            }
            for (int i = 0; i < _plainLines.Count; i++)
            {
                _plainLines[i].Visibility = Visibility.Collapsed;
            }
        }

        private void Place(PooledElement el, Mat3 matrix, double opacity, bool visible)
        {
            if (!visible || opacity <= 0.003)
            {
                el.Image.Visibility = Visibility.Collapsed;
                return;
            }
            double sx, sy, rot, tx, ty;
            Renderer.Decompose(matrix, out sx, out sy, out rot, out tx, out ty);
            el.Transform.ScaleX = sx;
            el.Transform.ScaleY = sy;
            el.Transform.Rotation = rot;
            el.Transform.TranslateX = tx;
            el.Transform.TranslateY = ty;
            el.Image.Opacity = opacity > 1.0 ? 1.0 : opacity;
            el.Image.Visibility = Visibility.Visible;
        }

        private Rectangle RentPlainLine()
        {
            for (int i = 0; i < _plainLines.Count; i++)
            {
                if (_plainLines[i].Visibility != Visibility.Visible)
                {
                    _plainLines[i].Visibility = Visibility.Visible;
                    return _plainLines[i];
                }
            }
            Rectangle r = new Rectangle();
            r.IsHitTestVisible = false;
            r.RenderTransformOrigin = new Point(0.5, 0.5);
            r.RenderTransform = new CompositeTransform();
            _root.Children.Add(r);
            _plainLines.Add(r);
            return r;
        }

        private void PlacePlainLine(Mat3 matrix, RgbaF color, double opacity, double thickness)
        {
            Rectangle r = RentPlainLine();
            double sx, sy, rot, tx, ty;
            Renderer.Decompose(matrix, out sx, out sy, out rot, out tx, out ty);
            r.Width = _width * 3.0;
            r.Height = Math.Max(1.0, thickness);
            r.Fill = new SolidColorBrush(ToColor(color));
            Canvas.SetLeft(r, -r.Width / 2.0);
            Canvas.SetTop(r, -r.Height / 2.0);
            CompositeTransform ct = (CompositeTransform)r.RenderTransform;
            ct.ScaleX = sx;
            ct.ScaleY = 1.0;
            ct.Rotation = rot;
            ct.TranslateX = tx;
            ct.TranslateY = ty;
            r.Opacity = opacity > 1.0 ? 1.0 : opacity;
        }

        private static Windows.UI.Color ToColor(RgbaF c)
        {
            return Windows.UI.Color.FromArgb(
                (byte)Math.Max(0, Math.Min(255, c.A * 255f)),
                (byte)Math.Max(0, Math.Min(255, c.R * 255f)),
                (byte)Math.Max(0, Math.Min(255, c.G * 255f)),
                (byte)Math.Max(0, Math.Min(255, c.B * 255f)));
        }

        /// <summary>Half-width of a note in chart units.</summary>
        public float NoteWidth = (float)Consts.NoteWidthRatioBase;

        public void Render(Chart chart, double time, float globalAlpha, double lineThicknessPx)
        {
            ReleaseAll();
            if (chart == null)
            {
                return;
            }
            Mat3 toScreen = ScreenMatrix();
            float ar = _aspectRatio;
            double margin = Math.Max(_width, _height) * 0.6;

            for (int oi = 0; oi < chart.Order.Count; oi++)
            {
                JudgeLine line = chart.Lines[chart.Order[oi]];
                line.Obj.SetTime(time);
                line.Height.SetTime(time);
                line.Color.SetTime(time);
                line.Incline.SetTime(time);
                if (line.TextAnim != null) line.TextAnim.SetTime(time);
                if (line.PaintAnim != null) line.PaintAnim.SetTime(time);

                float alpha;
                float lineAlpha = line.Obj.Alpha.TryNow(out alpha) ? alpha : 1f;
                lineAlpha *= globalAlpha;
                if (lineAlpha <= 0f && !chart.Settings.PeAlphaExtension)
                {
                    continue;
                }

                RgbaF lineColor;
                RgbaF lc = line.Color.Now();
                lineColor = lc;

                Mat3 lineChart = line.NowTransform(ar, chart.Lines);
                Mat3 lineToScreen = toScreen * lineChart;

                // ---- the line itself ----
                double lineOp = Math.Max(0.0, lineAlpha) * (lineColor.A <= 0f ? 1.0 : lineColor.A);
                if (line.Kind == JudgeLineKindType.Normal)
                {
                    RgbaF c = lineColor;
                    if (c.A <= 0f) c = new RgbaF(1f, 1f, 1f, 1f);
                    PlacePlainLine(lineToScreen, c, lineOp, lineThicknessPx);
                }
                else if (line.Kind == JudgeLineKindType.Texture)
                {
                    ChartTexture tex;
                    if (line.TexturePath != null && _lineTextures.TryGetValue(line.TexturePath, out tex))
                    {
                        Vec2 scale = line.Obj.Scale.NowWithDef(1f, 1f);
                        Mat3 m = lineToScreen;
                        PooledElement el = Rent(tex);
                        // Textures are drawn at natural pixel size, scaled by the line scale
                        // (base size is the texture's pixel size, so fold the rest in).
                        double sx, sy, rot, tx, ty;
                        Renderer.Decompose(m, out sx, out sy, out rot, out tx, out ty);
                        el.Transform.ScaleX = sx * scale.X / tex.Width;
                        el.Transform.ScaleY = sy * scale.Y / tex.Height;
                        el.Transform.Rotation = rot;
                        el.Transform.TranslateX = tx;
                        el.Transform.TranslateY = ty;
                        el.Image.Width = tex.Width;
                        el.Image.Height = tex.Height;
                        Canvas.SetLeft(el.Image, -tex.Width / 2.0);
                        Canvas.SetTop(el.Image, -tex.Height / 2.0);
                        el.Image.Opacity = lineOp > 1.0 ? 1.0 : lineOp;
                        el.Image.Visibility = Visibility.Visible;
                    }
                }
                else if (line.Kind == JudgeLineKindType.Paint)
                {
                    // "Paint" lines fill the screen with a colour; approximated with a rect.
                    double size = line.PaintAnim != null ? line.PaintAnim.Now() : 0.0;
                    if (size > 0)
                    {
                        RgbaF c = lineColor;
                        c.A = (float)Math.Min(1.0, lineAlpha * 2.55 * (c.A <= 0f ? 1f : c.A));
                        PlacePlainLine(toScreen, c, 1.0, _height * 3.0);
                    }
                }

                // ---- notes ----
                List<Note> notes = line.Notes;
                for (int ni = 0; ni < notes.Count; ni++)
                {
                    Note note = notes[ni];
                    if (note.Judge.Kind == JudgeStatusKind.Judged && note.Kind.Type != NoteKindType.Hold)
                    {
                        continue;
                    }

                    double noteAlpha = note.Obj.NowAlpha() * globalAlpha * line.Obj.NowAlpha();
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
                    double lineHeight = line.Height.Now();
                    double spd = note.Speed * line.Ctrl.YOr1();
                    float baseVal = (float)((note.Height - lineHeight) / ar * spd);
                    float inclineSin;
                    float inclineDeg = line.Incline.TryNow(out inclineDeg) ? inclineDeg : 0f;
                    inclineSin = (float)Math.Sin(inclineDeg * Math.PI / 180.0);

                    Mat3 noteChart = Renderer.NoteTransform(note, line, baseVal, inclineSin, ar, false);
                    Mat3 frame = lineChart;
                    if (!note.Above)
                    {
                        frame = frame * Mat3.Scaling(1f, -1f);
                    }
                    Mat3 full = toScreen * frame * noteChart;

                    // Cheap viewport test on the note's origin.
                    Vec2 origin = full.TransformPoint(Vec2.Zero);
                    if (origin.X < -margin || origin.X > _width + margin || origin.Y < -margin || origin.Y > _height + margin)
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
                        RenderHold(toScreen, frame, note, line, lineChart, baseVal, inclineSin, ar, spd, op);
                        continue;
                    }

                    ChartTexture tex = note.Kind.Type == NoteKindType.Flick ? _flick
                        : note.Kind.Type == NoteKindType.Drag ? _drag : _click;
                    PooledElement el = Rent(tex);
                    double nsx, nsy, nrot, ntx, nty;
                    Renderer.Decompose(full, out nsx, out nsy, out nrot, out ntx, out nty);
                    double baseHalfW = tex.Width / 2.0;
                    double baseHalfH = tex.Height / 2.0;
                    double noteHalfWpx = NoteWidth * (_width / 2.0);
                    el.Transform.ScaleX = noteHalfWpx / baseHalfW;
                    el.Transform.ScaleY = noteHalfWpx / baseHalfW;
                    el.Transform.Rotation = nrot;
                    el.Transform.TranslateX = ntx;
                    el.Transform.TranslateY = nty;
                    el.Image.Opacity = op > 1.0 ? 1.0 : op;
                    el.Image.Visibility = Visibility.Visible;
                }
            }
        }

        /// <summary>How long a "ghost" note (hit far too early / late) stays on screen.</summary>
        public const double BadNoteLifetime = 0.5;

        /// <summary>
        /// Draws the grey ghost that marks a note which was hit outside the BAD window.
        /// `bn.Matrix` is already the note's chart-space matrix (line transform * note
        /// transform), so it only needs projecting.
        ///
        /// Upstream tints the ghost black; WP8.1's Image has no cheap tint channel, so we
        /// approximate it with a fast fade instead.
        /// </summary>
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

            ChartTexture tex = bn.Kind.Type == NoteKindType.Flick ? _flick
                : bn.Kind.Type == NoteKindType.Drag ? _drag : _click;
            if (tex == null)
            {
                return;
            }

            double op = 1.0 - age / BadNoteLifetime;
            Mat3 full = ScreenMatrix() * bn.Matrix;

            PooledElement el = Rent(tex);
            el.Image.Width = tex.Width;
            el.Image.Height = tex.Height;
            Canvas.SetLeft(el.Image, -tex.Width / 2.0);
            Canvas.SetTop(el.Image, -tex.Height / 2.0);

            double nsx, nsy, nrot, ntx, nty;
            Renderer.Decompose(full, out nsx, out nsy, out nrot, out ntx, out nty);
            double noteHalfWpx = NoteWidth * (_width / 2.0);
            el.Transform.ScaleX = noteHalfWpx / (tex.Width / 2.0);
            el.Transform.ScaleY = noteHalfWpx / (tex.Width / 2.0);
            el.Transform.Rotation = nrot;
            el.Transform.TranslateX = ntx;
            el.Transform.TranslateY = nty;
            el.Image.Opacity = op > 1.0 ? 1.0 : op;
            el.Image.Visibility = Visibility.Visible;
        }

        private void RenderHold(Mat3 toScreen, Mat3 frame, Note note, JudgeLine line, Mat3 lineChart,
                                float baseVal, float inclineSin, float ar, double spd, double opacity)
        {
            if (_holdHead == null || _holdTail == null || _holdBody == null)
            {
                return;
            }
            double lineHeight = line.Height.Now();
            float endBase = (float)((note.Kind.EndHeight - lineHeight) / ar * spd);

            // Body: from the note head to the tail, along chart +y.
            double bottom = baseVal;
            double top = endBase;
            double len = top - bottom;

            double noteHalfWpx = NoteWidth * (_width / 2.0);
            // Chart y -> pixels along the line's local +y axis.
            Mat3 unit = toScreen * frame;
            Vec2 p0 = unit.TransformPoint(new Vec2(0f, (float)bottom));
            Vec2 p1 = unit.TransformPoint(new Vec2(0f, (float)top));
            double dx = p1.X - p0.X;
            double dy = p1.Y - p0.Y;
            double lenPx = Math.Sqrt(dx * dx + dy * dy);
            if (lenPx < 1.0)
            {
                return;
            }
            double angle = Math.Atan2(dy, dx) * 180.0 / Math.PI;

            // --- body ---
            PooledElement body = Rent(_holdBody);
            body.Image.Width = _holdBody.Width;
            body.Image.Height = _holdBody.Height;
            Canvas.SetLeft(body.Image, -_holdBody.Width / 2.0);
            Canvas.SetTop(body.Image, -_holdBody.Height / 2.0);
            body.Transform.ScaleX = (noteHalfWpx * 2.0) / _holdBody.Width;
            body.Transform.ScaleY = lenPx / _holdBody.Height;
            body.Transform.Rotation = angle - 90.0;
            body.Transform.TranslateX = (p0.X + p1.X) / 2.0;
            body.Transform.TranslateY = (p0.Y + p1.Y) / 2.0;
            body.Image.Opacity = opacity > 1.0 ? 1.0 : opacity;
            body.Image.Visibility = Visibility.Visible;

            // --- head (at the judgement point) ---
            PlaceCap(toScreen, frame, _holdHead, p0, dx / lenPx, dy / lenPx, noteHalfWpx, opacity, true);
            // --- tail (at the far end) ---
            PlaceCap(toScreen, frame, _holdTail, p1, dx / lenPx, dy / lenPx, noteHalfWpx, opacity, false);
        }

        private void PlaceCap(Mat3 toScreen, Mat3 frame, ChartTexture tex, Vec2 at, double ux, double uy, double noteHalfWpx, double opacity, bool isHead)
        {
            PooledElement el = Rent(tex);
            el.Image.Width = tex.Width;
            el.Image.Height = tex.Height;
            Canvas.SetLeft(el.Image, -tex.Width / 2.0);
            Canvas.SetTop(el.Image, -tex.Height / 2.0);
            double angle = Math.Atan2(uy, ux) * 180.0 / Math.PI;
            double halfH = tex.Height / 2.0 * (noteHalfWpx * 2.0 / tex.Width);
            // Offset along the hold axis so the cap sits outside the body.
            double offX = isHead ? -ux * halfH : ux * halfH;
            double offY = isHead ? -uy * halfH : uy * halfH;
            el.Transform.ScaleX = (noteHalfWpx * 2.0) / tex.Width;
            el.Transform.ScaleY = (noteHalfWpx * 2.0) / tex.Width;
            el.Transform.Rotation = angle - 90.0;
            el.Transform.TranslateX = at.X + offX;
            el.Transform.TranslateY = at.Y + offY;
            el.Image.Opacity = opacity > 1.0 ? 1.0 : opacity;
            el.Image.Visibility = Visibility.Visible;
        }
    }
}
