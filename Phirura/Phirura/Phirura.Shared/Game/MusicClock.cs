using System;
using System.Diagnostics;
using Windows.UI.Xaml.Controls;

namespace Phirura.Game
{
    /// <summary>
    /// A drift-corrected chart clock.
    ///
    /// WP8.1's MediaElement only refreshes `Position` a few times per second, which is
    /// far too coarse to drive a rhythm game. We therefore run a Stopwatch as the master
    /// clock and re-anchor it to the media position whenever the two drift apart, which
    /// keeps sub-frame accuracy without resampling the audio.
    /// </summary>
    public sealed class MusicClock
    {
        private readonly MediaElement _media;
        private readonly Stopwatch _watch = new Stopwatch();
        private double _anchorMedia;   // seconds
        private double _anchorWatch;   // seconds
        private double _frozen;
        private bool _running;
        private bool _hasAnchor;

        /// <summary>
        /// The last position the element actually reported as *moving*, or -1 before the first
        /// sample. Together with <see cref="_stalled"/> this is what tells a wedged element
        /// apart from ordinary drift - see <see cref="Resync"/>.
        /// </summary>
        private double _lastMedia = -1.0;

        /// <summary>
        /// Set when a sample found the media exactly where the previous one left it.
        ///
        /// The distinction matters because the two failures look identical from the outside
        /// (the chart and the music disagree) and want opposite corrections: ordinary drift is
        /// fixed by re-anchoring onto the media, a stall is made *worse* by it.
        /// </summary>
        private bool _stalled;

        /// <summary>
        /// How far behind the clock the element may drift before it is pushed forward.
        ///
        /// Wider than <see cref="MaxDrift"/> on purpose. The forward direction (the element is
        /// *ahead*) is free to correct - adopting it moves the chart to where the audio already
        /// is and replays nothing - so it can use a tight threshold. The backward direction
        /// costs a seek, and a seek on an element that will not take one is not a correction but
        /// a hitch, so it only happens for a gap a listener could actually hear.
        /// </summary>
        public double PushThreshold = 0.12;

        /// <summary>Minimum spacing between forward pushes, in milliseconds.</summary>
        public int PushIntervalMs = 300;

        /// <summary>How many forward pushes have been issued - the fps line reports this.</summary>
        public int Pushes;

        private int _lastPushTick = int.MinValue / 2;

        private int _lastLogTick = int.MinValue / 2;

        public MusicClock(MediaElement media)
        {
            _media = media;
        }

        /// <summary>Playback rate multiplier (nightcore / slow modes).</summary>
        public double Rate = 1.0;

        /// <summary>Seconds added to the media position to obtain the chart time.</summary>
        public double Offset;

        /// <summary>Largest tolerated drift before re-anchoring.</summary>
        public double MaxDrift = 0.04;

        public bool IsRunning
        {
            get { return _running; }
        }

        public double Now
        {
            get
            {
                if (!_running)
                {
                    return _frozen;
                }
                return _anchorMedia + (_watch.Elapsed.TotalSeconds - _anchorWatch) * Rate + Offset;
            }
        }

        /// <summary>Chart time without the user offset applied — used by the offset calibrator.</summary>
        public double RawNow
        {
            get
            {
                if (!_running)
                {
                    return _frozen - Offset;
                }
                return _anchorMedia + (_watch.Elapsed.TotalSeconds - _anchorWatch) * Rate;
            }
        }

        public void Start()
        {
            _watch.Restart();
            _anchorMedia = 0.0;
            _anchorWatch = 0.0;
            _hasAnchor = false;
            _running = true;
            _frozen = Offset;
            _media.Position = TimeSpan.Zero;
            ResetMediaTracking();
            _media.Play();
        }

        /// <summary>
        /// Starts the clock WITHOUT touching the media element.
        ///
        /// WP8.1 ignores both Position and Play() until the element has opened its source,
        /// and seeking at that moment can leave it stuck. So the chart clock is started
        /// free-running on the stopwatch and re-anchored from OnMediaOpened, once the media
        /// is genuinely open.
        /// </summary>
        public void StartFree()
        {
            _watch.Restart();
            _anchorMedia = 0.0;
            _anchorWatch = 0.0;
            _hasAnchor = true;
            _running = true;
            _frozen = Offset;
        }

        /// <summary>
        /// Re-anchors to the media's current position without seeking or playing. Used when
        /// the media finally opens after <see cref="StartFree"/>.
        /// </summary>
        public void Reanchor()
        {
            _anchorMedia = _media.Position.TotalSeconds;
            _anchorWatch = _watch.Elapsed.TotalSeconds;
            _hasAnchor = true;
        }

        public void StartAt(double seconds)
        {
            _watch.Restart();
            _anchorMedia = seconds;
            _anchorWatch = 0.0;
            _hasAnchor = true;
            _running = true;
            _frozen = seconds + Offset;
            _media.Position = TimeSpan.FromSeconds(Math.Max(0.0, seconds));
            ResetMediaTracking();
            _media.Play();
        }

        public void Pause()
        {
            _frozen = Now;
            _running = false;
            _watch.Stop();
            _media.Pause();
        }

        public void Resume()
        {
            _media.Position = TimeSpan.FromSeconds(Math.Max(0.0, _frozen - Offset));
            ResetMediaTracking();
            _watch.Restart();
            _anchorMedia = _frozen - Offset;
            _anchorWatch = 0.0;
            _hasAnchor = true;
            _running = true;
            _media.Play();
        }

