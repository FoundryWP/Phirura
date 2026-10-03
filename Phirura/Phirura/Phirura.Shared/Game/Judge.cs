using System;
using System.Collections.Generic;
using Phirura.Core;

namespace Phirura.Game
{
    public enum Judgement
    {
        Perfect = 0,
        Good = 1,
        Bad = 2,
        Miss = 3
    }

    public enum TouchPhase
    {
        Started,
        Moved,
        Stationary,
        Ended,
        Cancelled
    }

    public struct TouchInput
    {
        public ulong Id;
        public TouchPhase Phase;

        /// <summary>Touch position in chart space (x in [-1,1], y up, already divided by the aspect ratio).</summary>
        public Vec2 World;

        /// <summary>Touch position in [-1,1]^2 (y down) — the space upstream's FlickTracker works in.</summary>
        public Vec2 Ndc;

        /// <summary>Chart time of the press, or double.NegativeInfinity when it is "now".</summary>
        public double Time;
    }

    public sealed class PlayResult
    {
        public uint Score;
        public double Accuracy;
        public uint MaxCombo;
        public uint NumOfNotes;
        public uint[] Counts = new uint[4];
        public uint Early;
        public uint Late;
        public uint[] EarlyKind = new uint[4];
        public uint[] LateKind = new uint[4];

        public bool FullCombo
        {
            get { return Counts[2] == 0 && Counts[3] == 0; }
        }
    }

    /// <summary>A note hit too early/late — drawn briefly as a grey ghost.</summary>
    public sealed class BadNote
    {
        public double Time;
        public NoteKind Kind;
        public Mat3 Matrix;
    }

    /// <summary>Port of prpr's FlickTracker.</summary>
    internal sealed class FlickTracker
    {
        public const float FlickSpeedThreshold = 0.8f;

        private readonly float _threshold;
        private Vec2 _lastPoint;
        private bool _hasLastDelta;
        private Vec2 _lastDelta;
        private float _lastTime;
        public bool Flicked;
        private bool _stopped;

        public FlickTracker(float time, Vec2 point)
        {
            // Upstream hard-codes dpi = 275 here.
            const float dpi = 275f;
            _threshold = FlickSpeedThreshold * dpi / 386f;
            _lastPoint = point;
            _lastTime = time;
            _stopped = true;
            Flicked = false;
        }

        public void Push(float time, Vec2 position)
        {
            Vec2 delta = position - _lastPoint;
            _lastPoint = position;
            if (_hasLastDelta)
            {
                float dt = time - _lastTime;
                if (dt > 1e-6f)
                {
                    float speed = delta.Dot(_lastDelta) / dt;
                    if (speed < _threshold)
                    {
                        _stopped = true;
                    }
                    if (_stopped && !Flicked)
                    {
                        Flicked = delta.Length / dt >= _threshold * 2f;
                    }
                }
            }
            _lastDelta = delta.Normalized;
            _hasLastDelta = true;
            _lastTime = time;
        }
    }

    /// <summary>
    /// Port of prpr/src/judge.rs — judgement windows, holds, flicks, drags,
    /// and the scoring / accuracy model.
    /// </summary>
    public sealed class Judge
    {
        public const double LimitPerfect = 0.08;
        public const double LimitGood = 0.16;
        public const double LimitBad = 0.22;
        public const double UpTolerance = 0.05;
        public const double DistFactor = 0.2;
        public const double EarlyOffset = 0.07;

        private const double XDiffMax = 0.21 / (16.0 / 9.0) * 2.0;

        public struct JudgementEvent
        {
            public double Time;
            public Judgement What;
            public int LineId;
            public int NoteId;
            /// <summary>Offset in seconds (negative = early).</summary>
            public double Offset;
        }

        private sealed class LineCursor
        {
            public List<int> Indices = new List<int>();
            public int St;
        }

        private struct Pending
        {
            public Judgement What;
            public int LineId;
            public int NoteId;
            /// <summary>Press time used to compute the offset; NaN means "use the current time".</summary>
            public double PressTime;
            public bool IsBad;
        }

