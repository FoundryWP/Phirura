using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using Phirura.Core;
using Windows.Data.Json;

namespace Phirura.Parse
{
    /// <summary>
    /// Port of prpr/src/parse/rpe.rs — the RPE chart format (Phira's native format,
    /// and what most community charts ship as).
    /// </summary>
    public static class RpeParser
    {
        private const double SpeedRatio = Consts.SpeedRatio;

        private sealed class RpeEvent
        {
            public float EasingLeft;
            public float EasingRight = 1f;
            public int Bezier;
            public float[] BezierPoints = new float[4];
            public int EasingType = 1;
            public double Start;
            public double End;
            public Triple StartTime = Triple.Zero;
            public Triple EndTime = Triple.Zero;
            public RgbaF StartColor = RgbaF.White;
            public RgbaF EndColor = RgbaF.White;
            public string StartText = string.Empty;
            public string EndText = string.Empty;
            public bool IsColor;
            public bool IsText;

            public long BezierKeyA()
            {
                return (long)Int100(BezierPoints[0]) * 100L + Int100(BezierPoints[1]);
            }

            public int BezierKeyB()
            {
                return Int100(BezierPoints[2]);
            }

            public int BezierKeyC()
            {
                return Int100(BezierPoints[3]);
            }

            private static int Int100(float p)
            {
                return (int)Math.Round(p * 100f);
            }

            public ITweenFunction Tween(Dictionary<string, ITweenFunction> bezierMap)
            {
                byte tw = RpeTweenMap.Get(Math.Max(EasingType, 1));
                float left = Clamp01(EasingLeft);
                float right = Clamp01(EasingRight);
                if (Bezier != 0)
                {
                    string key = BezierKeyA() + ":" + BezierKeyB() + ":" + BezierKeyC();
                    ITweenFunction bez;
                    if (bezierMap.TryGetValue(key, out bez))
                    {
                        return bez;
                    }
                    return Tweens.Static(tw);
                }
                if (tw <= 2 || (Math.Abs(left) < Consts.Eps && Math.Abs(right - 1f) < Consts.Eps) || left >= right)
                {
                    return Tweens.Static(tw);
                }
                return new ClampedTween(tw, left, right);
            }
        }

        private sealed class RpeLayer
        {
            public List<RpeEvent> Alpha;
            public List<RpeEvent> MoveX;
            public List<RpeEvent> MoveY;
            public List<RpeEvent> Rotate;
            public List<RpeEvent> Speed;
        }

        private sealed class RpeExtended
        {
            public List<RpeEvent> Color;
            public List<RpeEvent> Text;
            public List<RpeEvent> ScaleX;
            public List<RpeEvent> ScaleY;
            public List<RpeEvent> Incline;
            public List<RpeEvent> Paint;
        }

        private sealed class RpeCtrlEvent
        {
            public int Easing;
            public double X;
            public double Value;
        }

        private sealed class RpeNote
        {
            public int Kind;
            public int Above;
            public Triple StartTime = Triple.Zero;
            public Triple EndTime = Triple.Zero;
            public float PositionX;
            public float YOffset;
            public int Alpha = 255;
            public string HitSound;
            public float Size = 1f;
            public float Speed = 1f;
            public int IsFake;
            public double VisibleTime;
            public bool HasTint;
            public RgbaF Tint = RgbaF.White;
            public bool HasFxTint;
            public RgbaF FxTint = RgbaF.White;
            public float JudgeArea = 1f;
        }

        private sealed class RpeJudgeLine
        {
            public string Name = string.Empty;
            public string Texture = string.Empty;
            public int Father = -1;
            public bool RotateWithFather;
            public List<RpeLayer> Layers = new List<RpeLayer>();
            public RpeExtended Extended;
            public List<RpeNote> Notes = new List<RpeNote>();
            public int IsCover;
            public int ZOrder;
            public UIElement AttachUI = UIElement.None;
            public List<RpeCtrlEvent> PosCtrl = new List<RpeCtrlEvent>();
            public List<RpeCtrlEvent> SizeCtrl = new List<RpeCtrlEvent>();
            public List<RpeCtrlEvent> AlphaCtrl = new List<RpeCtrlEvent>();
            public List<RpeCtrlEvent> YCtrl = new List<RpeCtrlEvent>();
        }

        private static float Clamp01(float v)
        {
            if (v < 0f) return 0f;
            if (v > 1f) return 1f;
            return v;
        }

        private static float Signum(double v)
        {
            return v >= 0.0 ? 1f : -1f;
        }

        // ------------------------------------------------------------------
        // Event parsing
        // ------------------------------------------------------------------

        private static AnimFloat ParseEventsFloat(BpmList r, List<RpeEvent> rpe, bool hasDefault, float def, Dictionary<string, ITweenFunction> bm)
        {
            if (rpe == null || rpe.Count == 0)
            {
                return new AnimFloat();
            }
            List<Keyframe<float>> kfs = new List<Keyframe<float>>();
            if (hasDefault && rpe[0].StartTime.Beats != 0.0)
            {
                kfs.Add(new Keyframe<float>(0.0, def, 0));
            }
            for (int i = 0; i < rpe.Count; i++)
            {
                RpeEvent e = rpe[i];
                kfs.Add(new Keyframe<float>(r.Time(e.StartTime), (float)e.Start, e.Tween(bm)));
                kfs.Add(new Keyframe<float>(r.Time(e.EndTime), (float)e.End, Tweens.Static(0)));
            }
            return AnimFloat.New(kfs);
        }

