using System;
using System.Collections.Generic;

namespace Phirura.Core
{
    public struct Keyframe<T>
    {
        public double Time;
        public T Value;
        public ITweenFunction Tween;

        public Keyframe(double time, T value, ITweenFunction tween)
        {
            Time = time;
            Value = value;
            Tween = tween;
        }

        public Keyframe(double time, T value, int tweenId)
        {
            Time = time;
            Value = value;
            Tween = Tweens.Static(tweenId);
        }
    }

    /// <summary>
    /// Port of prpr::core::Anim. Keyframes are stored flat, and an optional `Next`
    /// chain adds another animation's value on top (see `Anim::chain`).
    /// </summary>
    public abstract class AnimBase<T>
    {
        public double Time;
        public Keyframe<T>[] Keyframes = new Keyframe<T>[0];
        public int Cursor;
        public AnimBase<T> Next;

        protected abstract T Lerp(T a, T b, float f);
        protected abstract T AddValues(T a, T b);

        public bool IsDefault()
        {
            return Keyframes.Length == 0 && Next == null;
        }

        public static AnimBase<T> Chain(List<AnimBase<T>> elements)
        {
            if (elements == null || elements.Count == 0)
            {
                return null;
            }
            for (int i = 0; i < elements.Count - 1; i++)
            {
                elements[i].Next = elements[i + 1];
            }
            elements[elements.Count - 1].Next = null;
            return elements[0];
        }

        public bool Dead()
        {
            return Cursor + 1 >= Keyframes.Length;
        }

        public void SetTime(double time)
        {
            if (Keyframes.Length == 0 || time == Time)
            {
                Time = time;
                // Upstream returns early here and (as a side effect) never advances a
                // chained `next`. That breaks any chain whose head layer happened to be
                // an explicitly-empty event array, so we always propagate.
                if (Next != null)
                {
                    Next.SetTime(time);
                }
                return;
            }
            while (Cursor + 1 < Keyframes.Length && Keyframes[Cursor + 1].Time <= time)
            {
                Cursor++;
            }
            while (Cursor != 0 && Keyframes[Cursor].Time > time)
            {
                Cursor--;
            }
            Time = time;
            if (Next != null)
            {
                Next.SetTime(time);
            }
        }

        private bool NowInner(out T value)
        {
            if (Keyframes.Length == 0)
            {
                value = default(T);
                return false;
            }
            if (Cursor == Keyframes.Length - 1)
            {
                value = Keyframes[Cursor].Value;
                return true;
            }
            Keyframe<T> kf1 = Keyframes[Cursor];
            Keyframe<T> kf2 = Keyframes[Cursor + 1];
            double span = kf2.Time - kf1.Time;
            float t = span == 0.0 ? 1f : (float)((Time - kf1.Time) / span);
            value = Lerp(kf1.Value, kf2.Value, kf1.Tween.Y(t));
            return true;
        }

        public bool NowOpt(out T value)
        {
            T now;
            if (!NowInner(out now))
            {
                value = default(T);
                return false;
            }
            if (Next != null)
            {
                T nv;
                if (Next.NowOpt(out nv))
                {
                    now = AddValues(now, nv);
                }
            }
            value = now;
            return true;
        }

        public void MapValue(Func<T, T> f)
        {
            for (int i = 0; i < Keyframes.Length; i++)
            {
                Keyframes[i].Value = f(Keyframes[i].Value);
            }
            if (Next != null)
            {
                Next.MapValue(f);
            }
        }
    }

    public sealed class AnimFloat : AnimBase<float>
    {
        public AnimFloat()
        {
        }

        public AnimFloat(Keyframe<float>[] keyframes)
        {
            Keyframes = keyframes;
        }

        public static AnimFloat New(List<Keyframe<float>> keyframes)
        {
            AnimFloat a = new AnimFloat();
            a.Keyframes = keyframes.ToArray();
            return a;
        }

        public static AnimFloat Fixed(float value)
        {
            return New(new List<Keyframe<float>> { new Keyframe<float>(0.0, value, 0) });
        }

        protected override float Lerp(float a, float b, float f)
        {
            return a + (b - a) * f;
        }

        protected override float AddValues(float a, float b)
        {
            return a + b;
        }

        public float Now()
        {
            float v;
            return NowOpt(out v) ? v : 0f;
        }

        /// <summary>Returns false when the animation has no keyframes (Rust's `now_opt` is None).</summary>
        public bool TryNow(out float value)
        {
            return NowOpt(out value);
        }
    }

    public sealed class AnimColor : AnimBase<RgbaF>
    {
        public AnimColor()
        {
        }