        private readonly List<double> _diffs = new List<double>();
        private uint _combo;
        private uint _maxCombo;
        private readonly uint[] _counts = new uint[4];
        private readonly uint[] _earlyKind = new uint[4];
        private readonly uint[] _lateKind = new uint[4];
        private readonly uint _numOfNotes;

        private readonly List<LineCursor> _cursors = new List<LineCursor>();
        private readonly Dictionary<ulong, FlickTracker> _trackers = new Dictionary<ulong, FlickTracker>();
        private double _lastTime;
        private float _aspectRatio = 16f / 9f;

        public Judge(Chart chart)
        {
            uint total = 0;
            for (int i = 0; i < chart.Lines.Count; i++)
            {
                JudgeLine line = chart.Lines[i];
                LineCursor c = new LineCursor();
                for (int j = 0; j < line.Notes.Count; j++)
                {
                    if (!line.Notes[j].Fake)
                    {
                        c.Indices.Add(j);
                        total++;
                    }
                }
                c.Indices.Sort(delegate(int a, int b)
                {
                    return line.Notes[a].Time.NotNan().CompareTo(line.Notes[b].Time.NotNan());
                });
                _cursors.Add(c);
            }
            _numOfNotes = total;
        }

        public float AspectRatio
        {
            set { _aspectRatio = value <= 0f ? 16f / 9f : value; }
        }

        public void Reset()
        {
            for (int i = 0; i < _cursors.Count; i++)
            {
                _cursors[i].St = 0;
            }
            _trackers.Clear();
            _combo = 0;
            _maxCombo = 0;
            for (int i = 0; i < 4; i++)
            {
                _counts[i] = 0;
                _earlyKind[i] = 0;
                _lateKind[i] = 0;
            }
            _diffs.Clear();
            _lastTime = 0.0;
        }

        public uint Combo { get { return _combo; } }
        public uint[] Counts { get { return _counts; } }
        public uint NumOfNotes { get { return _numOfNotes; } }

        public double Accuracy
        {
            get
            {
                if (_numOfNotes == 0) return 1.0;
                return (_counts[0] + _counts[1] * 0.65) / _numOfNotes;
            }
        }

        public double RealTimeAccuracy
        {
            get
            {
                uint cnt = _counts[0] + _counts[1] + _counts[2] + _counts[3];
                if (cnt == 0) return 1.0;
                return (_counts[0] + _counts[1] * 0.65) / cnt;
            }
        }

        public uint Score
        {
            get
            {
                const uint total = 1000000;
                if (_numOfNotes > 0 && _counts[0] == _numOfNotes)
                {
                    return total;
                }
                double score = (0.9 * Accuracy + (double)_maxCombo / _numOfNotes * 0.1) * total;
                return (uint)Math.Round(score);
            }
        }

        public PlayResult Result()
        {
            uint early = 0;
            for (int i = 0; i < _diffs.Count; i++)
            {
                if (_diffs[i] < 0) early++;
            }
            PlayResult r = new PlayResult();
            r.Score = Score;
            r.Accuracy = Accuracy;
            r.MaxCombo = _maxCombo;
            r.NumOfNotes = _numOfNotes;
            Array.Copy(_counts, r.Counts, 4);
            r.Early = early;
            r.Late = (uint)_diffs.Count - early;
            Array.Copy(_earlyKind, r.EarlyKind, 4);
            Array.Copy(_lateKind, r.LateKind, 4);
            return r;
        }

        private void Commit(double t, Judgement what, double diff, List<JudgementEvent> sink, int lineId, int noteId)
        {
            if (sink != null)
            {
                JudgementEvent ev = new JudgementEvent();
                ev.Time = t;
                ev.What = what;
                ev.LineId = lineId;
                ev.NoteId = noteId;
                ev.Offset = diff;
                sink.Add(ev);
            }
            if (what == Judgement.Good)
            {
                _diffs.Add(diff);
            }
            int idx = (int)what;
            if (diff < 0) _earlyKind[idx]++;
            else if (diff > 0) _lateKind[idx]++;
            _counts[idx]++;
            if (what == Judgement.Perfect || what == Judgement.Good)
            {
                _combo++;
                if (_combo > _maxCombo) _maxCombo = _combo;
            }
            else
            {
                _combo = 0;
            }
        }

