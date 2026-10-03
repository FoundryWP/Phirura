using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using Phirura.Core;

namespace Phirura.Parse
{
    /// <summary>
    /// Port of prpr/src/parse/pec.rs — the PEC line-based text format.
    /// </summary>
    public static class PecParser
    {
        private sealed class PecEvent
        {
            public double StartTime;
            public double EndTime;
            public float End;
            public byte Easing;

            public static PecEvent Single(double time, float value)
            {
                PecEvent e = new PecEvent();
                e.StartTime = time;
                e.EndTime = time;
                e.End = value;
                e.Easing = 0;
                return e;
            }

            public static PecEvent New(double start, double end, float value, byte easing)
            {
                PecEvent e = new PecEvent();
                e.StartTime = start;
                e.EndTime = end;
                e.End = value;
                e.Easing = easing;
                return e;
            }
        }

        private sealed class PecJudgeLine
        {
            public List<KeyValuePair<double, float>> SpeedEvents = new List<KeyValuePair<double, float>>();
            public List<PecEvent> AlphaEvents = new List<PecEvent>();
            public List<PecEvent> MoveX = new List<PecEvent>();
            public List<PecEvent> MoveY = new List<PecEvent>();
            public List<PecEvent> RotateEvents = new List<PecEvent>();
            public List<Note> Notes = new List<Note>();
        }

        private static double TakeTime(List<string> tok, ref int i, BpmList r)
        {
            double v = TakeFloat(tok, ref i);
            return r.TimeBeats(v);
        }

        private static double TakeFloat(List<string> tok, ref int i)
        {
            if (i >= tok.Count)
            {
                throw new FormatException("unexpected end of line");
            }
            double v;
            if (!double.TryParse(tok[i], NumberStyles.Float, CultureInfo.InvariantCulture, out v))
            {
                throw new FormatException("expected a number, got '" + tok[i] + "'");
            }
            i++;
            return v;
        }

        private static int TakeInt(List<string> tok, ref int i)
        {
            return (int)TakeFloat(tok, ref i);
        }

        private static byte TakeTween(List<string> tok, ref int i)
        {
            int t = TakeInt(tok, ref i);
            return RpeTweenMap.Get(t);
        }

        private static void SanitizeEvents(List<PecEvent> events)
        {
            events.Sort(delegate(PecEvent a, PecEvent b)
            {
                int c = a.EndTime.NotNan().CompareTo(b.EndTime.NotNan());
                return c != 0 ? c : a.StartTime.NotNan().CompareTo(b.StartTime.NotNan());
            });
            double lastEnd = double.NegativeInfinity;
            for (int i = 0; i < events.Count; i++)
            {
                if (events[i].StartTime < lastEnd)
                {
                    events[i].StartTime = lastEnd;
                }
                lastEnd = events[i].EndTime;
            }
        }

        private static AnimFloat ParseEvents(List<PecEvent> events, string desc, List<string> warnings, int lineId)
        {
            SanitizeEvents(events);
            List<Keyframe<float>> kfs = new List<Keyframe<float>>();
            for (int i = 0; i < events.Count; i++)
            {
                PecEvent e = events[i];
                if (e.StartTime == e.EndTime)
                {
                    kfs.Add(new Keyframe<float>(e.StartTime, e.End, 0));
                }
                else
                {
                    if (kfs.Count == 0)
                    {
                        warnings.Add("line " + lineId + ": " + desc + " interpolation before a concrete value; event dropped");
                        continue;
                    }
                    kfs.Add(new Keyframe<float>(e.StartTime, kfs[kfs.Count - 1].Value, e.Easing));
                    kfs.Add(new Keyframe<float>(e.EndTime, e.End, 0));
                }
            }
            if (kfs.Count == 0)
            {
                return new AnimFloat();
            }
            return AnimFloat.New(kfs);
        }