        private static AnimColor ParseEventsColor(BpmList r, List<RpeEvent> rpe, bool hasDefault, RgbaF def, Dictionary<string, ITweenFunction> bm)
        {
            if (rpe == null || rpe.Count == 0)
            {
                return new AnimColor();
            }
            List<Keyframe<RgbaF>> kfs = new List<Keyframe<RgbaF>>();
            if (hasDefault && rpe[0].StartTime.Beats != 0.0)
            {
                kfs.Add(new Keyframe<RgbaF>(0.0, def, 0));
            }
            for (int i = 0; i < rpe.Count; i++)
            {
                RpeEvent e = rpe[i];
                kfs.Add(new Keyframe<RgbaF>(r.Time(e.StartTime), e.StartColor, e.Tween(bm)));
                kfs.Add(new Keyframe<RgbaF>(r.Time(e.EndTime), e.EndColor, Tweens.Static(0)));
            }
            return AnimColor.New(kfs);
        }

        private static AnimString ParseEventsString(BpmList r, List<RpeEvent> rpe, bool hasDefault, string def, Dictionary<string, ITweenFunction> bm)
        {
            if (rpe == null || rpe.Count == 0)
            {
                return new AnimString();
            }
            List<Keyframe<string>> kfs = new List<Keyframe<string>>();
            if (hasDefault && rpe[0].StartTime.Beats != 0.0)
            {
                kfs.Add(new Keyframe<string>(0.0, def, 0));
            }
            for (int i = 0; i < rpe.Count; i++)
            {
                RpeEvent e = rpe[i];
                kfs.Add(new Keyframe<string>(r.Time(e.StartTime), e.StartText, e.Tween(bm)));
                kfs.Add(new Keyframe<string>(r.Time(e.EndTime), e.EndText, Tweens.Static(0)));
            }
            return AnimString.New(kfs);
        }

        private static AnimFloat EventsWithFactor(BpmList r, List<RpeLayer> layers, Func<RpeLayer, List<RpeEvent>> get, float factor, Dictionary<string, ITweenFunction> bm)
        {
            List<AnimBase<float>> anis = new List<AnimBase<float>>();
            for (int i = 0; i < layers.Count; i++)
            {
                List<RpeEvent> es = get(layers[i]);
                if (es != null)
                {
                    anis.Add(ParseEventsFloat(r, es, false, 0f, bm));
                }
            }
            AnimFloat res = (AnimFloat)AnimFloat.Chain(anis);
            if (res == null || res.IsDefault())
            {
                return AnimFloat.Fixed(0f);
            }
            res.MapValue(delegate(float v) { return v * factor; });
            return res;
        }

        // ------------------------------------------------------------------
        // Speed events
        // ------------------------------------------------------------------

        private static ITweenFunction SpeedLinearTween(float startSpeed, float endSpeed)
        {
            if (Math.Abs(startSpeed - endSpeed) < Consts.Eps)
            {
                return Tweens.Static(2);
            }
            if (Math.Abs(startSpeed) > Math.Abs(endSpeed))
            {
                return new ClampedTween(7, 0f, 1f - endSpeed / startSpeed);
            }
            return new ClampedTween(6, startSpeed / endSpeed, 1f);
        }

        private static void SpeedSegmentTween(bool modern, float startSpeed, float endSpeed, ITweenFunction tween, out ITweenFunction result, out float factor)
        {
            bool ok = false;
            ITweenFunction res = null;
            float tot = 0f;

            if (!modern)
            {
                float df0 = tween.Derivative(0f);
                float df1 = tween.Derivative(1f);
                float denom = df1 - df0;
                if (!float.IsNaN(denom) && !float.IsInfinity(denom) && Math.Abs(denom) >= 1e-8f)
                {
                    float k = (endSpeed - startSpeed) / denom;
                    float b = startSpeed - k * df0;
                    ok = SpeedIntegralTween.TryCreate(tween, k, b, out res, out tot);
                }
            }
            else
            {
                ITweenFunction intTween;
                if (tween.Kind == TweenKind.Static)
                {
                    intTween = Tweens.IntStatic(((StaticTween)tween).Id);
                }
                else if (tween.Kind == TweenKind.Clamped)
                {
                    ClampedTween c = (ClampedTween)tween;
                    intTween = new IntClampedTween(c.Id, c.Lo, c.Hi);
                }
                else
                {
                    intTween = new GeneralIntTween(tween);
                }
                ok = SpeedIntegralTween.TryCreate(intTween, endSpeed - startSpeed, startSpeed, out res, out tot);
            }

            if (!ok)
            {
                result = SpeedLinearTween(startSpeed, endSpeed);
                factor = (startSpeed + endSpeed) / 2f;
                return;
            }
            result = res;
            factor = tot;
        }