        // ------------------------------------------------------------------

        public void Update(Chart chart, double t, double spd, List<TouchInput> touches,
                           List<BadNote> badNotes, bool autoplay, List<JudgementEvent> sink)
        {
            if (autoplay)
            {
                AutoPlayUpdate(chart, t, spd, sink);
                return;
            }
            if (spd <= 0) spd = 1.0;

            int lineCount = chart.Lines.Count;
            List<TouchInput> list = touches ?? new List<TouchInput>();

            // 1. Flick trackers.
            double delta = (t / spd - _lastTime) / (list.Count + 1);
            double tt = _lastTime;
            for (int i = 0; i < list.Count; i++)
            {
                TouchInput touch = list[i];
                tt += delta;
                float ft = (float)tt;
                switch (touch.Phase)
                {
                    case TouchPhase.Started:
                        _trackers[touch.Id] = new FlickTracker(ft, touch.Ndc);
                        break;
                    case TouchPhase.Moved:
                    case TouchPhase.Stationary:
                        {
                            FlickTracker tr;
                            if (_trackers.TryGetValue(touch.Id, out tr))
                            {
                                tr.Push(ft, touch.Ndc);
                            }
                            break;
                        }
                    default:
                        _trackers.Remove(touch.Id);
                        break;
                }
            }

            // 2. Touch position inside each line's local frame (only x is used for judgement).
            Vec2[][] pos = new Vec2[lineCount][];
            for (int id = 0; id < lineCount; id++)
            {
                JudgeLine line = chart.Lines[id];
                line.Obj.SetTime(t);
                Mat3 inv;
                line.NowTransform(_aspectRatio, chart.Lines).TryInverse(out inv);
                Vec2[] arr = new Vec2[list.Count];
                for (int k = 0; k < list.Count; k++)
                {
                    arr[k] = inv.TransformPoint(list[k].World);
                }
                pos[id] = arr;
            }

            double[] touchTime = new double[list.Count];
            for (int k = 0; k < list.Count; k++)
            {
                touchTime[k] = double.IsNegativeInfinity(list[k].Time) ? t : list[k].Time;
            }

            List<Pending> pending = new List<Pending>();

            // 3. Clicks and flicks.
            double initialLimit = LimitBad + Math.Max(0.0, XDiffMax / Consts.NoteWidthRatioBase - 1.0) * DistFactor;
            for (int k = 0; k < list.Count; k++)
            {
                TouchInput touch = list[k];
                bool click = touch.Phase == TouchPhase.Started;
                FlickTracker tracker;
                bool flick = (touch.Phase == TouchPhase.Moved || touch.Phase == TouchPhase.Stationary)
                             && _trackers.TryGetValue(touch.Id, out tracker) && tracker.Flicked;
                if (!click && !flick)
                {
                    continue;
                }

                double tt2 = touchTime[k];
                bool found = false;
                int bestLine = -1;
                int bestNote = -1;
                double bestDt = 0;
                double bestLimit = initialLimit;

                for (int lineId = 0; lineId < lineCount; lineId++)
                {
                    JudgeLine line = chart.Lines[lineId];
                    Vec2 local = pos[lineId][k];
                    LineCursor cur = _cursors[lineId];
                    for (int ci = cur.St; ci < cur.Indices.Count; ci++)
                    {
                        int id = cur.Indices[ci];
                        Note note = line.Notes[id];
                        if (note.Judge.Kind != JudgeStatusKind.NotJudged && note.Judge.Kind != JudgeStatusKind.PreJudge)
                        {
                            continue;
                        }
                        if (!click && (note.Kind.Type == NoteKindType.Click || note.Kind.Type == NoteKindType.Hold))
                        {
                            continue;
                        }
                        double raw = (note.Time - tt2) / spd;
                        if (raw >= bestLimit)
                        {
                            break;
                        }
                        double dt = raw < 0 ? Math.Abs(Math.Min(0.0, raw + EarlyOffset)) : raw;
                        note.Obj.Translation.X.SetTime(t);
                        double dist = Math.Abs(note.Obj.Translation.X.Now() - local.X) / note.JudgeArea;
                        if (dist > XDiffMax)
                        {
                            continue;
                        }
                        double limit = note.Kind.Type == NoteKindType.Click
                            ? LimitBad - LimitPerfect * Math.Max(0.0, dist - 0.9)
                            : LimitGood;
                        if (dt > limit)
                        {
                            continue;
                        }
                        double dt2 = (note.Kind.Type == NoteKindType.Flick || note.Kind.Type == NoteKindType.Drag) ? dt + LimitGood : dt;
                        double key = dt2 + Math.Max(0.0, dist / Consts.NoteWidthRatioBase - 1.0) * DistFactor;
                        if (key < bestLimit)
                        {
                            bestLimit = key;
                            bestLine = lineId;
                            bestNote = id;
                            bestDt = dt2;
                            found = true;
                        }
                    }
                }

                if (!found)
                {
                    continue;
                }

                JudgeLine hitLine = chart.Lines[bestLine];
                Note hitNote = hitLine.Notes[bestNote];
                if (hitNote.Kind.Type == NoteKindType.Drag)
                {
                    continue;
                }
                if (click)
                {
                    if (hitNote.Kind.Type == NoteKindType.Flick)
                    {
                        continue;
                    }
                    if (bestDt <= LimitGood || hitNote.Kind.Type == NoteKindType.Hold)
                    {
                        if (hitNote.Kind.Type == NoteKindType.Click)
                        {
                            hitNote.Judge = JudgeStatus.Judged;
                            Pending p = new Pending();
                            p.What = bestDt <= LimitPerfect ? Judgement.Perfect : Judgement.Good;
                            p.LineId = bestLine;
                            p.NoteId = bestNote;
                            p.PressTime = t;
                            pending.Add(p);
                        }
                        else
                        {
                            // Hold head: scoring happens when the hold completes (see step 5).
                            hitNote.Judge = JudgeStatus.MakeHold(bestDt <= LimitPerfect, t, t, false, double.PositiveInfinity);
                        }
                    }
                    else if (hitNote.Judge.Kind == JudgeStatusKind.NotJudged)
                    {
                        hitNote.Judge = JudgeStatus.PreJudgeStatus;
                        Pending p = new Pending();
                        p.What = Judgement.Bad;
                        p.LineId = bestLine;
                        p.NoteId = bestNote;
                        p.PressTime = t;
                        p.IsBad = true;
                        pending.Add(p);
                    }
                }
                else
                {
                    hitNote.Judge = JudgeStatus.PreJudgeStatus;
                    FlickTracker tr;
                    if (_trackers.TryGetValue(touch.Id, out tr))
                    {
                        tr.Flicked = false;
                    }
                }
            }

            // 4. Holds and misses.
            for (int lineId = 0; lineId < lineCount; lineId++)
            {
                JudgeLine line = chart.Lines[lineId];
                line.Obj.SetTime(t);
                LineCursor cur = _cursors[lineId];
                for (int ci = cur.St; ci < cur.Indices.Count; ci++)
                {
                    int id = cur.Indices[ci];
                    Note note = line.Notes[id];

                    if (note.Kind.Type == NoteKindType.Hold && note.Judge.Kind == JudgeStatusKind.Hold)
                    {
                        JudgeStatus st = note.Judge;
                        if ((note.Kind.EndTime - t) / spd <= LimitBad)
                        {
                            st.PreJudge = true;
                            note.Judge = st;
                            continue;
                        }
                        note.Obj.Translation.X.SetTime(t);
                        double x = note.Obj.Translation.X.Now();
                        bool held = false;
                        for (int k = 0; k < list.Count; k++)
                        {
                            if (Math.Abs(pos[lineId][k].X - x) / note.JudgeArea <= XDiffMax)
                            {
                                held = true;
                                break;
                            }
                        }
                        if (!held)
                        {
                            if (t > st.UpTime + UpTolerance)
                            {
                                note.Judge = JudgeStatus.Judged;
                                Pending mp = new Pending();
                                mp.What = Judgement.Miss;
                                mp.LineId = lineId;
                                mp.NoteId = id;
                                mp.PressTime = double.NaN;
                                pending.Add(mp);
                            }
                            else if (double.IsPositiveInfinity(st.UpTime))
                            {
                                st.UpTime = t;
                                note.Judge = st;
                            }
                        }
                        else
                        {
                            st.UpTime = double.PositiveInfinity;
                            note.Judge = st;
                        }
                        continue;
                    }

                    if (note.Judge.Kind != JudgeStatusKind.NotJudged)
                    {
                        continue;
                    }
                    double dt = (t - note.Time) / spd;
                    if (dt > LimitBad)
                    {
                        note.Judge = JudgeStatus.Judged;
                        Pending mp = new Pending();
                        mp.What = Judgement.Miss;
                        mp.LineId = lineId;
                        mp.NoteId = id;
                        mp.PressTime = double.NaN;
                        pending.Add(mp);
                        continue;
                    }
                    if (-dt > LimitBad)
                    {
                        break;
                    }
                    if (note.Kind.Type != NoteKindType.Drag && note.Kind.Type != NoteKindType.Flick)
                    {
                        continue;
                    }
                    double adt = Math.Abs(dt);
                    note.Obj.Translation.X.SetTime(t);
                    double x2 = note.Obj.Translation.X.Now();
                    for (int k = 0; k < list.Count; k++)
                    {
                        double dx = Math.Abs(pos[lineId][k].X - x2) / note.JudgeArea;
                        if (dx <= XDiffMax && adt <= LimitBad - LimitPerfect * Math.Max(0.0, dx - 0.9))
                        {
                            note.Judge = JudgeStatus.PreJudgeStatus;
                            break;
                        }
                    }
                }
            }

            // 5. Resolve pre-judged notes.
            for (int lineId = 0; lineId < lineCount; lineId++)
            {
                JudgeLine line = chart.Lines[lineId];
                line.Obj.SetTime(t);
                LineCursor cur = _cursors[lineId];
                for (int ci = cur.St; ci < cur.Indices.Count; ci++)
                {
                    int id = cur.Indices[ci];
                    Note note = line.Notes[id];

                    if (note.Judge.Kind == JudgeStatusKind.Hold && note.Judge.PreJudge
                        && note.Kind.Type == NoteKindType.Hold && note.Kind.EndTime <= t)
                    {
                        bool perfect = note.Judge.Perfect;
                        double pressTime = note.Judge.Diff;
                        note.Judge = JudgeStatus.Judged;
                        Pending hp = new Pending();
                        hp.What = perfect ? Judgement.Perfect : Judgement.Good;
                        hp.LineId = lineId;
                        hp.NoteId = id;
                        hp.PressTime = pressTime;
                        pending.Add(hp);
                        continue;
                    }

                    double ghostT = t + LimitGood;
                    if (note.Kind.Type == NoteKindType.Click)
                    {
                        if (ghostT < note.Time)
                        {
                            break;
                        }
                    }
                    else if (t < note.Time)
                    {
                        continue;
                    }
                    if (note.Judge.Kind == JudgeStatusKind.PreJudge)
                    {
                        note.Judge = JudgeStatus.Judged;
                        if (note.Kind.Type != NoteKindType.Click)
                        {
                            Pending pp = new Pending();
                            pp.What = Judgement.Perfect;
                            pp.LineId = lineId;
                            pp.NoteId = id;
                            pp.PressTime = note.Time;
                            pending.Add(pp);
                        }
                    }
                }
            }

            // 6. Commit.
            for (int i = 0; i < pending.Count; i++)
            {
                Pending p = pending[i];
                JudgeLine line = chart.Lines[p.LineId];
                Note note = line.Notes[p.NoteId];
                line.Obj.SetTime(t);
                note.Obj.SetTime(t);

                double diff;
                if (p.What == Judgement.Miss)
                {
                    diff = 0.25;
                }
                else if (note.Kind.Type == NoteKindType.Drag || note.Kind.Type == NoteKindType.Flick)
                {
                    diff = 0.0;
                }
                else
                {
                    double at = double.IsNaN(p.PressTime) ? t : p.PressTime;
                    diff = (at - note.Time) / spd;
                }
                Commit(t, p.What, diff, sink, p.LineId, p.NoteId);

                if (p.What == Judgement.Bad && note.Kind.Type != NoteKindType.Hold)
                {
                    BadNote bn = new BadNote();
                    bn.Time = t;
                    bn.Kind = note.Kind;
                    double lineHeight = line.Height.Now();
                    float baseVal = (float)((note.Height - lineHeight) / _aspectRatio * note.Speed);
                    Mat3 mat = line.NowTransform(_aspectRatio, chart.Lines);
                    if (!note.Above)
                    {
                        mat = mat * Mat3.Scaling(1f, -1f);
                    }
                    float inclineDeg;
                    float inclineSin = line.Incline.TryNow(out inclineDeg) ? (float)Math.Sin(inclineDeg * Math.PI / 180.0) : 0f;
                    mat = mat * Renderer.NoteTransform(note, line, baseVal, inclineSin);
                    bn.Matrix = mat;
                    badNotes.Add(bn);
                }
            }

            // 7. Advance cursors.
            for (int lineId = 0; lineId < lineCount; lineId++)
            {
                JudgeLine line = chart.Lines[lineId];
                LineCursor cur = _cursors[lineId];
                while (cur.St < cur.Indices.Count && line.Notes[cur.Indices[cur.St]].Judge.Kind == JudgeStatusKind.Judged)
                {
                    cur.St++;
                }
            }

            _lastTime = t / spd;
        }

