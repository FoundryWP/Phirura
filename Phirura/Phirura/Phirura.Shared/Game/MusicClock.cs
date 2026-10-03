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
            _watch.Restart();
            _anchorMedia = chartSeconds - Offset;
            _anchorWatch = 0.0;
            _hasAnchor = true;
        }

        /// <summary>Call once per frame, after the media position has been refreshed.</summary>
        public void Resync()
        {
            if (!_running || !_hasAnchor)
            {
                return;
            }
            double media = _media.Position.TotalSeconds;
            double estimate = _anchorMedia + (_watch.Elapsed.TotalSeconds - _anchorWatch) * Rate;
            double drift = media - estimate;
            if (Math.Abs(drift) > MaxDrift)
            {
                _anchorMedia = media;
                _anchorWatch = _watch.Elapsed.TotalSeconds;
            }
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
