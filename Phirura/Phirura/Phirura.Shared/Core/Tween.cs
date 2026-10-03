using System;

namespace Phirura.Core
{
    public enum TweenKind
    {
        Static,
        IntStatic,
        Clamped,
        IntClamped,
        Bezier,
        GeneralInt,
        SpeedIntegral
    }

    /// <summary>Port of prpr::core::TweenFunction.</summary>
    public interface ITweenFunction
    {
        float Y(float x);
        TweenKind Kind { get; }
    }

    public static class TweenMath
    {
        public static float Lerp(float a, float b, float t)
        {
            return a + (b - a) * t;
        }
    }

    public static class TweenExtensions
    {
        public static float Derivative(this ITweenFunction f, float x)
        {
            const float eps = 1e-6f;
            float l = Math.Max(x - eps, 1e-7f);
            float r = Math.Min(x + eps, 1f - 1e-7f);
            if (r <= l)
            {
                return 0f;
            }
            return (f.Y(r) - f.Y(l)) / (r - l);
        }
    }

    /// <summary>Port of prpr's TWEEN_FUNCTIONS table (33 easings).</summary>
    public static class Easing
    {
        public const float PI = (float)Math.PI;

        private static float Sine(float x) { return 1f - (float)Math.Cos((x * PI) / 2.0); }
        private static float Quad(float x) { return x * x; }
        private static float Cubic(float x) { return x * x * x; }
        private static float Quart(float x) { return x * x * x * x; }
        private static float Quint(float x) { return x * x * x * x * x; }
        private static float Expo(float x) { return (float)Math.Pow(2.0, 10.0 * (x - 1.0)); }
        private static float Circ(float x) { return 1f - (float)Math.Sqrt(Math.Max(0.0, 1.0 - (double)x * x)); }

        private static float Back(float x)
        {
            const float C1 = 1.70158f;
            const float C3 = C1 + 1f;
            return (C3 * x - C1) * x * x;
        }

        private static float Elastic(float x)
        {
            const float C4 = (2f * PI) / 3f;
            return -(float)(Math.Pow(2.0, 10.0 * x - 10.0) * Math.Sin((x * 10.0 - 10.75) * C4));
        }

        private static float Bounce(float x)
        {
            const float N1 = 7.5625f;
            const float D1 = 2.75f;
            float y = 1f - x;
            if (y < 1f / D1) return 1f - (N1 * y * y);
            if (y < 2f / D1) return 1f - (N1 * (y - 1.5f / D1) * (y - 1.5f / D1) + 0.75f);
            if (y < 2.5f / D1) return 1f - (N1 * (y - 2.25f / D1) * (y - 2.25f / D1) + 0.9375f);
            return 1f - (N1 * (y - 2.625f / D1) * (y - 2.625f / D1) + 0.984375f);
        }

        // ---- integral easings, used by RPE >= 170 "modern" speed events ----

        private static float IntSine(float x) { return x - (float)Math.Sin(x * PI / 2.0) * (2f / PI); }
        private static float IntQuad(float x) { return x * x * x / 3f; }
        private static float IntCubic(float x) { return x * x * x * x / 4f; }
        private static float IntQuart(float x) { return (float)Math.Pow(x, 5.0) / 5f; }
        private static float IntQuint(float x) { return (float)Math.Pow(x, 6.0) / 6f; }

        private static float IntExpo(float x)
        {
            float ln2 = (float)Math.Log(2.0);
            return (float)((Math.Pow(2.0, 10.0 * x - 10.0) - Math.Pow(2.0, -10.0)) / (10.0 * ln2));
        }

        private static float IntCirc(float x)
        {
            return x - 0.5f * (x * (float)Math.Sqrt(Math.Max(0.0, 1.0 - (double)x * x)) + (float)Math.Asin(x));
        }

        private static float IntBack(float x)
        {
            const float C1 = 1.70158f;
            const float C3 = C1 + 1f;
            return (C3 * x / 4f - C1 / 3f) * x * x * x;
        }

        private static float IntElasticAntideriv(float x)
        {
            const float C4 = (2f * PI) / 3f;
            float a = (float)Math.Log(2.0);
            float b = C4;
            float u = 10f * x - 10f;
            float v = (x * 10f - 10.75f) * b;
            return -(float)(Math.Pow(2.0, u) / (10.0 * (a * a + b * b))) * (float)(a * Math.Sin(v) - b * Math.Cos(v));
        }

        private static float IntElastic(float x)
        {
            return IntElasticAntideriv(x) - IntElasticAntideriv(0f);
        }