        private void AutoPlayUpdate(Chart chart, double t, double spd, List<JudgementEvent> sink)
        {
            List<Pending> pending = new List<Pending>();
            for (int lineId = 0; lineId < chart.Lines.Count; lineId++)
            {
                JudgeLine line = chart.Lines[lineId];
                LineCursor cur = _cursors[lineId];
                for (int ci = cur.St; ci < cur.Indices.Count; ci++)
                {
                    int id = cur.Indices[ci];
                    Note note = line.Notes[id];
                    if (note.Judge.Kind == JudgeStatusKind.Hold && note.Kind.Type == NoteKindType.Hold && t >= note.Kind.EndTime)
                    {
                        note.Judge = JudgeStatus.Judged;
                        Pending hp = new Pending();
                        hp.What = Judgement.Perfect;
                        hp.LineId = lineId;
                        hp.NoteId = id;
                        hp.PressTime = note.Time;
                        pending.Add(hp);
                        continue;
                    }
                    if (note.Judge.Kind != JudgeStatusKind.NotJudged)
                    {
                        continue;
                    }
                    if (note.Time > t)
                    {
                        break;
                    }
                    if (note.Kind.Type == NoteKindType.Hold)
                    {
                        note.Judge = JudgeStatus.MakeHold(true, t, (t - note.Time) / spd, false, double.PositiveInfinity);
                        Commit(t, Judgement.Perfect, 0.0, sink, lineId, id);
                    }
                    else
                    {
                        note.Judge = JudgeStatus.Judged;
                        Pending p = new Pending();
                        p.What = Judgement.Perfect;
                        p.LineId = lineId;
                        p.NoteId = id;
                        p.PressTime = note.Time;
                        pending.Add(p);
                    }
                }
                while (cur.St < cur.Indices.Count && line.Notes[cur.Indices[cur.St]].Judge.Kind == JudgeStatusKind.Judged)
                {
                    cur.St++;
                }
            }
            for (int i = 0; i < pending.Count; i++)
            {
                Pending p = pending[i];
                Commit(t, Judgement.Perfect, 0.0, sink, p.LineId, p.NoteId);
            }
        }
    }
}