        public void Seek(double chartSeconds)
        {
            _frozen = chartSeconds;
            _media.Position = TimeSpan.FromSeconds(Math.Max(0.0, chartSeconds - Offset));
            ResetMediaTracking();
            _watch.Restart();
            _anchorMedia = chartSeconds - Offset;
            _anchorWatch = 0.0;
            _hasAnchor = true;
        }

        /// <summary>Call once per frame, after the media position has been refreshed.</summary>
        ///
        /// The rule is one-way by design: a media that is *ahead* of the stopwatch wins, because
        /// that is the ordinary case this re-anchor exists for; a media that has stopped moving
        /// does not get a vote at all.
        ///
        /// That asymmetry is the whole fix for the chart stuttering. `Now` advances purely
        /// through the stopwatch term, so simply declining to re-anchor keeps it monotonic -
        /// whereas re-anchoring onto a frozen element (the previous behaviour) produced a
        /// sawtooth: the stopwatch ran ahead until it exceeded <see cref="MaxDrift"/>, was
        /// yanked back to the frozen position, ran ahead again. On device that measured as the
        /// media pinned at 10.27 s for 280 s while the engine's own clock ran from 82 s to
        /// 427 s, and as `music_position` jittering by up to 0.034 s between samples - which is
        /// the back-and-forth visible on screen. Freezing the chart alongside the media would
        /// have been the other wrong answer: the element being wedged says nothing about
        /// whether the song should stop.
        public void Resync()
        {
            if (!_running || !_hasAnchor)
            {
                return;
            }
            double media = _media.Position.TotalSeconds;
            double estimate = _anchorMedia + (_watch.Elapsed.TotalSeconds - _anchorWatch) * Rate;

            // Unchanged since the last sample. The 0.5 s allowance is for a *backwards* jump,
            // which is a seek (a restart, a scrub) and restarts the measurement rather than
            // counting as a stall - and without it a restart into an earlier part of the track
            // would be mistaken for a dead element.
            if (_lastMedia >= 0.0 && media <= _lastMedia + 0.0005 && media + 0.5 >= _lastMedia)
            {
                _stalled = true;
                LogThrottled(media, estimate, media - estimate, true);
                return;
            }

            _lastMedia = media;
            _stalled = false;

            double drift = media - estimate;
            if (drift > MaxDrift)
            {
                // Ahead: adopt it. This is free - the chart moves to where the audio already
                // is, and nothing that was played gets played twice.
                _anchorMedia = media;
                _anchorWatch = _watch.Elapsed.TotalSeconds;
            }
            else if (drift < -PushThreshold)
            {
                // Behind: the element never gets a vote, in *either* direction. The previous
                // code re-anchored the stopwatch onto it here, and that is precisely what the
                // player sees as the song playing backwards and then forwards again - the clock
                // runs on, is yanked back to wherever the element happens to be, runs on, is
                // yanked back. `Now` may therefore only move forward or jump forward, and the
                // element is the thing that gets corrected.
                //
                // The target comes from the clock and not from `media`, so it is the right
                // value whether the read above is truthful or stale: the element *should* be at
                // `estimate` either way. Rate-limited because an element that refuses the seek
                // would otherwise be handed one every frame, and each refused seek is a hitch in
                // whatever it is doing instead.
                int nowTick = Environment.TickCount;
                if (unchecked(nowTick - _lastPushTick) >= PushIntervalMs)
                {
                    _lastPushTick = nowTick;
                    Pushes++;
                    try
                    {
                        _media.Position = TimeSpan.FromSeconds(Math.Max(0.0, estimate - Offset));
                    }
                    catch (Exception)
                    {
                        // A still-wedged element can refuse the seek. The clock is already
                        // correct, so losing this is a missed correction, not a broken state.
                    }
                    LogThrottled(media, estimate, drift, false);
                }
            }
            else
            {
                LogThrottled(media, estimate, drift, false);
            }
        }

        /// <summary>
        /// One line every 250 ms of what the element reported against what the clock believes.
        ///
        /// This exists because the two readers in the shell disagreed on device by nearly two
        /// seconds - the guard saw the element at 8.66 s while this class saw it tracking the
        /// clock at 10.73 s - and neither number could be checked against the other without
        /// both being written down at the same instant. `push=` says how often the element had
        /// to be dragged forward, which is the only value here that is a verdict rather than a
        /// measurement.
        /// </summary>
        private void LogThrottled(double media, double estimate, double drift, bool stalled)
        {
            int nowTick = Environment.TickCount;
            if (unchecked(nowTick - _lastLogTick) < 250)
            {
                return;
            }
            _lastLogTick = nowTick;
            global::Phirura.AppLog.Write("MusicClock: media=" + media.ToString("0.###")
                + " clock=" + estimate.ToString("0.###")
                + " drift=" + drift.ToString("0.###")
                + (stalled ? " stalled=1" : " stalled=0")
                + " push=" + Pushes + " run=" + (_running ? 1 : 0)
                + " anchor=" + (_hasAnchor ? 1 : 0));
        }

        /// <summary>
        /// Forgets the media's movement history.
        ///
        /// Called by everything that writes <see cref="MediaElement.Position"/> directly, so a
        /// deliberate seek is never read as the element failing to advance - the two would
        /// otherwise be indistinguishable for the next sample.
        /// </summary>
        private void ResetMediaTracking()
        {
            _lastMedia = -1.0;
            _stalled = false;
        }

        /// <summary>First anchor, taken once the media reports a real position.</summary>
        public void PrimeAnchor()
        {
            if (!_hasAnchor && _running)
            {
                _anchorMedia = _media.Position.TotalSeconds;
                _anchorWatch = _watch.Elapsed.TotalSeconds;
                _hasAnchor = true;
            }
        }
    }
}