        private static float IntBounce(float x)
        {
            return x - BounceH(1f) + BounceH(1f - x);
        }

        private static float BounceH(float u)
        {
            const float N1 = 7.5625f;
            const float D1 = 2.75f;

            float end1 = 1f / D1;
            float val1 = N1 / 3f * (end1 * end1 * end1);

            float end2 = 2f / D1;
            float c2 = val1 - (N1 / 3f * ((end1 - 1.5f / D1) * (end1 - 1.5f / D1) * (end1 - 1.5f / D1)) + 0.75f * end1);
            float val2 = (N1 / 3f * ((end2 - 1.5f / D1) * (end2 - 1.5f / D1) * (end2 - 1.5f / D1)) + 0.75f * end2) + c2;

            float end3 = 2.5f / D1;
            float c3 = val2 - (N1 / 3f * ((end2 - 2.25f / D1) * (end2 - 2.25f / D1) * (end2 - 2.25f / D1)) + 0.9375f * end2);
            float val3 = (N1 / 3f * ((end3 - 2.25f / D1) * (end3 - 2.25f / D1) * (end3 - 2.25f / D1)) + 0.9375f * end3) + c3;

            float c4 = val3 - (N1 / 3f * ((end3 - 2.625f / D1) * (end3 - 2.625f / D1) * (end3 - 2.625f / D1)) + 0.984375f * end3);

            if (u < end1) return N1 / 3f * (u * u * u);
            if (u < end2) return N1 / 3f * ((u - 1.5f / D1) * (u - 1.5f / D1) * (u - 1.5f / D1)) + 0.75f * u + c2;
            if (u < end3) return N1 / 3f * ((u - 2.25f / D1) * (u - 2.25f / D1) * (u - 2.25f / D1)) + 0.9375f * u + c3;
            return N1 / 3f * ((u - 2.625f / D1) * (u - 2.625f / D1) * (u - 2.625f / D1)) + 0.984375f * u + c4;
        }

        private static float Out(Func<float, float> f, float x) { return 1f - f(1f - x); }

        private static float InOut(Func<float, float> f, float x)
        {
            float x2 = x * 2f;
            if (x2 < 1f) return f(x2) / 2f;
            return 1f - f(2f - x2) / 2f;
        }

        private static float IOut(Func<float, float> f, float x) { return x + f(1f - x) - f(1f); }

        private static float IInOut(Func<float, float> f, float x)
        {
            float x2 = x * 2f;
            if (x2 < 1f) return f(x2) / 4f;
            return x - 0.5f + f(2f - x2) / 4f;
        }

        public static readonly Func<float, float>[] Functions = Build();
        public static readonly Func<float, float>[] IntFunctions = BuildInt();

        private static Func<float, float>[] Build()
        {
            return new Func<float, float>[]
            {
                delegate(float x) { return 0f; },
                delegate(float x) { return 1f; },
                delegate(float x) { return x; },
                // sine
                delegate(float x) { return Sine(x); },
                delegate(float x) { return Out(Sine, x); },
                delegate(float x) { return InOut(Sine, x); },
                // quad
                delegate(float x) { return Quad(x); },
                delegate(float x) { return Out(Quad, x); },
                delegate(float x) { return InOut(Quad, x); },
                // cubic
                delegate(float x) { return Cubic(x); },
                delegate(float x) { return Out(Cubic, x); },
                delegate(float x) { return InOut(Cubic, x); },
                // quart
                delegate(float x) { return Quart(x); },
                delegate(float x) { return Out(Quart, x); },
                delegate(float x) { return InOut(Quart, x); },
                // quint
                delegate(float x) { return Quint(x); },
                delegate(float x) { return Out(Quint, x); },
                delegate(float x) { return InOut(Quint, x); },
                // expo
                delegate(float x) { return Expo(x); },
                delegate(float x) { return Out(Expo, x); },
                delegate(float x) { return InOut(Expo, x); },
                // circ
                delegate(float x) { return Circ(x); },
                delegate(float x) { return Out(Circ, x); },
                delegate(float x) { return InOut(Circ, x); },
                // back
                delegate(float x) { return Back(x); },
                delegate(float x) { return Out(Back, x); },
                delegate(float x) { return InOut(Back, x); },
                // elastic
                delegate(float x) { return Elastic(x); },
                delegate(float x) { return Out(Elastic, x); },
                delegate(float x) { return InOut(Elastic, x); },
                // bounce
                delegate(float x) { return Bounce(x); },
                delegate(float x) { return Out(Bounce, x); },
                delegate(float x) { return InOut(Bounce, x); }
            };
        }

