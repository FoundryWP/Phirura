using System;
using System.Globalization;
using Windows.Data.Json;

namespace Phirura.Parse
{
    /// <summary>
    /// Defensive wrappers over Windows.Data.Json.
    ///
    /// Community RPE charts are hand-edited and routinely store numbers as strings,
    /// omit optional fields, or emit `null` where an object is expected. Upstream
    /// relies on serde's leniency, so every accessor here has a default and never throws.
    /// </summary>
    public static class JsonExt
    {
        public static bool Has(JsonObject o, string key)
        {
            return o != null && o.ContainsKey(key);
        }

        public static IJsonValue Get(JsonObject o, string key)
        {
            if (o == null || !o.ContainsKey(key))
            {
                return null;
            }
            return o.GetNamedValue(key);
        }

        public static double AsDouble(IJsonValue v, double def)
        {
            if (v == null || v.ValueType == JsonValueType.Null)
            {
                return def;
            }
            if (v.ValueType == JsonValueType.Number)
            {
                double d = v.GetNumber();
                return double.IsNaN(d) || double.IsInfinity(d) ? def : d;
            }
            if (v.ValueType == JsonValueType.String)
            {
                double d;
                if (double.TryParse(v.GetString(), NumberStyles.Float, CultureInfo.InvariantCulture, out d))
                {
                    return double.IsNaN(d) || double.IsInfinity(d) ? def : d;
                }
            }
            if (v.ValueType == JsonValueType.Boolean)
            {
                return v.GetBoolean() ? 1.0 : 0.0;
            }
            return def;
        }

        public static double Num(JsonObject o, string key, double def)
        {
            return AsDouble(Get(o, key), def);
        }

        public static int Int(JsonObject o, string key, int def)
        {
            double d = Num(o, key, def);
            if (double.IsNaN(d) || double.IsInfinity(d))
            {
                return def;
            }
            if (d > int.MaxValue) return int.MaxValue;
            if (d < int.MinValue) return int.MinValue;
            return (int)Math.Round(d, MidpointRounding.AwayFromZero);
        }

        public static float Float(JsonObject o, string key, float def)
        {
            return (float)Num(o, key, def);
        }

        public static bool Bool(JsonObject o, string key, bool def)
        {
            IJsonValue v = Get(o, key);
            if (v == null || v.ValueType == JsonValueType.Null)
            {
                return def;
            }
            if (v.ValueType == JsonValueType.Boolean)
            {
                return v.GetBoolean();
            }
            if (v.ValueType == JsonValueType.Number)
            {
                return Math.Abs(v.GetNumber()) > 1e-9;
            }
            return def;
        }

        public static string Str(JsonObject o, string key, string def)
        {
            IJsonValue v = Get(o, key);
            if (v == null || v.ValueType == JsonValueType.Null)
            {
                return def;
            }
            if (v.ValueType == JsonValueType.String)
            {
                return v.GetString();
            }
            if (v.ValueType == JsonValueType.Number)
            {
                return v.GetNumber().ToString(CultureInfo.InvariantCulture);
            }
            return def;
        }

        public static JsonArray Arr(JsonObject o, string key)
        {
            IJsonValue v = Get(o, key);
            if (v == null || v.ValueType != JsonValueType.Array)
            {
                return null;
            }
            return v.GetArray();
        }

        public static JsonObject Obj(JsonObject o, string key)
        {
            IJsonValue v = Get(o, key);
            if (v == null || v.ValueType != JsonValueType.Object)
            {
                return null;
            }
            return v.GetObject();
        }

        public static JsonObject ObjAt(JsonArray a, int i)
        {
            if (a == null || i < 0 || i >= a.Count)
            {
                return null;
            }
            IJsonValue v = a[i];
            return v != null && v.ValueType == JsonValueType.Object ? v.GetObject() : null;
        }

        /// <summary>Parses an RPE beat triple `[i, n, d]`.</summary>
        public static Core.Triple TripleAt(IJsonValue v)
        {
            if (v == null || v.ValueType != JsonValueType.Array)
            {
                return Core.Triple.Zero;
            }
            JsonArray a = v.GetArray();
            if (a.Count >= 3)
            {
                int i = (int)AsDouble(a[0], 0);
                int n = (int)AsDouble(a[1], 0);
                int d = (int)AsDouble(a[2], 1);
                return new Core.Triple(i, n, d);
            }
            if (a.Count == 1)
            {
                return new Core.Triple((int)AsDouble(a[0], 0), 0, 1);
            }
            return Core.Triple.Zero;
        }

        public static Core.Triple Triple(JsonObject o, string key)
        {
            return TripleAt(Get(o, key));
        }
    }
}