        private static AnimFloat ParseSpeedEvents(List<KeyValuePair<double, float>> pec, double maxTime)
        {
            if (pec.Count == 0 || pec[0].Key >= Consts.Eps)
            {
                pec.Insert(0, new KeyValuePair<double, float>(0.0, 0f));
            }
            List<Keyframe<float>> kfs = new List<Keyframe<float>>();
            double height = 0.0;
            double lastTime = 0.0;
            float lastSpeed = 0f;
            for (int i = 0; i < pec.Count; i++)
            {
                double time = pec[i].Key;
                height += (time - lastTime) * lastSpeed;
                kfs.Add(new Keyframe<float>(time, (float)height, 2));
                lastTime = time;
                lastSpeed = pec[i].Value;
            }
            kfs.Add(new Keyframe<float>(maxTime, (float)(height + (maxTime - lastTime) * lastSpeed), 0));
            return AnimFloat.New(kfs);
        }

        private static JudgeLine ParseJudgeLine(PecJudgeLine pec, int id, double maxTime, List<string> warnings)
        {
            AnimFloat height = ParseSpeedEvents(pec.SpeedEvents, maxTime);

            for (int i = 0; i < pec.Notes.Count; i++)
            {
                Note note = pec.Notes[i];
                height.SetTime(note.Time);
                note.Height = height.Now();
                if (note.Kind.IsHold)
                {
                    height.SetTime(note.Kind.EndTime);
                    note.Kind.EndHeight = height.Now();
                }
            }

            for (int i = 0; i < pec.MoveX.Count; i++)
            {
                pec.MoveX[i].End = pec.MoveX[i].End / 2048f * 2f - 1f;
            }
            for (int i = 0; i < pec.MoveY.Count; i++)
            {
                pec.MoveY[i].End = pec.MoveY[i].End / 1400f * 2f - 1f;
            }
            for (int i = 0; i < pec.AlphaEvents.Count; i++)
            {
                if (pec.AlphaEvents[i].End >= 0f)
                {
                    pec.AlphaEvents[i].End /= 255f;
                }
            }

            JudgeLine line = new JudgeLine();
            line.Id = id;
            line.Kind = JudgeLineKindType.Normal;
            line.Obj.Alpha = ParseEvents(pec.AlphaEvents, "alpha", warnings, id);
            line.Obj.Translation = new AnimVector(
                ParseEvents(pec.MoveX, "move X", warnings, id),
                ParseEvents(pec.MoveY, "move Y", warnings, id));
            line.Obj.Rotation = ParseEvents(pec.RotateEvents, "rotate", warnings, id);
            line.Obj.Scale = new AnimVector(AnimFloat.Fixed(3.91f / 6f), new AnimFloat());
            line.Ctrl = new CtrlObject();
            line.Height = height;
            line.Incline = new AnimFloat();
            line.Notes = pec.Notes;
            line.Color = new AnimColor();
            line.Parent = -1;
            line.RotWithParent = false;
            line.ZIndex = 0;
            line.ShowBelow = false;
            line.AttachUI = UIElement.None;
            return line;
        }

