using System;
using System.Collections.Generic;
using System.Linq;

namespace Phirura.Core
{
    public static class Consts
    {
        public const double NoteWidthRatioBase = 0.13175016;
        public const double HeightRatio = 0.83175;
        public const double Eps = 1e-5;

        /// <summary>RPE charts are authored in a 1350 x 900 space.</summary>
        public const float RpeWidth = 1350f;
        public const float RpeHeight = 900f;

        /// <summary>10 / 45 / HEIGHT_RATIO — RPE speed unit to world units.</summary>
        public const double SpeedRatio = 10.0 / 45.0 / HeightRatio;
    }

    public static class NumericExt
    {
        /// <summary>Port of prpr's `NotNanExt::not_nan` — NaN sorts last.</summary>
        public static double NotNan(this double v)
        {
            return double.IsNaN(v) ? double.PositiveInfinity : v;
        }

        public static float NotNan(this float v)
        {
            return float.IsNaN(v) ? float.PositiveInfinity : v;
        }
    }

    public enum NoteKindType
    {
        Click = 1,
        Hold = 2,
        Flick = 3,
        Drag = 4
    }

    public struct NoteKind
    {
        public NoteKindType Type;
        public double EndTime;
        public double EndHeight;

        public static NoteKind Click()
        {
            NoteKind k = new NoteKind();
            k.Type = NoteKindType.Click;
            return k;
        }

        public static NoteKind Hold(double endTime, double endHeight)
        {
            NoteKind k = new NoteKind();
            k.Type = NoteKindType.Hold;
            k.EndTime = endTime;
            k.EndHeight = endHeight;
            return k;
        }

        public static NoteKind Flick()
        {
            NoteKind k = new NoteKind();
            k.Type = NoteKindType.Flick;
            return k;
        }

        public static NoteKind Drag()
        {
            NoteKind k = new NoteKind();
            k.Type = NoteKindType.Drag;
            return k;
        }

        public bool IsHold
        {
            get { return Type == NoteKindType.Hold; }
        }

        /// <summary>Render order within a line: hold, drag, click, flick (prpr's NoteKind::order).</summary>
        public int Order
        {
            get
            {
                switch (Type)
                {
                    case NoteKindType.Hold: return 0;
                    case NoteKindType.Drag: return 1;
                    case NoteKindType.Click: return 2;
                    default: return 3;
                }
            }
        }
    }

    public enum JudgeStatusKind
    {
        NotJudged,
        PreJudge,
        Judged,
        Hold
    }

    public struct JudgeStatus
    {
        public JudgeStatusKind Kind;
        public bool Perfect;
        public double At;
        public double Diff;
        public bool PreJudge;
        public double UpTime;

        public static readonly JudgeStatus NotJudged = new JudgeStatus { Kind = JudgeStatusKind.NotJudged };
        public static readonly JudgeStatus Judged = new JudgeStatus { Kind = JudgeStatusKind.Judged };
        public static readonly JudgeStatus PreJudgeStatus = new JudgeStatus { Kind = JudgeStatusKind.PreJudge };

        public static JudgeStatus MakeHold(bool perfect, double at, double diff, bool preJudge, double upTime)
        {
            JudgeStatus s = new JudgeStatus();
            s.Kind = JudgeStatusKind.Hold;
            s.Perfect = perfect;
            s.At = at;
            s.Diff = diff;
            s.PreJudge = preJudge;
            s.UpTime = upTime;
            return s;
        }
    }

    /// <summary>Which in-game UI element a judge line drives (RPE `attachUI`).</summary>
    public enum UIElement
    {
        None = 0,
        Pause = 1,
        ComboNumber = 2,
        Combo = 3,
        Score = 4,
        Bar = 5,
        Name = 6,
        Level = 7
    }

    /// <summary>Port of prpr::core::Object — an animated local coordinate system.</summary>
    public sealed class Object
    {
        public AnimFloat Alpha = new AnimFloat();
        public AnimVector Scale = new AnimVector();
        public AnimFloat Rotation = new AnimFloat();
        public AnimVector Translation = new AnimVector();

        public void SetTime(double time)
        {
            Alpha.SetTime(time);
            Scale.SetTime(time);
            Rotation.SetTime(time);
            Translation.SetTime(time);
        }

        public bool Dead()
        {
            return Alpha.Dead() && Scale.X.Dead() && Scale.Y.Dead() && Rotation.Dead() && Translation.X.Dead() && Translation.Y.Dead();
        }

        public Mat3 NowRotation()
        {
            return Mat3.RotationDegrees(Rotation.Now());
        }

        public Vec2 NowTranslation(float aspectRatio)
        {
            Vec2 tr = Translation.Now();
            tr.Y /= aspectRatio;
            return tr;
        }

        public Mat3 Now(float aspectRatio)
        {
            return NowRotation().AppendTranslation(NowTranslation(aspectRatio));
        }