        public static AnimColor New(List<Keyframe<RgbaF>> keyframes)
        {
            AnimColor a = new AnimColor();
            a.Keyframes = keyframes.ToArray();
            return a;
        }

        public static AnimColor Fixed(RgbaF value)
        {
            return New(new List<Keyframe<RgbaF>> { new Keyframe<RgbaF>(0.0, value, 0) });
        }

        protected override RgbaF Lerp(RgbaF a, RgbaF b, float f)
        {
            return new RgbaF(
                a.R + (b.R - a.R) * f,
                a.G + (b.G - a.G) * f,
                a.B + (b.B - a.B) * f,
                a.A + (b.A - a.A) * f);
        }

        protected override RgbaF AddValues(RgbaF a, RgbaF b)
        {
            return new RgbaF(a.R + b.R, a.G + b.G, a.B + b.B, a.A + b.A);
        }

        public RgbaF Now()
        {
            RgbaF v;
            return NowOpt(out v) ? v : RgbaF.White;
        }
    }

    public sealed class AnimString : AnimBase<string>
    {
        public AnimString()
        {
        }

        public static AnimString New(List<Keyframe<string>> keyframes)
        {
            AnimString a = new AnimString();
            a.Keyframes = keyframes.ToArray();
            return a;
        }

        public static AnimString Fixed(string value)
        {
            return New(new List<Keyframe<string>> { new Keyframe<string>(0.0, value, 0) });
        }

        protected override string Lerp(string a, string b, float t)
        {
            a = a ?? string.Empty;
            b = b ?? string.Empty;
            if (a.Contains("%P%") && b.Contains("%P%"))
            {
                string xa = a.Replace("%P%", string.Empty);
                string xb = b.Replace("%P%", string.Empty);
                if (t >= 1f) return xb;
                if (t <= 0f) return xa;
                float x, y;
                float.TryParse(xa, out x);
                float.TryParse(xb, out y);
                float value = x + t * (y - x);
                bool ints = Math.Abs(x - Math.Floor(x)) < 1e-6 && Math.Abs(y - Math.Floor(y)) < 1e-6;
                return ints ? ((int)value).ToString() : value.ToString("0.000");
            }
            if (a.Length == 0 && b.Length == 0) return string.Empty;
            if (b.Length == 0)
            {
                string xa = a.Contains("%P%") ? a.Replace("%P%", string.Empty) : a;
                return Lerp(b, xa, 1f - t);
            }
            if (a.Length == 0)
            {
                int take = (int)Math.Round(t * b.Length);
                if (take < 0) take = 0;
                if (take > b.Length) take = b.Length;
                return b.Substring(0, take);
            }
            if (b.StartsWith(a))
            {
                int takeNum = (int)Math.Floor((b.Length - a.Length) * (double)t) + a.Length;
                if (takeNum < a.Length) takeNum = a.Length;
                if (takeNum > b.Length) takeNum = b.Length;
                return a + b.Substring(a.Length, takeNum - a.Length);
            }
            if (a.StartsWith(b))
            {
                int takeNum = (int)Math.Round((a.Length - b.Length) * (double)(1f - t)) + b.Length;
                if (takeNum < b.Length) takeNum = b.Length;
                if (takeNum > a.Length) takeNum = a.Length;
                return b + a.Substring(b.Length, takeNum - b.Length);
            }
            if (a.Contains("%P%")) return a.Replace("%P%", string.Empty);
            return a;
        }

        protected override string AddValues(string a, string b)
        {
            return a;
        }

        public string Now()
        {
            string v;
            return NowOpt(out v) ? v : string.Empty;
        }
    }

    /// <summary>Port of prpr::core::AnimVector (a pair of independent float animations).</summary>
    public sealed class AnimVector
    {
        public AnimFloat X;
        public AnimFloat Y;

        public AnimVector()
        {
            X = new AnimFloat();
            Y = new AnimFloat();
        }

        public AnimVector(AnimFloat x, AnimFloat y)
        {
            X = x;
            Y = y;
        }

        public static AnimVector Fixed(Vec2 v)
        {
            return new AnimVector(AnimFloat.Fixed(v.X), AnimFloat.Fixed(v.Y));
        }

        public void SetTime(double time)
        {
            X.SetTime(time);
            Y.SetTime(time);
        }

        public Vec2 Now()
        {
            return new Vec2(X.Now(), Y.Now());
        }

        public Vec2 NowWithDef(float dx, float dy)
        {
            float x, y;
            return new Vec2(X.TryNow(out x) ? x : dx, Y.TryNow(out y) ? y : dy);
        }
    }
}