        private static AnimFloat ParseSpeedEvents(BpmList r, List<RpeLayer> layers, Dictionary<string, ITweenFunction> bm, double maxTime, bool modern)
        {
            List<List<RpeEvent>> speedLayers = new List<List<RpeEvent>>();
            for (int i = 0; i < layers.Count; i++)
            {
                if (layers[i].Speed != null)
                {
                    speedLayers.Add(layers[i].Speed);
                }
            }
            if (speedLayers.Count == 0)
            {
                return DefaultSpeed(maxTime);
            }

            List<AnimBase<float>> anis = new List<AnimBase<float>>();
            for (int li = 0; li < speedLayers.Count; li++)
            {
                List<RpeEvent> layer = speedLayers[li];
                if (layer.Count == 0)
                {
                    continue;
                }
                List<RpeEvent> events = layer.OrderBy(e => e.StartTime.Beats.NotNan()).ToList();

                List<Keyframe<float>> kfs = new List<Keyframe<float>>();
                kfs.Add(new Keyframe<float>(0.0, 0f, 2));
                double height = 0.0;

                Action<double, double, ITweenFunction, float> pushKf = delegate(double startTime, double endTime, ITweenFunction tween, float fac)
                {
                    if (endTime - startTime <= Consts.Eps)
                    {
                        return;
                    }
                    if (kfs.Count > 0)
                    {
                        int last = kfs.Count - 1;
                        if (Math.Abs(kfs[last].Time - startTime) < Consts.Eps)
                        {
                            Keyframe<float> k = kfs[last];
                            k.Value = (float)height;
                            k.Tween = tween;
                            kfs[last] = k;
                        }
                        else
                        {
                            kfs.Add(new Keyframe<float>(startTime, (float)height, tween));
                        }
                    }
                    height += fac * (endTime - startTime);
                };

                double cursor = 0.0;
                float lastSpeed = 0f;
                for (int ei = 0; ei < events.Count; ei++)
                {
                    RpeEvent ev = events[ei];
                    double startTime = Math.Max(r.Time(ev.StartTime), cursor);
                    double endTime = Math.Max(r.Time(ev.EndTime), startTime);
                    float startSpeed = (float)ev.Start * (float)SpeedRatio;
                    float endSpeed = (float)ev.End * (float)SpeedRatio;

                    pushKf(cursor, startTime, Tweens.Static(2), lastSpeed);

                    if (endTime > startTime + Consts.Eps)
                    {
                        if (ev.EasingType == 0)
                        {
                            pushKf(startTime, endTime, Tweens.Static(2), startSpeed);
                        }
                        else if (ev.EasingType <= 1)
                        {
                            if (startSpeed * endSpeed < 0f)
                            {
                                float x = startSpeed / (startSpeed - endSpeed);
                                double mid = startTime + (endTime - startTime) * x;
                                pushKf(startTime, mid, SpeedLinearTween(startSpeed, 0f), (startSpeed + 0f) / 2f);
                                pushKf(mid, endTime, SpeedLinearTween(0f, endSpeed), (0f + endSpeed) / 2f);
                            }
                            else
                            {
                                pushKf(startTime, endTime, SpeedLinearTween(startSpeed, endSpeed), (startSpeed + endSpeed) / 2f);
                            }
                        }
                        else
                        {
                            ITweenFunction tw;
                            float fac;
                            SpeedSegmentTween(modern, startSpeed, endSpeed, ev.Tween(bm), out tw, out fac);
                            pushKf(startTime, endTime, tw, fac);
                        }
                    }
                    cursor = endTime;
                    lastSpeed = endSpeed;
                }

                pushKf(cursor, maxTime, Tweens.Static(2), lastSpeed);
                if (kfs.Count > 0 && Math.Abs(kfs[kfs.Count - 1].Time - maxTime) > Consts.Eps)
                {
                    kfs.Add(new Keyframe<float>(maxTime, (float)height, 0));
                }
                anis.Add(AnimFloat.New(kfs));
            }

            if (anis.Count == 0)
            {
                return new AnimFloat();
            }
            return (AnimFloat)AnimFloat.Chain(anis);
        }

        /// <summary>
        /// RPE requires `speedEvents`, but hand-written and converted charts sometimes omit
        /// them. Without any, every note's height integral stays 0, which stacks the entire
        /// chart on top of the judge line and looks like nothing is moving. Fall back to a
        /// constant speed of one chart-unit per second, so notes get ~1 s of travel time.
        ///
        /// The returned animation holds absolute heights (the integral), matching what both
        /// the modern and legacy paths produce.
        /// </summary>
        private static AnimFloat DefaultSpeed(double maxTime)
        {
            double end = maxTime > 0.0 ? maxTime : 1.0;
            List<Keyframe<float>> kfs = new List<Keyframe<float>>();
            kfs.Add(new Keyframe<float>(0.0, 0f, 2));            // 2 == linear
            kfs.Add(new Keyframe<float>(end, (float)end, 0));
            return AnimFloat.New(kfs);
        }