        public float NowAlpha()
        {
            float v;
            float a = Alpha.TryNow(out v) ? v : 1.0f;
            return Math.Max(0f, a);
        }

        /// <summary>`now_scale` — scale about an arbitrary centre point.</summary>
        public Mat3 NowScale(Vec2 ct)
        {
            Vec2 scale = Scale.NowWithDef(1f, 1f);
            return Mat3.Translation(-ct.X, -ct.Y).AppendNonuniformScaling(scale).AppendTranslation(ct);
        }
    }

    /// <summary>Port of prpr::core::CtrlObject — driven by the note's *height*, not by time.</summary>
    public sealed class CtrlObject
    {
        public AnimFloat Alpha = new AnimFloat();
        public AnimFloat Size = new AnimFloat();
        public AnimFloat Pos = new AnimFloat();
        public AnimFloat Y = new AnimFloat();

        public void SetHeight(double height)
        {
            Alpha.SetTime(height);
            Size.SetTime(height);
            Pos.SetTime(height);
            Y.SetTime(height);
        }

        public float AlphaOr1()
        {
            float v;
            return Alpha.TryNow(out v) ? v : 1f;
        }

        public float SizeOr1()
        {
            float v;
            return Size.TryNow(out v) ? v : 1f;
        }

        public float PosOr1()
        {
            float v;
            return Pos.TryNow(out v) ? v : 1f;
        }

        public float YOr1()
        {
            float v;
            return Y.TryNow(out v) ? v : 1f;
        }
    }

    public enum JudgeLineKindType
    {
        Normal,
        Texture,
        Text,
        Paint
    }

    public sealed class Note
    {
        public Object Obj = new Object();
        public NoteKind Kind;
        public string HitSound = "click";
        public double Time;
        public double Height;
        public double Speed = 1.0;
        public RgbaF Color = RgbaF.White;
        public bool HasFxColor;
        public RgbaF FxColor;
        public float JudgeArea = 1f;

        /// <summary>Note comes from the other side of the line.</summary>
        public bool Above;

        public bool MultipleHint;
        public bool Fake;
        public JudgeStatus Judge = JudgeStatus.NotJudged;

        /// <summary>Port of Note::plain — used as the primary sort key.</summary>
        public bool Plain()
        {
            return !Fake && Kind.Type != NoteKindType.Hold && Obj.Translation.Y.Keyframes.Length <= 1;
        }

        public float Rotation(JudgeLine line)
        {
            return line.Obj.Rotation.Now() + (Above ? 0f : 180f);
        }
    }

    public sealed class JudgeLine
    {
        public Object Obj = new Object();
        public CtrlObject Ctrl = new CtrlObject();
        public JudgeLineKindType Kind = JudgeLineKindType.Normal;

        /// <summary>Texture path relative to the chart folder (for Kind == Texture).</summary>
        public string TexturePath;
        public AnimString TextAnim;
        public AnimFloat PaintAnim;

        /// <summary>∫ speed(t) dt — the absolute Y of notes attached to this line.</summary>
        public AnimFloat Height = new AnimFloat();
        public AnimFloat Incline = new AnimFloat();

        public List<Note> Notes = new List<Note>();
        public AnimColor Color = new AnimColor();

        public int Parent = -1;
        public bool RotWithParent;
        public int ZIndex;
        public bool ShowBelow = true;
        public UIElement AttachUI = UIElement.None;

        public int Id;

        public float FetchRot(List<JudgeLine> lines)
        {
            float rot = Obj.Rotation.Now();
            if (RotWithParent && Parent >= 0 && Parent < lines.Count)
            {
                rot += lines[Parent].FetchRot(lines);
            }
            return rot;
        }

        public Vec2 FetchPos(float aspectRatio, List<JudgeLine> lines)
        {
            if (Parent >= 0 && Parent < lines.Count)
            {
                JudgeLine p = lines[Parent];
                Vec2 pt = p.FetchPos(aspectRatio, lines);
                double rad = p.FetchRot(lines) * Math.PI / 180.0;
                Vec2 t = Obj.NowTranslation(aspectRatio);
                float c = (float)Math.Cos(rad);
                float s = (float)Math.Sin(rad);
                return pt + new Vec2(t.X * c - t.Y * s, t.X * s + t.Y * c);
            }
            return Obj.NowTranslation(aspectRatio);
        }

        public Mat3 NowTransform(float aspectRatio, List<JudgeLine> lines)
        {
            return Mat3.RotationDegrees(FetchRot(lines)).AppendTranslation(FetchPos(aspectRatio, lines));
        }
    }

    public sealed class ChartSettings
    {
        /// <summary>PEC's negative-alpha extension.</summary>
        public bool PeAlphaExtension;
        public bool HoldPartialCover;
    }

    public sealed class Chart
    {
        /// <summary>Audio offset in seconds.</summary>
        public float Offset;