        private static Func<float, float>[] BuildInt()
        {
            return new Func<float, float>[]
            {
                delegate(float x) { return 0f; },
                delegate(float x) { return x; },
                delegate(float x) { return x * x / 2f; },
                delegate(float x) { return IntSine(x); },
                delegate(float x) { return IOut(IntSine, x); },
                delegate(float x) { return IInOut(IntSine, x); },
                delegate(float x) { return IntQuad(x); },
                delegate(float x) { return IOut(IntQuad, x); },
                delegate(float x) { return IInOut(IntQuad, x); },
                delegate(float x) { return IntCubic(x); },
                delegate(float x) { return IOut(IntCubic, x); },
                delegate(float x) { return IInOut(IntCubic, x); },
                delegate(float x) { return IntQuart(x); },
                delegate(float x) { return IOut(IntQuart, x); },
                delegate(float x) { return IInOut(IntQuart, x); },
                delegate(float x) { return IntQuint(x); },
                delegate(float x) { return IOut(IntQuint, x); },
                delegate(float x) { return IInOut(IntQuint, x); },
                delegate(float x) { return IntExpo(x); },
                delegate(float x) { return IOut(IntExpo, x); },
                delegate(float x) { return IInOut(IntExpo, x); },
                delegate(float x) { return IntCirc(x); },
                delegate(float x) { return IOut(IntCirc, x); },
                delegate(float x) { return IInOut(IntCirc, x); },
                delegate(float x) { return IntBack(x); },
                delegate(float x) { return IOut(IntBack, x); },
                delegate(float x) { return IInOut(IntBack, x); },
                delegate(float x) { return IntElastic(x); },
                delegate(float x) { return IOut(IntElastic, x); },
                delegate(float x) { return IInOut(IntElastic, x); },
                delegate(float x) { return IntBounce(x); },
                delegate(float x) { return IOut(IntBounce, x); },
                delegate(float x) { return IInOut(IntBounce, x); }
            };
        }

        /// <summary>prpr's `easing_from(major, minor) = major * 3 + minor`.</summary>
        public static byte From(int major, int minor)
        {
            return (byte)(major * 3 + minor);
        }
    }

    /// <summary>Port of prpr's RPE_TWEEN_MAP (30 entries, index 0 = linear).</summary>
    public static class RpeTweenMap
    {
        public static readonly byte[] Map = new byte[]
        {
            2, 2,                                       // 0, 1  linear
            Easing.From(1, 1), Easing.From(1, 0),       // 2, 3  sine out / in
            Easing.From(2, 1), Easing.From(2, 0),       // 4, 5  quad out / in
            Easing.From(1, 2), Easing.From(2, 2),       // 6, 7  sine inout / quad inout
            Easing.From(3, 1), Easing.From(3, 0),       // 8, 9  cubic out / in
            Easing.From(4, 1), Easing.From(4, 0),       // 10, 11 quart out / in
            Easing.From(3, 2), Easing.From(4, 2),       // 12, 13 cubic inout / quart inout
            Easing.From(5, 1), Easing.From(5, 0),       // 14, 15 quint out / in
            Easing.From(6, 1), Easing.From(6, 0),       // 16, 17 expo out / in
            Easing.From(7, 1), Easing.From(7, 0),       // 18, 19 circ out / in
            Easing.From(8, 1), Easing.From(8, 0),       // 20, 21 back out / in
            Easing.From(7, 2), Easing.From(8, 2),       // 22, 23 circ inout / back inout
            Easing.From(9, 1), Easing.From(9, 0),       // 24, 25 elastic out / in
            Easing.From(10, 1), Easing.From(10, 0),     // 26, 27 bounce out / in
            Easing.From(10, 2), Easing.From(9, 2)       // 28, 29 bounce inout / elastic inout
        };

        public static byte Get(int index)
        {
            if (index < 0 || index >= Map.Length)
            {
                return Map[0];
            }
            return Map[index];
        }
    }

    public sealed class StaticTween : ITweenFunction
    {
        public readonly byte Id;

        public StaticTween(byte id)
        {
            Id = id;
        }

        public float Y(float x)
        {
            return Easing.Functions[Id](x);
        }

        public TweenKind Kind
        {
            get { return TweenKind.Static; }
        }
    }

    public sealed class IntStaticTween : ITweenFunction
    {
        public readonly byte Id;