        private static AnimFloat ParseSpeedEventsLegacy(BpmList r, List<RpeLayer> layers, double maxTime)
        {
            List<List<RpeEvent>> speedLayers = new List<List<RpeEvent>>();
            for (int i = 0; i < layers.Count; i++)
            {
                if (layers[i].Speed != null)
                {
                    speedLayers.Add(layers[i].Speed);
                }
            }
            if (speedLayers.Count == 0)
            {
                return DefaultSpeed(maxTime);
            }

            List<AnimBase<float>> anis = new List<AnimBase<float>>();
            for (int i = 0; i < speedLayers.Count; i++)
            {
                List<RpeEvent> it = speedLayers[i];
                if (it.Count == 0)
                {
                    continue;
                }
                List<Keyframe<float>> kfs = new List<Keyframe<float>>();
                for (int j = 0; j < it.Count; j++)
                {
                    kfs.Add(new Keyframe<float>(r.Time(it[j].StartTime), (float)it[j].Start, 2));
                    kfs.Add(new Keyframe<float>(r.Time(it[j].EndTime), (float)it[j].End, 0));
                }
                anis.Add(AnimFloat.New(kfs));
            }
            if (anis.Count == 0)
            {
                return new AnimFloat();
            }

            List<double> pts = new List<double>();
            for (int i = 0; i < anis.Count; i++)
            {
                Keyframe<float>[] kfs = anis[i].Keyframes;
                for (int j = 0; j < kfs.Length; j++)
                {
                    pts.Add(kfs[j].Time.NotNan());
                }
            }
            pts.Add(maxTime.NotNan());
            pts.Sort();
            pts = Dedup(pts);

            AnimFloat sani = (AnimFloat)AnimFloat.Chain(anis);
            sani.MapValue(delegate(float v) { return v * (float)SpeedRatio; });

            int n = pts.Count;
            for (int i = 0; i + 1 < n; i++)
            {
                double nowTime = pts[i];
                double endTime = pts[i + 1];
                sani.SetTime(nowTime);
                float speed = sani.Now();
                sani.SetTime(endTime - 1e-4);
                float endSpeed = sani.Now();
                if (Signum(speed) * Signum(endSpeed) < 0f)
                {
                    double denom = speed - endSpeed;
                    if (Math.Abs(denom) > 1e-12)
                    {
                        pts.Add((nowTime + (endTime - nowTime) * (speed / denom)).NotNan());
                    }
                }
            }
            pts.Sort();
            pts = Dedup(pts);

            List<Keyframe<float>> outKfs = new List<Keyframe<float>>();
            double height = 0.0;
            for (int i = 0; i + 1 < pts.Count; i++)
            {
                double nowTime = pts[i];
                double endTime = pts[i + 1];
                sani.SetTime(nowTime);
                float speed = sani.Now();
                // NOTE: deliberately end_time - 1e-4 (see upstream comment about Hold tweens)
                sani.SetTime(endTime - 1e-4);
                float endSpeed = sani.Now();

                if (Math.Abs(speed - endSpeed) < Consts.Eps)
                {
                    outKfs.Add(new Keyframe<float>(nowTime, (float)height, 2));
                }
                else if (Math.Abs(speed) > Math.Abs(endSpeed))
                {
                    outKfs.Add(new Keyframe<float>(nowTime, (float)height, new ClampedTween(7, 0f, 1f - endSpeed / speed)));
                }
                else
                {
                    outKfs.Add(new Keyframe<float>(nowTime, (float)height, new ClampedTween(6, speed / endSpeed, 1f)));
                }
                height += (speed + endSpeed) * (endTime - nowTime) / 2.0;
            }
            if (outKfs.Count == 0)
            {
                return new AnimFloat();
            }
            outKfs.Add(new Keyframe<float>(maxTime, (float)height, 0));
            return AnimFloat.New(outKfs);
        }

        private static List<double> Dedup(List<double> pts)
        {
            List<double> res = new List<double>(pts.Count);
            for (int i = 0; i < pts.Count; i++)
            {
                if (res.Count == 0 || res[res.Count - 1] != pts[i])
                {
                    res.Add(pts[i]);
                }
            }
            return res;
        }

        // ------------------------------------------------------------------
        // Notes
        // ------------------------------------------------------------------

        private static List<Note> ParseNotes(BpmList r, List<RpeNote> rpe, AnimFloat height, List<string> warnings, int lineId)
        {
            List<Note> notes = new List<Note>();
            for (int i = 0; i < rpe.Count; i++)
            {
                RpeNote rn = rpe[i];
                double time = r.Time(rn.StartTime);
                height.SetTime(time);
                double noteHeight = height.Now();
                double yOffset = rn.YOffset * 2.0 / Consts.RpeHeight * rn.Speed;

                NoteKind kind;
                switch (rn.Kind)
                {
                    case 1:
                        kind = NoteKind.Click();
                        break;
                    case 2:
                        {
                            double endTime = r.Time(rn.EndTime);
                            height.SetTime(endTime);
                            kind = NoteKind.Hold(endTime, height.Now());
                            break;
                        }
                    case 3:
                        kind = NoteKind.Flick();
                        break;
                    case 4:
                        kind = NoteKind.Drag();
                        break;
                    default:
                        warnings.Add("line " + lineId + ": unknown note type " + rn.Kind);
                        continue;
                }

                Note note = new Note();
                note.Kind = kind;
                note.Time = time;
                note.Height = noteHeight;
                note.Speed = rn.Speed;
                note.Above = rn.Above == 1;
                note.Fake = rn.IsFake != 0;
                note.JudgeArea = rn.JudgeArea;
                note.Color = rn.HasTint ? rn.Tint : RgbaF.White;
                note.HasFxColor = rn.HasFxTint;
                note.FxColor = rn.FxTint;
                note.HitSound = MapHitSound(rn.HitSound, kind);

                if (rn.VisibleTime >= time)
                {
                    note.Obj.Alpha = rn.Alpha >= 255 ? new AnimFloat() : AnimFloat.Fixed(rn.Alpha / 255f);
                }
                else
                {
                    float alpha = Math.Min(255, rn.Alpha) / 255f;
                    note.Obj.Alpha = AnimFloat.New(new List<Keyframe<float>>
                    {
                        new Keyframe<float>(0.0, 0f, 0),
                        new Keyframe<float>(time - rn.VisibleTime, alpha, 0)
                    });
                }
                note.Obj.Translation = new AnimVector(
                    AnimFloat.Fixed(rn.PositionX / (Consts.RpeWidth / 2f)),
                    AnimFloat.Fixed((float)yOffset));
                note.Obj.Scale = new AnimVector(AnimFloat.Fixed(rn.Size), AnimFloat.Fixed(rn.Size));
                notes.Add(note);
            }
            return notes;
        }

