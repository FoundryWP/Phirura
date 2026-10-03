using System;

namespace Phirura.Core
{
    /// <summary>
    /// Port of prpr's `Vector`/`Point` usage (nalgebra Vector2/Point2).
    /// Kept as our own struct so the shared project stays free of platform types.
    /// </summary>
    public struct Vec2
    {
        public float X;
        public float Y;

        public Vec2(float x, float y)
        {
            X = x;
            Y = y;
        }

        public static readonly Vec2 Zero = new Vec2(0f, 0f);

        public static Vec2 operator +(Vec2 a, Vec2 b)
        {
            return new Vec2(a.X + b.X, a.Y + b.Y);
        }

        public static Vec2 operator -(Vec2 a, Vec2 b)
        {
            return new Vec2(a.X - b.X, a.Y - b.Y);
        }

        public static Vec2 operator -(Vec2 a)
        {
            return new Vec2(-a.X, -a.Y);
        }

        public static Vec2 operator *(Vec2 a, float s)
        {
            return new Vec2(a.X * s, a.Y * s);
        }

        public static Vec2 operator *(float s, Vec2 a)
        {
            return new Vec2(a.X * s, a.Y * s);
        }

        public float Length
        {
            get { return (float)Math.Sqrt(X * X + Y * Y); }
        }

        public Vec2 Normalized
        {
            get
            {
                float l = Length;
                return l < 1e-12f ? Zero : new Vec2(X / l, Y / l);
            }
        }

        public float Dot(Vec2 b)
        {
            return X * b.X + Y * b.Y;
        }

        public override string ToString()
        {
            return "(" + X.ToString("0.###") + ", " + Y.ToString("0.###") + ")";
        }
    }

    /// <summary>Float RGBA colour; converted to Windows.UI.Color only at render time.</summary>
    public struct RgbaF
    {
        public float R;
        public float G;
        public float B;
        public float A;

        public RgbaF(float r, float g, float b, float a)
        {
            R = r;
            G = g;
            B = b;
            A = a;
        }

        public static readonly RgbaF White = new RgbaF(1f, 1f, 1f, 1f);
        public static readonly RgbaF Transparent = new RgbaF(0f, 0f, 0f, 0f);

        public static RgbaF FromBytes(byte r, byte g, byte b, byte a)
        {
            return new RgbaF(r / 255f, g / 255f, b / 255f, a / 255f);
        }

        public RgbaF WithAlpha(float a)
        {
            return new RgbaF(R, G, B, a);
        }

        public override string ToString()
        {
            return "rgba(" + R.ToString("0.##") + "," + G.ToString("0.##") + "," + B.ToString("0.##") + "," + A.ToString("0.##") + ")";
        }
    }

    /// <summary>
    /// Row-major 3x3 homogeneous matrix, mirroring nalgebra::Matrix3 usage in prpr
    /// (2D affine: rotation + non-uniform scale + translation).
    /// </summary>
    public struct Mat3
    {
        public float M11, M12, M13;
        public float M21, M22, M23;
        public float M31, M32, M33;

        public static Mat3 Identity()
        {
            Mat3 m = new Mat3();
            m.M11 = 1f; m.M22 = 1f; m.M33 = 1f;
            return m;
        }

        public static Mat3 Translation(float x, float y)
        {
            Mat3 m = Identity();
            m.M13 = x;
            m.M23 = y;
            return m;
        }

        public static Mat3 RotationDegrees(float deg)
        {
            double rad = deg * Math.PI / 180.0;
            float c = (float)Math.Cos(rad);
            float s = (float)Math.Sin(rad);
            Mat3 m = Identity();
            m.M11 = c; m.M12 = -s;
            m.M21 = s; m.M22 = c;
            return m;
        }

        public static Mat3 Scaling(float x, float y)
        {
            Mat3 m = Identity();
            m.M11 = x;
            m.M22 = y;
            return m;
        }

        public static Mat3 operator *(Mat3 a, Mat3 b)
        {
            Mat3 r = new Mat3();
            r.M11 = a.M11 * b.M11 + a.M12 * b.M21 + a.M13 * b.M31;
            r.M12 = a.M11 * b.M12 + a.M12 * b.M22 + a.M13 * b.M32;
            r.M13 = a.M11 * b.M13 + a.M12 * b.M23 + a.M13 * b.M33;
            r.M21 = a.M21 * b.M11 + a.M22 * b.M21 + a.M23 * b.M31;
            r.M22 = a.M21 * b.M12 + a.M22 * b.M22 + a.M23 * b.M32;
            r.M23 = a.M21 * b.M13 + a.M22 * b.M23 + a.M23 * b.M33;
            r.M31 = a.M31 * b.M11 + a.M32 * b.M21 + a.M33 * b.M31;
            r.M32 = a.M31 * b.M12 + a.M32 * b.M22 + a.M33 * b.M32;
            r.M33 = a.M31 * b.M13 + a.M32 * b.M23 + a.M33 * b.M33;
            return r;
        }

        /// <summary>M * T(t) — matches nalgebra's append_translation.</summary>
        public Mat3 AppendTranslation(Vec2 t)
        {
            return this * Translation(t.X, t.Y);
        }

        /// <summary>M * S(s) — matches nalgebra's append_nonuniform_scaling.</summary>
        public Mat3 AppendNonuniformScaling(Vec2 s)
        {
            return this * Scaling(s.X, s.Y);
        }

        public Vec2 TransformPoint(Vec2 p)
        {
            return new Vec2(
                M11 * p.X + M12 * p.Y + M13,
                M21 * p.X + M22 * p.Y + M23);
        }

        /// <summary>Rotation about an arbitrary point, as in Object::new_rotation_wrt_point.</summary>
        public static Mat3 RotationWrtPoint(float deg, Vec2 pt)
        {
            return Translation(pt.X, pt.Y) * RotationDegrees(deg) * Translation(-pt.X, -pt.Y);
        }

        public bool TryInverse(out Mat3 inv)
        {
            float a = M11, b = M12, c = M13;
            float d = M21, e = M22, f = M23;
            float g = M31, h = M32, i = M33;
            float A = e * i - f * h;
            float B = -(d * i - f * g);
            float C = d * h - e * g;
            float det = a * A + b * B + c * C;
            if (Math.Abs(det) < 1e-20f)
            {
                inv = Identity();
                return false;
            }
            float id = 1f / det;
            Mat3 r = new Mat3();
            r.M11 = A * id;
            r.M12 = -(b * i - c * h) * id;
            r.M13 = (b * f - c * e) * id;
            r.M21 = B * id;
            r.M22 = (a * i - c * g) * id;
            r.M23 = -(a * f - c * d) * id;
            r.M31 = C * id;
            r.M32 = -(a * h - b * g) * id;
            r.M33 = (a * e - b * d) * id;
            inv = r;
            return true;
        }
    }
}
