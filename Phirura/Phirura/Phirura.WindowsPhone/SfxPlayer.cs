using System;
using Windows.UI.Xaml.Controls;

namespace Phirura
{
    /// <summary>
    /// Fires the chart's hit sounds (prpr's HitSound: click / flick / drag, plus hold -> click).
    ///
    /// WP8.1 exposes no low-latency audio API to a C#/XAML app: Windows.Media.Audio.AudioGraph
    /// does not exist on this profile, Windows.Media.Playback.MediaPlayer has no public
    /// constructor, and there is no XAudio2 binding. The only option left is a small
    /// round-robin pool of MediaElements that get rewound and restarted per hit. That costs
    /// more latency than a native mixer would, but it is the platform's ceiling rather than a
    /// shortcut, and it is what the official WP8.1 samples do.
    ///
    /// The voices are real MediaElements parented into a hidden host panel, because a
    /// MediaElement that is not in the visual tree is unreliable about actually starting.
    /// </summary>
    public sealed class SfxPlayer
    {
        private const int VoiceCount = 6;

        private readonly MediaElement[] _voices = new MediaElement[VoiceCount];
        private readonly string[] _loaded = new string[VoiceCount];

        /// <summary>
        /// Voices whose Source was just assigned and whose Play() therefore may have been
        /// dropped. WP8.1 ignores Play() until the element has opened its source, so the
        /// request is re-issued from MediaOpened. Without this the very first hit of each
        /// kind (on a fresh voice) would be silent.
        /// </summary>
        private readonly bool[] _pending = new bool[VoiceCount];
        private int _next;

        /// <summary>0..1, mirrors prpr's config.volume_sfx.</summary>
        public double Volume = 0.7;

        public bool Enabled = true;

        /// <summary>Diagnostics: how many Play() calls have been issued.</summary>
        public int Played;

        public SfxPlayer(Panel host)
        {
            for (int i = 0; i < VoiceCount; i++)
            {
                MediaElement m = new MediaElement();
                m.AutoPlay = false;
                m.IsHitTestVisible = false;
                m.AudioCategory = Windows.UI.Xaml.Media.AudioCategory.GameMedia;
                m.MediaOpened += OnVoiceOpened;
                host.Children.Add(m);
                _voices[i] = m;
                _loaded[i] = null;
                _pending[i] = false;
            }
        }

        private void OnVoiceOpened(object sender, Windows.UI.Xaml.RoutedEventArgs e)
        {
            MediaElement m = sender as MediaElement;
            if (m == null)
            {
                return;
            }
            int idx = Array.IndexOf(_voices, m);
            if (idx < 0 || !_pending[idx])
            {
                return;
            }
            _pending[idx] = false;
            try
            {
                m.Volume = Volume;
                m.Play();
            }
            catch (Exception ex)
            {
                AppLog.Write("SfxPlayer.OnVoiceOpened", ex);
            }
        }

        /// <summary>
        /// Plays <paramref name="uri"/>. Voices are handed out round-robin, so a burst of
        /// simultaneous hits steals the oldest voice instead of queueing up behind it.
        /// </summary>
        public void Play(string uri)
        {
            if (!Enabled || string.IsNullOrEmpty(uri))
            {
                return;
            }

            int idx = _next;
            _next = (_next + 1) % VoiceCount;
            MediaElement m = _voices[idx];

            try
            {
                if (_loaded[idx] != uri)
                {
                    // Assigning Source rewinds the element by itself, and seeking at the same
                    // moment is what breaks things (the media is not open yet), so only arm
                    // the retry here and leave Position alone.
                    m.Source = new Uri(uri);
                    _loaded[idx] = uri;
                    _pending[idx] = true;
                }
                else
                {
                    // Already open: rewinding here is safe and cheap.
                    _pending[idx] = false;
                    m.Position = TimeSpan.Zero;
                }
                m.Volume = Volume;
                m.Play();
                Played++;
            }
            catch (Exception ex)
            {
                // A hit sound is cosmetic; never let a media failure break the frame loop.
                AppLog.Write("SfxPlayer.Play " + uri, ex);
            }
        }

        public void Stop()
        {
            for (int i = 0; i < VoiceCount; i++)
            {
                _pending[i] = false;
                try
                {
                    _voices[i].Stop();
                }
                catch (Exception)
                {
                }
            }
        }
    }
}
