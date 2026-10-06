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

        /// <summary>
        /// Distinct URIs already logged as opened, so the log says *which* effect was attempted
        /// once rather than once per voice that happened to pick it up.
        /// </summary>
        private readonly System.Collections.Generic.HashSet<string> _openedLog =
            new System.Collections.Generic.HashSet<string>();

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
                // `Other`, matching `Player` - the last pairing that had not been tried.
                //
                // Three configurations have been tested on this device and the music was cut in
                // all three: (music GameMedia + sfx GameMedia), (music GameMedia + sfx
                // GameEffects), and (music Other + sfx GameEffects). What the move to `GameEffects`
                // could not do is remove the asymmetry, because the *effects* stream is the one
                // carrying event privilege: Microsoft documents `GameMedia` as background audio
                // that "mutes automatically", and singles out only `GameChat` as a category that
                // "will not attenuate other streams" - which says nothing reassuring about
                // `GameEffects` and a great deal about which side of the pair gets to decide.
                //
                // Leaving only the music as an ordinary stream still left an effects stream
                // opposite it. With both on `Other` neither carries background or event
                // privilege, so there is nothing for the engine to arbitrate between two peer
                // streams. This is not about loudness: the hit sounds measure 11-17 dB *below*
                // the music (click -22.5 dB, drag -28.6 dB against the track at -11.0 dB), so
                // they cannot drown it - only stop it.
                m.AudioCategory = Windows.UI.Xaml.Media.AudioCategory.Other;
                m.MediaOpened += OnVoiceOpened;
                // Without this an effect the platform refuses to open fails *silently*:
                // `MediaOpened` never fires, `_pending` stays set, and the sound simply never
                // plays with nothing anywhere to say so - which is exactly what makes "the main
                // menu lost its click sounds" undiagnosable from the log alone.
                m.MediaFailed += OnVoiceFailed;
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
        /// Reports an effect the platform would not open. Wire-only: no recovery, because there is
        /// nothing to recover *to* - the URI came from phira's own asset tree and a decode failure
        /// is a property of the file and the codec, not of this call.
        ///
        /// The point is that it is reported at all. A <see cref="MediaElement"/> that cannot open
        /// its source raises this and nothing else: `MediaOpened` stays unraised, `_pending` stays
        /// set, and `Play` has already returned believing the sound started. Silence with no log
        /// line and silence because a sound was never requested are the same picture on screen, and
        /// only this separates them - which is what makes "the main menu lost its click sounds"
        /// answerable instead of arguable.
        /// </summary>
        private void OnVoiceFailed(object sender, Windows.UI.Xaml.ExceptionRoutedEventArgs e)
        {
            MediaElement m = sender as MediaElement;
            int idx = m == null ? -1 : Array.IndexOf(_voices, m);
            if (idx >= 0 && idx < VoiceCount)
            {
                _pending[idx] = false;
            }
            AppLog.Write("SfxPlayer.MediaFailed voice=" + idx
                + " uri=" + (idx >= 0 && idx < VoiceCount ? _loaded[idx] : null)
                + " :: " + (e == null ? "(no args)" : e.ErrorMessage));
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

                    if (_openedLog.Add(uri))
                    {
                        AppLog.Write("SfxPlayer: open '" + uri + "' on voice " + idx);
                    }
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