        public List<JudgeLine> Lines = new List<JudgeLine>();
        public BpmList Bpm = new BpmList();
        public ChartSettings Settings = new ChartSettings();

        /// <summary>Indices into Lines, sorted by z-index; lines with attachUI are excluded.</summary>
        public List<int> Order = new List<int>();

        public int[] AttachUI = new int[7];

        /// <summary>Chart display info (title/artist/level/difficulty).</summary>
        public string Name = string.Empty;
        public string Artist = string.Empty;
        public string Charter = string.Empty;
        public string Level = string.Empty;
        public string IllustrationPath;
        public string MusicPath;
        public string SourceFormat = "RPE";

        public double Duration;

        public Chart()
        {
            for (int i = 0; i < 7; i++)
            {
                AttachUI[i] = -1;
            }
        }

        public void Finalize(int maxNoteCountHint)
        {
            Order.Clear();
            for (int i = 0; i < 7; i++)
            {
                AttachUI[i] = -1;
            }
            for (int i = 0; i < Lines.Count; i++)
            {
                JudgeLine line = Lines[i];
                line.Id = i;
                if (line.AttachUI != UIElement.None)
                {
                    AttachUI[(int)line.AttachUI - 1] = i;
                }
                else
                {
                    Order.Add(i);
                }
            }
            Order = Order.OrderBy(i => Lines[i].ZIndex).ThenBy(i => i).ToList();

            // prpr sorts notes per line so that "plain" notes (no holds / no Y motion)
            // come first; the renderer relies on that ordering.
            for (int i = 0; i < Lines.Count; i++)
            {
                List<Note> notes = Lines[i].Notes;
                Lines[i].Notes = notes
                    .OrderBy(n => n.Plain() ? 0 : 1)
                    .ThenBy(n => n.Above ? 0 : 1)
                    .ThenBy(n => (double)n.Speed.NotNan())
                    .ThenBy(n => ((n.Height + n.Obj.Translation.Y.Now()) * n.Speed).NotNan())
                    .ToList();
            }

            ProcessLines(Lines);
            ComputeDuration();
        }

        private void ComputeDuration()
        {
            double max = 0.0;
            for (int i = 0; i < Lines.Count; i++)
            {
                List<Note> notes = Lines[i].Notes;
                for (int j = 0; j < notes.Count; j++)
                {
                    double t = notes[j].Kind.IsHold ? notes[j].Kind.EndTime : notes[j].Time;
                    if (t > max) max = t;
                }
            }
            Duration = max;
        }

        public int NoteCount()
        {
            int n = 0;
            for (int i = 0; i < Lines.Count; i++)
            {
                List<Note> notes = Lines[i].Notes;
                for (int j = 0; j < notes.Count; j++)
                {
                    if (!notes[j].Fake) n++;
                }
            }
            return n;
        }

        public void Reset()
        {
            for (int i = 0; i < Lines.Count; i++)
            {
                List<Note> notes = Lines[i].Notes;
                for (int j = 0; j < notes.Count; j++)
                {
                    notes[j].Judge = JudgeStatus.NotJudged;
                }
            }
        }

        /// <summary>Port of prpr::parse::process_lines — flags notes sharing an exact time.</summary>
        public static void ProcessLines(List<JudgeLine> lines)
        {
            List<double> times = new List<double>();
            for (int li = 0; li < lines.Count; li++)
            {
                List<Note> v = lines[li].Notes;
                List<int> idx = Enumerable.Range(0, v.Count).OrderBy(i => v[i].Time.NotNan()).ToList();
                int i2 = 0;
                while (i2 < v.Count)
                {
                    times.Add(v[idx[i2]].Time.NotNan());
                    int j = i2 + 1;
                    while (j < v.Count && v[idx[j]].Time == v[idx[i2]].Time)
                    {
                        j++;
                    }
                    if (j != i2 + 1)
                    {
                        times.Add(v[idx[i2]].Time.NotNan());
                    }
                    i2 = j;
                }
            }
            times.Sort();
            List<double> mt = new List<double>();
            for (int i = 0; i + 1 < times.Count; i++)
            {
                if (times[i] == times[i + 1] && (i == 0 || times[i - 1] != times[i]))
                {
                    mt.Add(times[i]);
                }
            }
            for (int li = 0; li < lines.Count; li++)
            {
                List<Note> v = lines[li].Notes;
                List<int> idx = Enumerable.Range(0, v.Count).OrderBy(j => v[j].Time.NotNan()).ToList();
                int i = 0;
                for (int k = 0; k < idx.Count; k++)
                {
                    Note note = v[idx[k]];
                    double time = note.Time;
                    while (i < mt.Count && mt[i] < time)
                    {
                        i++;
                    }
                    if (i < mt.Count && mt[i] == time)
                    {
                        note.MultipleHint = true;
                    }
                }
            }
        }
    }
}