        public static Chart Parse(string source, List<string> warnings)
        {
            double? offset = null;
            BpmList bpm = null;
            List<KeyValuePair<double, double>> bpmList = new List<KeyValuePair<double, double>>();
            List<PecJudgeLine> lines = new List<PecJudgeLine>();
            int lastLine = -1;

            Action ensureBpm = delegate
            {
                if (bpm == null)
                {
                    List<KeyValuePair<double, double>> copy = new List<KeyValuePair<double, double>>(bpmList);
                    if (copy.Count == 0)
                    {
                        copy.Add(new KeyValuePair<double, double>(0.0, 120.0));
                    }
                    bpm = new BpmList(copy);
                }
            };

            Func<int, PecJudgeLine> getLine = delegate(int idx)
            {
                while (lines.Count <= idx)
                {
                    lines.Add(new PecJudgeLine());
                }
                return lines[idx];
            };

            Func<Note> lastNote = delegate
            {
                if (lastLine < 0 || lines[lastLine].Notes.Count == 0)
                {
                    throw new FormatException("no note to modify");
                }
                return lines[lastLine].Notes[lines[lastLine].Notes.Count - 1];
            };

            string[] rawLines = source.Replace("\r\n", "\n").Replace('\r', '\n').Split('\n');
            for (int lineNo = 0; lineNo < rawLines.Length; lineNo++)
            {
                string raw = rawLines[lineNo];
                List<string> tok = raw.Split(new char[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries).ToList();
                try
                {
                    if (offset == null)
                    {
                        if (tok.Count == 0)
                        {
                            continue;
                        }
                        int i0 = 0;
                        offset = TakeFloat(tok, ref i0) / 1000.0 - 0.15;
                        continue;
                    }

                    if (tok.Count == 0)
                    {
                        continue;
                    }
                    int i = 0;
                    string cmd = tok[i++];
                    if (cmd.Length > 2)
                    {
                        throw new FormatException("unknown command '" + cmd + "'");
                    }

                    if (cmd == "bp")
                    {
                        if (bpm != null)
                        {
                            throw new FormatException("bp must appear before any other command");
                        }
                        double beats = TakeFloat(tok, ref i);
                        double value = TakeFloat(tok, ref i);
                        bpmList.Add(new KeyValuePair<double, double>(beats, value));
                    }
                    else if (cmd.Length == 2 && cmd[0] == 'n' && cmd[1] >= '1' && cmd[1] <= '4')
                    {
                        ensureBpm();
                        int lineId = TakeInt(tok, ref i);
                        lastLine = lineId;
                        PecJudgeLine l = getLine(lineId);
                        double time = TakeTime(tok, ref i, bpm);
                        NoteKind kind;
                        switch (cmd[1])
                        {
                            case '1': kind = NoteKind.Click(); break;
                            case '2': kind = NoteKind.Hold(TakeTime(tok, ref i, bpm), 0.0); break;
                            case '3': kind = NoteKind.Flick(); break;
                            default: kind = NoteKind.Drag(); break;
                        }
                        float positionX = (float)TakeFloat(tok, ref i) / 1024f;
                        bool above = TakeInt(tok, ref i) == 1;
                        int fakeRaw = TakeInt(tok, ref i);
                        bool fake = fakeRaw != 0;

                        Note note = new Note();
                        note.Kind = kind;
                        note.Time = time;
                        note.Speed = 1.0;
                        note.Above = above;
                        note.Fake = fake;
                        note.JudgeArea = 1f;
                        note.Color = RgbaF.White;
                        note.HitSound = kind.Type == NoteKindType.Flick ? "flick" : (kind.Type == NoteKindType.Drag ? "drag" : "click");
                        note.Obj.Translation = new AnimVector(AnimFloat.Fixed(positionX), new AnimFloat());
                        l.Notes.Add(note);

                        // Optional trailing modifiers. Upstream consumes a token to test
                        // for '#' which loses a leading '&'; we peek instead.
                        while (i < tok.Count)
                        {
                            if (tok[i] == "#")
                            {
                                i++;
                                note.Speed = TakeFloat(tok, ref i);
                            }
                            else if (tok[i] == "&")
                            {
                                i++;
                                float size = (float)TakeFloat(tok, ref i);
                                if (Math.Abs(size - 1f) >= Consts.Eps)
                                {
                                    note.Obj.Scale.X = AnimFloat.Fixed(size);
                                }
                            }
                            else
                            {
                                break;
                            }
                        }
                    }
                    else if (cmd == "#")
                    {
                        lastNote().Speed = TakeFloat(tok, ref i);
                    }
                    else if (cmd == "&")
                    {
                        float size = (float)TakeFloat(tok, ref i);
                        if (Math.Abs(size - 1f) >= Consts.Eps)
                        {
                            lastNote().Obj.Scale.X = AnimFloat.Fixed(size);
                        }
                    }
                    else if (cmd.Length == 2 && cmd[0] == 'c')
                    {
                        ensureBpm();
                        int lineId = TakeInt(tok, ref i);
                        PecJudgeLine l = getLine(lineId);
                        double time = TakeTime(tok, ref i, bpm);
                        switch (cmd[1])
                        {
                            case 'v':
                                l.SpeedEvents.Add(new KeyValuePair<double, float>(time, (float)TakeFloat(tok, ref i) / 5.85f));
                                break;
                            case 'p':
                                {
                                    float x = (float)TakeFloat(tok, ref i);
                                    float y = (float)TakeFloat(tok, ref i);
                                    l.MoveX.Add(PecEvent.Single(time, x));
                                    l.MoveY.Add(PecEvent.Single(time, y));
                                    break;
                                }
                            case 'd':
                                l.RotateEvents.Add(PecEvent.Single(time, -(float)TakeFloat(tok, ref i)));
                                break;
                            case 'a':
                                l.AlphaEvents.Add(PecEvent.Single(time, (float)TakeFloat(tok, ref i)));
                                break;
                            case 'm':
                                {
                                    double endTime = TakeTime(tok, ref i, bpm);
                                    float x = (float)TakeFloat(tok, ref i);
                                    float y = (float)TakeFloat(tok, ref i);
                                    byte t = TakeTween(tok, ref i);
                                    l.MoveX.Add(PecEvent.New(time, endTime, x, t));
                                    l.MoveY.Add(PecEvent.New(time, endTime, y, t));
                                    break;
                                }
                            case 'r':
                                {
                                    double endTime = TakeTime(tok, ref i, bpm);
                                    float ang = -(float)TakeFloat(tok, ref i);
                                    byte t = TakeTween(tok, ref i);
                                    l.RotateEvents.Add(PecEvent.New(time, endTime, ang, t));
                                    break;
                                }
                            case 'f':
                                {
                                    double endTime = TakeTime(tok, ref i, bpm);
                                    float alpha = (float)TakeFloat(tok, ref i);
                                    l.AlphaEvents.Add(PecEvent.New(time, endTime, alpha, 2));
                                    break;
                                }
                            default:
                                throw new FormatException("unknown command '" + cmd + "'");
                        }
                    }
                    else
                    {
                        throw new FormatException("unknown command '" + cmd + "'");
                    }
                }
                catch (Exception ex)
                {
                    warnings.Add("pec line " + (lineNo + 1) + ": " + ex.Message);
                }
            }

            if (offset == null)
            {
                offset = 0.0;
            }
            ensureBpm();

            double maxTime = 0.0;
            for (int i = 0; i < lines.Count; i++)
            {
                PecJudgeLine l = lines[i];
                for (int j = 0; j < l.AlphaEvents.Count; j++) maxTime = Math.Max(maxTime, l.AlphaEvents[j].EndTime.NotNan());
                for (int j = 0; j < l.RotateEvents.Count; j++) maxTime = Math.Max(maxTime, l.RotateEvents[j].EndTime.NotNan());
                for (int j = 0; j < l.MoveX.Count; j++) maxTime = Math.Max(maxTime, l.MoveX[j].EndTime.NotNan());
                for (int j = 0; j < l.MoveY.Count; j++) maxTime = Math.Max(maxTime, l.MoveY[j].EndTime.NotNan());
                for (int j = 0; j < l.SpeedEvents.Count; j++) maxTime = Math.Max(maxTime, l.SpeedEvents[j].Key.NotNan());
                for (int j = 0; j < l.Notes.Count; j++) maxTime = Math.Max(maxTime, l.Notes[j].Time.NotNan());
            }
            maxTime += 1.0;

            Chart chart = new Chart();
            chart.SourceFormat = "PEC";
            chart.Offset = (float)offset.Value;
            chart.Bpm = bpm;
            chart.Settings = new ChartSettings();
            chart.Settings.PeAlphaExtension = true;
            for (int i = 0; i < lines.Count; i++)
            {
                chart.Lines.Add(ParseJudgeLine(lines[i], i, maxTime, warnings));
            }
            chart.Finalize(0);
            return chart;
        }
    }
}