        public IntStaticTween(byte id)
        {
            Id = id;
        }

        public float Y(float x)
        {
            return Easing.IntFunctions[Id](x);
        }

        public TweenKind Kind
        {
            get { return TweenKind.IntStatic; }
        }
    }

    public sealed class ClampedTween : ITweenFunction
    {
        public readonly byte Id;
        public readonly float Lo;
        public readonly float Hi;
        private readonly float _ylo, _yhi;

        public ClampedTween(byte tween, float lo, float hi)
        {
            Id = tween;
            Lo = lo;
            Hi = hi;
            Func<float, float> f = Easing.Functions[tween];
            _ylo = f(lo);
            _yhi = f(hi);
        }

        public float Y(float x)
        {
            float y = Easing.Functions[Id](TweenMath.Lerp(Lo, Hi, x));
            return (y - _ylo) / (_yhi - _ylo);
        }

        public TweenKind Kind
        {
            get { return TweenKind.Clamped; }
        }
    }

    public sealed class IntClampedTween : ITweenFunction
    {
        public readonly byte Id;
        private readonly float _xlo, _xhi;
        private readonly float _ylo, _yhi;
        private readonly float _base;

        public IntClampedTween(byte tween, float xlo, float xhi)
        {
            Id = tween;
            _xlo = xlo;
            _xhi = xhi;
            Func<float, float> f = Easing.Functions[tween];
            _ylo = f(xlo);
            _yhi = f(xhi);
            _base = Easing.IntFunctions[tween](xlo);
        }

        public float Y(float x)
        {
            float denom = _yhi - _ylo;
            if (float.IsNaN(denom) || float.IsInfinity(denom) || Math.Abs(denom) < 1e-8f)
            {
                return x * x / 2f;
            }
            float xx = TweenMath.Lerp(_xlo, _xhi, x);
            float integral = Easing.IntFunctions[Id](xx) - _base - _ylo * (xx - _xlo);
            float scale = (_xhi - _xlo) * denom;
            return integral / scale;
        }

        public TweenKind Kind
        {
            get { return TweenKind.IntClamped; }
        }
    }

    /// <summary>Numerically integrates an arbitrary tween (Gauss-Legendre, 3 nodes).</summary>
    public sealed class GeneralIntTween : ITweenFunction
    {
        private readonly ITweenFunction _inner;

        public GeneralIntTween(ITweenFunction inner)
        {
            _inner = inner;
        }

        public float Y(float x)
        {
            const float sqrt06 = 0.7745967f;
            float[] nodes = new float[] { -sqrt06, 0f, sqrt06 };
            float[] weights = new float[] { 5f / 9f, 8f / 9f, 5f / 9f };
            float radius = x / 2f;
            float sum = 0f;
            for (int i = 0; i < 3; i++)
            {
                float sampleX = radius * (nodes[i] + 1f);
                sum += weights[i] * _inner.Y(sampleX);
            }
            return radius * sum;
        }

        public TweenKind Kind
        {
            get { return TweenKind.GeneralInt; }
        }
    }

    /// <summary>Port of prpr's SpeedIntegralTween (RPE speed events with easingType &gt;= 2).</summary>
    public sealed class SpeedIntegralTween : ITweenFunction
    {
        private readonly ITweenFunction _tween;
        private readonly float _k;
        private readonly float _b;
        private readonly float _total;

        private SpeedIntegralTween(ITweenFunction tween, float k, float b, float total)
        {
            _tween = tween;
            _k = k;
            _b = b;
            _total = total;
        }

        private float Partial(float x)
        {
            return _tween.Y(x) * _k + _b * x;
        }

        public static bool TryCreate(ITweenFunction tween, float k, float b, out ITweenFunction result, out float total)
        {
            SpeedIntegralTween t = new SpeedIntegralTween(tween, k, b, 0f);
            total = t.Partial(1f);
            if (float.IsNaN(total) || float.IsInfinity(total) || Math.Abs(total) < 1e-5f)
            {
                result = null;
                return false;
            }
            result = new SpeedIntegralTween(tween, k, b, total);
            return true;
        }

        public float Y(float x)
        {
            if (x <= 0f) return 0f;
            if (x >= 1f) return 1f;
            float y = Partial(x) / _total;
            return float.IsNaN(y) || float.IsInfinity(y) ? x : y;
        }

        public TweenKind Kind
        {
            get { return TweenKind.SpeedIntegral; }
        }
    }

