using System;
using System.Collections.Generic;

namespace Phirura.Core
{
    /// <summary>
    /// Port of prpr::core::Triple — an RPE beat time written as `[i, n, d]` meaning `i + n/d` beats.
    /// </summary>
    public struct Triple
    {
        public int I;
        public int N;
        public int D;

        public Triple(int i, int n, int d)
        {
            I = i;
            N = n;
            D = d == 0 ? 1 : d;
        }

        public static readonly Triple Zero = new Triple(0, 0, 1);

        public double Beats
        {
            get { return I + (double)N / D; }
        }

        public override string ToString()
        {
            return "[" + I + "," + N + "," + D + "]";
        }
    }

    /// <summary>
    /// Port of prpr::core::BpmList.
    ///
    /// Upstream keeps a mutable search cursor; because that cursor is shared between
    /// `time_beats` and `beat` (which are called with interleaved, non-monotonic
    /// arguments during parsing) we instead resolve the segment with a plain scan.
    /// BPM lists have a handful of entries, so this is free and removes a whole class
    /// of ordering bugs. The result is identical for sorted input.
    /// </summary>
    public sealed class BpmList
    {
        private struct Element
        {
            public double Beats;
            public double Time;
            public double Bpm;
        }

        private readonly List<Element> _elements = new List<Element>();

        public BpmList()
        {
        }

        /// <summary>Build from a list of (beats, bpm) pairs.</summary>
        public BpmList(List<KeyValuePair<double, double>> ranges)
        {
            double time = 0.0;
            double lastBeats = 0.0;
            bool hasLast = false;
            double lastBpm = 0.0;
            for (int i = 0; i < ranges.Count; i++)
            {
                double nowBeats = ranges[i].Key;
                double bpm = ranges[i].Value;
                if (hasLast)
                {
                    time += (nowBeats - lastBeats) * (60.0 / lastBpm);
                }
                lastBeats = nowBeats;
                lastBpm = bpm;
                hasLast = true;
                Element e = new Element();
                e.Beats = nowBeats;
                e.Time = time;
                e.Bpm = bpm;
                _elements.Add(e);
            }
        }

        public int Count
        {
            get { return _elements.Count; }
        }

        private int IndexOfBeats(double beats)
        {
            int idx = 0;
            for (int i = 0; i < _elements.Count; i++)
            {
                if (_elements[i].Beats <= beats)
                {
                    idx = i;
                }
                else
                {
                    break;
                }
            }
            return idx;
        }

        private int IndexOfTime(double time)
        {
            int idx = 0;
            for (int i = 0; i < _elements.Count; i++)
            {
                if (_elements[i].Time <= time)
                {
                    idx = i;
                }
                else
                {
                    break;
                }
            }
            return idx;
        }

        /// <summary>Time in seconds at a given beat.</summary>
        public double TimeBeats(double beats)
        {
            if (_elements.Count == 0)
            {
                return 0.0;
            }
            Element e = _elements[IndexOfBeats(beats)];
            return e.Time + (beats - e.Beats) * (60.0 / e.Bpm);
        }

        public double Time(Triple t)
        {
            return TimeBeats(t.Beats);
        }

        /// <summary>Beat coordinate at a given time in seconds.</summary>
        public double Beat(double time)
        {
            if (_elements.Count == 0)
            {
                return 0.0;
            }
            Element e = _elements[IndexOfTime(time)];
            return e.Beats + (time - e.Time) / (60.0 / e.Bpm);
        }

        /// <summary>Lowest BPM in the list — used for autoplay / chart-info display.</summary>
        public double MinBpm()
        {
            double min = double.MaxValue;
            for (int i = 0; i < _elements.Count; i++)
            {
                if (_elements[i].Bpm < min) min = _elements[i].Bpm;
            }
            return min == double.MaxValue ? 0.0 : min;
        }

        public double MaxBpm()
        {
            double max = 0.0;
            for (int i = 0; i < _elements.Count; i++)
            {
                if (_elements[i].Bpm > max) max = _elements[i].Bpm;
            }
            return max;
        }
    }
}