        private static string MapHitSound(string s, NoteKind kind)
        {
            if (string.IsNullOrEmpty(s))
            {
                return DefaultHitSound(kind);
            }
            if (s == "flick.mp3") return "flick";
            if (s == "tap.mp3") return "click";
            if (s == "drag.mp3") return "drag";
            return "click";
        }

        private static string DefaultHitSound(NoteKind kind)
        {
            switch (kind.Type)
            {
                case NoteKindType.Flick: return "flick";
                case NoteKindType.Drag: return "drag";
                default: return "click";
            }
        }

        // ------------------------------------------------------------------
        // Ctrl events
        // ------------------------------------------------------------------

        private static AnimFloat ParseCtrlEvents(List<RpeCtrlEvent> rpe)
        {
            if (rpe == null || rpe.Count == 0)
            {
                return new AnimFloat();
            }
            if (rpe.Count == 2 && rpe[0].Easing == 1 && Math.Abs(rpe[0].Value - 1.0) < 1e-4)
            {
                return new AnimFloat();
            }
            List<Keyframe<float>> kfs = new List<Keyframe<float>>();
            for (int i = 0; i < rpe.Count; i++)
            {
                ITweenFunction tw;
                if (i + 1 < rpe.Count)
                {
                    tw = Tweens.Static(RpeTweenMap.Get(Math.Max(rpe[i + 1].Easing, 1)));
                }
                else
                {
                    tw = Tweens.Static(0);
                }
                kfs.Add(new Keyframe<float>(rpe[i].X, (float)rpe[i].Value, tw));
            }
            return AnimFloat.New(kfs);
        }

        // ------------------------------------------------------------------
        // Line parsing
        // ------------------------------------------------------------------

        private static JudgeLine ParseJudgeLine(BpmList r, RpeJudgeLine rpe, double maxTime, bool modern, Dictionary<string, ITweenFunction> bm, List<string> warnings, int id)
        {
            List<RpeLayer> layers = rpe.Layers;

            AnimFloat height = modern
                ? ParseSpeedEvents(r, layers, bm, maxTime, true)
                : ParseSpeedEventsLegacy(r, layers, maxTime);
            List<Note> notes = ParseNotes(r, rpe.Notes, height, warnings, id);

            JudgeLine line = new JudgeLine();
            line.Id = id;
            line.Height = height;
            line.Notes = notes;
            line.Parent = rpe.Father;
            line.RotWithParent = rpe.RotateWithFather;
            line.ZIndex = rpe.ZOrder;
            line.ShowBelow = rpe.IsCover != 1;
            line.AttachUI = rpe.AttachUI;

            line.Obj.Alpha = EventsWithFactor(r, layers, delegate(RpeLayer l) { return l.Alpha; }, 1f / 255f, bm);
            line.Obj.Rotation = EventsWithFactor(r, layers, delegate(RpeLayer l) { return l.Rotate; }, -1f, bm);
            line.Obj.Translation = new AnimVector(
                EventsWithFactor(r, layers, delegate(RpeLayer l) { return l.MoveX; }, 2f / Consts.RpeWidth, bm),
                EventsWithFactor(r, layers, delegate(RpeLayer l) { return l.MoveY; }, 2f / Consts.RpeHeight, bm));

            bool isLinePng = rpe.Texture == "line.png";
            float scaleFactor = isLinePng ? 1f : 2f / Consts.RpeWidth;
            float scaleDefault = isLinePng ? 1f : scaleFactor;
            if (rpe.Extended != null)
            {
                float xFactor = scaleFactor;
                bool noText = rpe.Extended.Text == null || rpe.Extended.Text.Count == 0;
                if (isLinePng && noText && rpe.AttachUI == UIElement.None)
                {
                    xFactor = scaleFactor * 0.5f;
                }
                line.Obj.Scale = new AnimVector(
                    ParseScale(r, rpe.Extended.ScaleX, xFactor, scaleDefault, bm),
                    ParseScale(r, rpe.Extended.ScaleY, scaleFactor, scaleDefault, bm));
            }
            else
            {
                line.Obj.Scale = new AnimVector(AnimFloat.Fixed(scaleDefault), AnimFloat.Fixed(scaleDefault));
            }

            line.Ctrl = new CtrlObject();
            line.Ctrl.Alpha = ParseCtrlEvents(rpe.AlphaCtrl);
            line.Ctrl.Size = ParseCtrlEvents(rpe.SizeCtrl);
            line.Ctrl.Pos = ParseCtrlEvents(rpe.PosCtrl);
            line.Ctrl.Y = ParseCtrlEvents(rpe.YCtrl);

            line.Incline = rpe.Extended != null && rpe.Extended.Incline != null
                ? ParseEventsFloat(r, rpe.Extended.Incline, true, 0f, bm)
                : new AnimFloat();

            line.Kind = JudgeLineKindType.Normal;
            line.TexturePath = null;
            if (isLinePng)
            {
                if (rpe.Extended != null && rpe.Extended.Paint != null)
                {
                    line.Kind = JudgeLineKindType.Paint;
                    line.PaintAnim = ParseEventsFloat(r, rpe.Extended.Paint, true, -1f, bm);
                }
                else if (rpe.Extended != null && rpe.Extended.Text != null)
                {
                    line.Kind = JudgeLineKindType.Text;
                    line.TextAnim = ParseEventsString(r, rpe.Extended.Text, true, string.Empty, bm);
                }
                else
                {
                    line.Kind = JudgeLineKindType.Normal;
                }
            }
            else if (!string.IsNullOrEmpty(rpe.Texture))
            {
                line.Kind = JudgeLineKindType.Texture;
                line.TexturePath = rpe.Texture;
            }

            if (rpe.Extended != null && rpe.Extended.Color != null)
            {
                line.Color = ParseEventsColor(r, rpe.Extended.Color, true, RgbaF.White, bm);
            }
            else
            {
                line.Color = new AnimColor();
            }
            return line;
        }