    /// <summary>Port of prpr's BezierTween (https://github.com/gre/bezier-easing).</summary>
    public sealed class BezierTween : ITweenFunction
    {
        private const int SampleTableSize = 21;
        private const float SampleStep = 1f / (SampleTableSize - 1);
        private const float NewtonMinStep = 1e-3f;
        private const int NewtonIterations = 4;
        private const float SubdivisionPrecision = 1e-7f;
        private const int SubdivisionMaxIteration = 10;
        private const float SlopeEps = 1e-7f;

        private readonly float[] _sampleTable = new float[SampleTableSize];
        public readonly float P1X, P1Y, P2X, P2Y;

        public BezierTween(float p1x, float p1y, float p2x, float p2y)
        {
            P1X = p1x; P1Y = p1y; P2X = p2x; P2Y = p2y;
            for (int i = 0; i < SampleTableSize; i++)
            {
                _sampleTable[i] = Sample(p1x, p2x, i * SampleStep);
            }
        }

        private static void Coefficients(float x1, float x2, out float a, out float b, out float c)
        {
            a = (x1 - x2) * 3f + 1f;
            b = x2 * 3f - x1 * 6f;
            c = x1 * 3f;
        }

        private static float Sample(float x1, float x2, float t)
        {
            float a, b, c;
            Coefficients(x1, x2, out a, out b, out c);
            return ((a * t + b) * t + c) * t;
        }

        private static float Slope(float x1, float x2, float t)
        {
            float a, b, c;
            Coefficients(x1, x2, out a, out b, out c);
            return (a * 3f * t + b * 2f) * t + c;
        }

        private static float NewtonRaphsonIterate(float x, float t, float x1, float x2)
        {
            for (int i = 0; i < NewtonIterations; i++)
            {
                float slope = Slope(x1, x2, t);
                if (slope <= SlopeEps)
                {
                    return t;
                }
                float diff = Sample(x1, x2, t) - x;
                t -= diff / slope;
            }
            return t;
        }

        private static float BinarySubdivide(float x, float l, float r, float x1, float x2)
        {
            float t = (l + r) / 2f;
            for (int i = 0; i < SubdivisionMaxIteration; i++)
            {
                float diff = Sample(x1, x2, t) - x;
                if (Math.Abs(diff) <= SubdivisionPrecision)
                {
                    break;
                }
                if (diff > 0f) r = t; else l = t;
                t = (l + r) / 2f;
            }
            return t;
        }

        private float TForX(float x)
        {
            if (x == 0f || x == 1f)
            {
                return x;
            }
            int id = (int)(x / SampleStep);
            if (id > SampleTableSize - 2) id = SampleTableSize - 2;
            if (id < 0) id = 0;
            float dist = (x - _sampleTable[id]) / (_sampleTable[id + 1] - _sampleTable[id]);
            float initT = SampleStep * (id + dist);
            float slope = Slope(P1X, P2X, initT);
            if (slope <= SlopeEps)
            {
                return initT;
            }
            if (slope >= NewtonMinStep)
            {
                return NewtonRaphsonIterate(x, initT, P1X, P2X);
            }
            return BinarySubdivide(x, SampleStep * id, SampleStep * (id + 1), P1X, P2X);
        }

        public float Y(float x)
        {
            return Sample(P1Y, P2Y, TForX(x));
        }

        public TweenKind Kind
        {
            get { return TweenKind.Bezier; }
        }
    }

    /// <summary>Cached tween instances, mirroring prpr's `StaticTween::get_rc`.</summary>
    public static class Tweens
    {
        private static readonly ITweenFunction[] Cache = BuildStatic();
        private static readonly ITweenFunction[] IntCache = BuildInt();

        private static ITweenFunction[] BuildStatic()
        {
            ITweenFunction[] a = new ITweenFunction[Easing.Functions.Length];
            for (int i = 0; i < a.Length; i++)
            {
                a[i] = new StaticTween((byte)i);
            }
            return a;
        }

        private static ITweenFunction[] BuildInt()
        {
            ITweenFunction[] a = new ITweenFunction[Easing.IntFunctions.Length];
            for (int i = 0; i < a.Length; i++)
            {
                a[i] = new IntStaticTween((byte)i);
            }
            return a;
        }

        public static ITweenFunction Static(int id)
        {
            if (id < 0 || id >= Cache.Length)
            {
                id = 0;
            }
            return Cache[id];
        }

        public static ITweenFunction IntStatic(int id)
        {
            if (id < 0 || id >= IntCache.Length)
            {
                id = 0;
            }
            return IntCache[id];
        }
    }
}