        private static AnimFloat ParseScale(BpmList r, List<RpeEvent> opt, float factor, float def, Dictionary<string, ITweenFunction> bm)
        {
            if (opt == null || opt.Count == 0)
            {
                return AnimFloat.Fixed(def);
            }
            AnimFloat res = ParseEventsFloat(r, opt, false, 0f, bm);
            res.MapValue(delegate(float v) { return v * factor; });
            return res;
        }

        // ------------------------------------------------------------------
        // Entry point
        // ------------------------------------------------------------------

        public static Chart Parse(string source, List<string> warnings)
        {
            JsonObject root = JsonObject.Parse(source);
            JsonObject meta = JsonExt.Obj(root, "META");
            double offsetMs = meta != null ? JsonExt.Num(meta, "offset", 0.0) : 0.0;
            int rpeVersion = meta != null ? (int)JsonExt.Num(meta, "RPEVersion", 160.0) : 160;
            bool modern = rpeVersion >= 170;

            JsonArray bpmArr = JsonExt.Arr(root, "BPMList");
            List<KeyValuePair<double, double>> ranges = new List<KeyValuePair<double, double>>();
            if (bpmArr != null)
            {
                for (int i = 0; i < bpmArr.Count; i++)
                {
                    JsonObject item = JsonExt.ObjAt(bpmArr, i);
                    if (item == null) continue;
                    double bpm = JsonExt.Num(item, "bpm", 120.0);
                    Triple t = JsonExt.Triple(item, "startTime");
                    ranges.Add(new KeyValuePair<double, double>(t.Beats, bpm));
                }
            }
            if (ranges.Count == 0)
            {
                ranges.Add(new KeyValuePair<double, double>(0.0, 120.0));
            }
            BpmList bpmList = new BpmList(ranges);

            JsonArray lineArr = JsonExt.Arr(root, "judgeLineList");
            List<RpeJudgeLine> rawLines = new List<RpeJudgeLine>();
            if (lineArr != null)
            {
                for (int i = 0; i < lineArr.Count; i++)
                {
                    JsonObject o = JsonExt.ObjAt(lineArr, i);
                    if (o != null)
                    {
                        rawLines.Add(ParseRawLine(o));
                    }
                }
            }

            Dictionary<string, ITweenFunction> bezierMap = BuildBezierMap(rawLines);

            // Max time across notes and every event type, + 1s (upstream behaviour).
            double maxTime = 0.0;
            for (int i = 0; i < rawLines.Count; i++)
            {
                RpeJudgeLine l = rawLines[i];
                for (int j = 0; j < l.Notes.Count; j++)
                {
                    maxTime = Math.Max(maxTime, bpmList.Time(l.Notes[j].EndTime).NotNan());
                }
                for (int j = 0; j < l.Layers.Count; j++)
                {
                    RpeLayer ly = l.Layers[j];
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, ly.Alpha));
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, ly.MoveX));
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, ly.MoveY));
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, ly.Rotate));
                }
                if (l.Extended != null)
                {
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, l.Extended.ScaleX));
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, l.Extended.ScaleY));
                    maxTime = Math.Max(maxTime, MaxEndTime(bpmList, l.Extended.Text));
                }
            }
            maxTime += 1.0;

            Chart chart = new Chart();
            chart.SourceFormat = "RPE";
            chart.Offset = (float)(offsetMs / 1000.0);
            chart.Bpm = bpmList;
            chart.Settings = new ChartSettings();

            for (int i = 0; i < rawLines.Count; i++)
            {
                chart.Lines.Add(ParseJudgeLine(bpmList, rawLines[i], maxTime, modern, bezierMap, warnings, i));
            }

            // Guard against infinite parent recursion (upstream bails here too).
            for (int i = 0; i < chart.Lines.Count; i++)
            {
                List<int> visited = new List<int>();
                visited.Add(i);
                int guard = 0;
                JudgeLine cur = chart.Lines[i];
                while (cur.Parent >= 0 && cur.Parent < chart.Lines.Count)
                {
                    if (visited.Contains(cur.Parent))
                    {
                        warnings.Add("line " + i + ": cyclic parent relation, parent link removed");
                        cur.Parent = -1;
                        break;
                    }
                    visited.Add(cur.Parent);
                    cur = chart.Lines[cur.Parent];
                    if (++guard > chart.Lines.Count + 1)
                    {
                        break;
                    }
                }
            }

            chart.Finalize(0);
            return chart;
        }

        private static double MaxEndTime(BpmList r, List<RpeEvent> events)
        {
            double max = 0.0;
            if (events == null)
            {
                return max;
            }
            for (int i = 0; i < events.Count; i++)
            {
                max = Math.Max(max, r.Time(events[i].EndTime).NotNan());
            }
            return max;
        }

        private static Dictionary<string, ITweenFunction> BuildBezierMap(List<RpeJudgeLine> lines)
        {
            Dictionary<string, ITweenFunction> map = new Dictionary<string, ITweenFunction>();
            for (int i = 0; i < lines.Count; i++)
            {
                RpeJudgeLine l = lines[i];
                for (int j = 0; j < l.Layers.Count; j++)
                {
                    AddBeziers(map, l.Layers[j].Alpha);
                    AddBeziers(map, l.Layers[j].MoveX);
                    AddBeziers(map, l.Layers[j].MoveY);
                    AddBeziers(map, l.Layers[j].Rotate);
                    AddBeziers(map, l.Layers[j].Speed);
                }
                if (l.Extended != null)
                {
                    AddBeziers(map, l.Extended.Paint);
                    AddBeziers(map, l.Extended.ScaleX);
                    AddBeziers(map, l.Extended.ScaleY);
                    AddBeziers(map, l.Extended.Incline);
                    AddBeziers(map, l.Extended.Text);
                    AddBeziers(map, l.Extended.Color);
                }
            }
            return map;
        }

        private static void AddBeziers(Dictionary<string, ITweenFunction> map, List<RpeEvent> events)
        {
            if (events == null)
            {
                return;
            }
            for (int i = 0; i < events.Count; i++)
            {
                RpeEvent e = events[i];
                if (e.Bezier == 0)
                {
                    continue;
                }
                string key = e.BezierKeyA() + ":" + e.BezierKeyB() + ":" + e.BezierKeyC();
                if (!map.ContainsKey(key))
                {
                    map[key] = new BezierTween(
                        e.BezierPoints[0], e.BezierPoints[1],
                        e.BezierPoints[2], e.BezierPoints[3]);
                }
            }
        }

        // ------------------------------------------------------------------
        // Raw deserialization
        // ------------------------------------------------------------------

        private static RpeJudgeLine ParseRawLine(JsonObject o)
        {
            RpeJudgeLine l = new RpeJudgeLine();
            l.Name = JsonExt.Str(o, "Name", string.Empty);
            l.Texture = JsonExt.Str(o, "Texture", string.Empty);
            if (JsonExt.Has(o, "father"))
            {
                l.Father = JsonExt.Int(o, "father", -1);
            }
            l.RotateWithFather = JsonExt.Bool(o, "rotateWithFather", false);
            l.IsCover = JsonExt.Int(o, "isCover", 0);
            l.ZOrder = JsonExt.Int(o, "zOrder", 0);
            l.AttachUI = ParseUIElement(JsonExt.Str(o, "attachUI", null));

            JsonArray layers = JsonExt.Arr(o, "eventLayers");
            if (layers != null)
            {
                for (int i = 0; i < layers.Count; i++)
                {
                    JsonObject lo = JsonExt.ObjAt(layers, i);
                    if (lo == null)
                    {
                        continue;
                    }
                    RpeLayer layer = new RpeLayer();
                    layer.Alpha = ParseEventList(JsonExt.Arr(lo, "alphaEvents"));
                    layer.MoveX = ParseEventList(JsonExt.Arr(lo, "moveXEvents"));
                    layer.MoveY = ParseEventList(JsonExt.Arr(lo, "moveYEvents"));
                    layer.Rotate = ParseEventList(JsonExt.Arr(lo, "rotateEvents"));
                    layer.Speed = ParseEventList(JsonExt.Arr(lo, "speedEvents"));
                    l.Layers.Add(layer);
                }
            }

            JsonObject ext = JsonExt.Obj(o, "extended");
            if (ext != null)
            {
                RpeExtended e = new RpeExtended();
                e.Color = ParseEventList(JsonExt.Arr(ext, "colorEvents"));
                e.Text = ParseEventList(JsonExt.Arr(ext, "textEvents"));
                e.ScaleX = ParseEventList(JsonExt.Arr(ext, "scaleXEvents"));
                e.ScaleY = ParseEventList(JsonExt.Arr(ext, "scaleYEvents"));
                e.Incline = ParseEventList(JsonExt.Arr(ext, "inclineEvents"));
                e.Paint = ParseEventList(JsonExt.Arr(ext, "paintEvents"));
                l.Extended = e;
            }

            JsonArray notes = JsonExt.Arr(o, "notes");
            if (notes != null)
            {
                for (int i = 0; i < notes.Count; i++)
                {
                    JsonObject no = JsonExt.ObjAt(notes, i);
                    if (no != null)
                    {
                        l.Notes.Add(ParseRawNote(no));
                    }
                }
            }

            l.PosCtrl = ParseCtrlList(JsonExt.Arr(o, "posControl"), "pos");
            l.SizeCtrl = ParseCtrlList(JsonExt.Arr(o, "sizeControl"), "size");
            l.AlphaCtrl = ParseCtrlList(JsonExt.Arr(o, "alphaControl"), "alpha");
            l.YCtrl = ParseCtrlList(JsonExt.Arr(o, "yControl"), "y");
            return l;
        }

        private static List<RpeCtrlEvent> ParseCtrlList(JsonArray arr, string key)
        {
            List<RpeCtrlEvent> res = new List<RpeCtrlEvent>();
            if (arr == null)
            {
                return res;
            }
            for (int i = 0; i < arr.Count; i++)
            {
                JsonObject o = JsonExt.ObjAt(arr, i);
                if (o == null)
                {
                    continue;
                }
                RpeCtrlEvent e = new RpeCtrlEvent();
                e.Easing = JsonExt.Int(o, "easing", 1);
                e.X = JsonExt.Num(o, "x", 0.0);
                e.Value = JsonExt.Num(o, key, 1.0);
                res.Add(e);
            }
            return res;
        }

        private static UIElement ParseUIElement(string s)
        {
            if (string.IsNullOrEmpty(s))
            {
                return UIElement.None;
            }
            switch (s.ToLowerInvariant())
            {
                case "pause": return UIElement.Pause;
                case "combonumber": return UIElement.ComboNumber;
                case "combo": return UIElement.Combo;
                case "score": return UIElement.Score;
                case "bar": return UIElement.Bar;
                case "name": return UIElement.Name;
                case "level": return UIElement.Level;
                default: return UIElement.None;
            }
        }

        private static RpeNote ParseRawNote(JsonObject o)
        {
            RpeNote n = new RpeNote();
            n.Kind = JsonExt.Int(o, "type", 1);
            n.Above = JsonExt.Int(o, "above", 1);
            n.StartTime = JsonExt.Triple(o, "startTime");
            n.EndTime = JsonExt.Triple(o, "endTime");
            n.PositionX = JsonExt.Float(o, "positionX", 0f);
            n.YOffset = JsonExt.Float(o, "yOffset", 0f);
            n.Alpha = JsonExt.Int(o, "alpha", 255);
            n.HitSound = JsonExt.Str(o, "hitsound", null);
            n.Size = JsonExt.Float(o, "size", 1f);
            n.Speed = JsonExt.Float(o, "speed", 1f);
            n.IsFake = JsonExt.Int(o, "isFake", 0);
            n.VisibleTime = JsonExt.Num(o, "visibleTime", 999.0);
            n.JudgeArea = JsonExt.Float(o, "judgeArea", 1f);
            if (n.JudgeArea <= 0f)
            {
                n.JudgeArea = 1f;
            }

            JsonArray tint = JsonExt.Arr(o, "tint");
            if (tint != null && tint.Count >= 3)
            {
                n.HasTint = true;
                n.Tint = RgbaF.FromBytes((byte)JsonExt.AsDouble(tint[0], 255), (byte)JsonExt.AsDouble(tint[1], 255), (byte)JsonExt.AsDouble(tint[2], 255), 255);
            }
            JsonArray fxTint = JsonExt.Arr(o, "tintHitEffects");
            if (fxTint != null && fxTint.Count >= 3)
            {
                n.HasFxTint = true;
                n.FxTint = RgbaF.FromBytes((byte)JsonExt.AsDouble(fxTint[0], 255), (byte)JsonExt.AsDouble(fxTint[1], 255), (byte)JsonExt.AsDouble(fxTint[2], 255), 255);
            }
            return n;
        }

        private static List<RpeEvent> ParseEventList(JsonArray arr)
        {
            if (arr == null)
            {
                return null;
            }
            List<RpeEvent> res = new List<RpeEvent>(arr.Count);
            for (int i = 0; i < arr.Count; i++)
            {
                JsonObject o = JsonExt.ObjAt(arr, i);
                if (o == null)
                {
                    continue;
                }
                RpeEvent e = new RpeEvent();
                e.EasingLeft = JsonExt.Float(o, "easingLeft", 0f);
                e.EasingRight = JsonExt.Float(o, "easingRight", 1f);
                e.Bezier = JsonExt.Int(o, "bezier", 0);
                e.EasingType = JsonExt.Int(o, "easingType", 1);
                JsonArray bp = JsonExt.Arr(o, "bezierPoints");
                if (bp != null && bp.Count >= 4)
                {
                    for (int k = 0; k < 4; k++)
                    {
                        e.BezierPoints[k] = (float)JsonExt.AsDouble(bp[k], 0.0);
                    }
                }
                e.StartTime = JsonExt.Triple(o, "startTime");
                e.EndTime = JsonExt.Triple(o, "endTime");

                IJsonValue startV = JsonExt.Get(o, "start");
                IJsonValue endV = JsonExt.Get(o, "end");
                if (startV != null && startV.ValueType == JsonValueType.Array)
                {
                    e.IsColor = true;
                    e.StartColor = ParseColor(startV);
                    e.EndColor = ParseColor(endV);
                }
                else if (startV != null && startV.ValueType == JsonValueType.String)
                {
                    e.IsText = true;
                    e.StartText = startV.GetString();
                    e.EndText = endV != null && endV.ValueType == JsonValueType.String ? endV.GetString() : string.Empty;
                }
                else
                {
                    e.Start = JsonExt.AsDouble(startV, 0.0);
                    e.End = JsonExt.AsDouble(endV, 0.0);
                }
                res.Add(e);
            }
            return res;
        }

        private static RgbaF ParseColor(IJsonValue v)
        {
            if (v == null || v.ValueType != JsonValueType.Array)
            {
                return RgbaF.White;
            }
            JsonArray a = v.GetArray();
            if (a.Count < 3)
            {
                return RgbaF.White;
            }
            byte r = ClampByte(JsonExt.AsDouble(a[0], 255));
            byte g = ClampByte(JsonExt.AsDouble(a[1], 255));
            byte b = ClampByte(JsonExt.AsDouble(a[2], 255));
            byte alpha = a.Count >= 4 ? ClampByte(JsonExt.AsDouble(a[3], 255)) : (byte)255;
            return RgbaF.FromBytes(r, g, b, alpha);
        }

        private static byte ClampByte(double v)
        {
            if (v < 0) return 0;
            if (v > 255) return 255;
            return (byte)Math.Round(v);
        }
    }
}
