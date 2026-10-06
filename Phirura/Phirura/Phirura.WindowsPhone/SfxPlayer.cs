using System;
using System.Collections.Generic;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media;

namespace Phirura
{
    /// <summary>
    /// Fires the chart's hit sounds (prpr's HitSound: click / flick / drag, plus hold -> click)
    /// and phira's UI sounds.
    ///
    /// WP8.1 exposes no low-latency audio API to a C#/XAML app: Windows.Media.Audio.AudioGraph is
    /// Windows 10 only (it is not in the WP8.1 Windows.winmd at all), MediaPlayer has no usable
    /// public surface for this, and XAudio2 - though the WP8.1 SDK ships xaudio2.h and
    /// lib\ARM\XAudio2.lib - would mean a second native DLL and a new import on the shipped one.
    /// So the voices are MediaElements, and the design constraint is what they cost.
    ///
    /// THE RULE THIS CLASS OBEYS: a voice is bound to one sound for its whole life, and its
    /// `Source` is assigned once, at construction. Nothing assigns a Source during play.
    ///
    /// That is not an optimisation, it is the fix for two measured failures. On device, assigning
    /// `Source` mid-chart produced `SfxPlayer.MediaFailed ... MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED`
    /// and ten `MediaElement.Play()` calls throwing E_UNEXPECTED (0x8000FFFF) inside twenty
    /// seconds of play, and every one of those was a sound that never came out. It is also the
    /// latency: a hit that has to open a file first waits for `MediaOpened` before it can play,
    /// which is hundreds of milliseconds on this device - audible as the hit sound arriving after
    /// the note. A voice that is already open starts from `Position = 0` in one call, and the
    /// round-robin over voices *bound to that sound* keeps a burst from queueing behind itself.
    ///
    /// The voices are real MediaElements parented into a hidden host panel, because a
    /// MediaElement that is not in the visual tree is unreliable about actually starting.
    /// </summary>
    public sealed class SfxPlayer
    {
        /// <summary>
        /// Every sound this class can be asked for, and how many overlapping voices each gets.
        ///
        /// The counts are per sound rather than a shared pool, which is the whole point: a shared
        /// pool hands out whichever voice is next and then has to re-target it, and re-targeting
        /// is the expensive and destructive operation. Three for `click` because it is the only
        /// sound that arrives in bursts (click and hold both use it); two for the longer drag and
        /// flick; one for everything that plays at most a few times a minute.
        ///
        /// A sound that is not in this table still works - `BindVoice` grows the table on
        /// demand - it just pays one Source assignment the first time, which is the cost this
        /// table exists to move out of the chart.
        /// </summary>
        private static readonly string[] Preload = new string[]
        {
            "ms-appx:///Assets/game/click.wav",
            "ms-appx:///Assets/game/drag.wav",
            "ms-appx:///Assets/game/flick.wav",
            "ms-appx:///Assets/game/cali_hit.wav",
            "ms-appx:///Assets/game/button.wav",
            "ms-appx:///Assets/game/button_large.wav",
            "ms-appx:///Assets/game/switch.wav"
        };

        private static readonly int[] Plan = new int[] { 3, 2, 2, 1, 1, 1, 1 };

        /// <summary>Hard ceiling on voices, so a caller looping over unexpected URIs cannot build
        /// an unbounded number of media pipelines.</summary>
        private const int MaxVoices = 16;

        private readonly List<MediaElement> _voices = new List<MediaElement>();
        private readonly List<string> _uriOf = new List<string>();

        /// <summary>True once the element has raised `MediaOpened`. `Play` before that is
        /// ignored by the platform, which is what `_pending` exists to recover from.</summary>
        private readonly List<bool> _open = new List<bool>();

        /// <summary>
        /// Set when the platform has taken a voice out of service - `MediaFailed`, or a `Play`
        /// that threw. A dead voice is skipped by `Pick` forever rather than hammered: on device
        /// one broken `click.wav` voice threw E_UNEXPECTED ten times in fourteen seconds, and
        /// every one of those was an attempt to play a sound the engine had already refused.
        /// </summary>
        private readonly List<bool> _dead = new List<bool>();

        private readonly Dictionary<string, List<int>> _ring = new Dictionary<string, List<int>>();
        private readonly Dictionary<string, int> _cursor = new Dictionary<string, int>();

        /// <summary>Voices whose Source was assigned and whose Play() may have been dropped
        /// because the element had not opened yet. Re-issued from `MediaOpened`.</summary>
        private readonly List<bool> _pending = new List<bool>();

        /// <summary>Distinct one-line reports already written, so a persistent condition is
        /// logged once instead of once per hit.</summary>
        private readonly HashSet<string> _saidOnce = new HashSet<string>();

        private Panel _host;

        /// <summary>0..1, mirrors prpr's config.volume_sfx.</summary>
        public double Volume = 0.7;

        public bool Enabled = true;

        /// <summary>Diagnostics: how many Play() calls have been issued.</summary>
        public int Played;

        // Logged once, on the first hit that actually took the mixer. The second reading matters
        // more than the first: by then the chart has decoded its PCM, so the clip and voice counts
        // say whether the samples reached the mixer at all rather than merely that it started.
        private bool _mixDiagLogged;

        /// <summary>Diagnostics: how many voices the platform has taken out of service.</summary>
        public int Dead;

        /// <summary>
        /// The music element, for watching what a hit does to it. Set once by the page that owns
        /// both; read-only here, because the question "did the sound cut the music" is answered
        /// by the music's own state across a hit, not by anything this class does to it.
        /// </summary>
        public MediaElement Music;

        /// <summary>
        /// How many times a <c>Play</c> found the music in a different state than the call
        /// before it. The comparison is the point: a state that never changes while sounds fire
        /// rules out "the sound rebuilt the stream"; one that flips on every hit is the glitch.
        /// </summary>
        public int MusicStateChanges;

        private MediaElementState _lastMusicState = MediaElementState.Closed;

        /// <summary>
        /// Whether the music was playing when the last hit landed. The resume-on-glitch only
        /// fires when the hit *changed* the state - a music the player paused on purpose is left
        /// paused, and one that was already stopped is left stopped.
        /// </summary>
        private bool _musicWasPlaying;

        public SfxPlayer(Panel host)
        {
            _host = host;
            for (int i = 0; i < Preload.Length && i < Plan.Length; i++)
            {
                for (int k = 0; k < Plan[i]; k++)
                {
                    BindVoice(Preload[i]);
                }
            }
            AppLog.Write("SfxPlayer: preloaded " + _voices.Count + " voices for "
                + _ring.Count + " sounds (no Source is assigned during play from here on)");
        }

        /// <summary>
        /// Creates one voice permanently bound to <paramref name="uri"/> and adds it to that
        /// sound's ring. Returns its index, or -1 if <see cref="MaxVoices"/> has been reached.
        ///
        /// Bound for life: the alternative is the old design, where six shared voices were handed
        /// out round-robin and re-targeted whenever the next hit happened to be a different
        /// sound - which is exactly the mid-game `Source` assignment that broke on device.
        /// </summary>
        private int BindVoice(string uri)
        {
            if (_voices.Count >= MaxVoices)
            {
                SayOnce("full", "SfxPlayer: voice ceiling " + MaxVoices + " reached, cannot bind '"
                    + uri + "'");
                return -1;
            }

            int idx = _voices.Count;
            MediaElement m = new MediaElement();
            m.AutoPlay = false;
            m.IsHitTestVisible = false;

            // `Other`, matching `Player`. Four pairings have now been tested on this device and
            // the music was cut in all of them: (music GameMedia + sfx GameMedia),
            // (music GameMedia + sfx GameEffects), (music Other + sfx GameEffects) and
            // (both Other). The category was never the variable - the number of *Source
            // assignments happening while the chart plays* was, and removing them is what this
            // class now does. See the type doc for the measured evidence.
            m.AudioCategory = AudioCategory.Other;
            m.MediaOpened += OnVoiceOpened;
            // Without this an effect the platform refuses to open fails *silently*: `MediaOpened`
            // never fires, `_pending` stays set, and the sound simply never plays with nothing
            // anywhere to say so - which is what made "the main menu lost its click sounds"
            // undiagnosable from the log alone.
            m.MediaFailed += OnVoiceFailed;
            _host.Children.Add(m);

            _voices.Add(m);
            _uriOf.Add(uri);
            _open.Add(false);
            _dead.Add(false);
            _pending.Add(false);

            List<int> ring;
            if (!_ring.TryGetValue(uri, out ring))
            {
                ring = new List<int>();
                _ring[uri] = ring;
                _cursor[uri] = 0;
            }
            ring.Add(idx);

            try
            {
                // The one and only Source assignment this voice will ever see. It happens at
                // construction - page load, seconds before the chart - so the open, and any
                // failure it might produce, is off the critical path of the first hit.
                m.Source = new Uri(uri);
            }
            catch (Exception ex)
            {
                _dead[idx] = true;
                Dead++;
                AppLog.Write("SfxPlayer.bind voice=" + idx + " " + uri, ex);
            }
            return idx;
        }

        private void OnVoiceOpened(object sender, Windows.UI.Xaml.RoutedEventArgs e)
        {
            int idx = IndexOf(sender);
            if (idx < 0)
            {
                return;
            }
            _open[idx] = true;
            int ready = 0;
            int dead = 0;
            for (int i = 0; i < _voices.Count; i++)
            {
                if (_open[i]) ready++;
                if (_dead[i]) dead++;
            }
            AppLog.Write("SfxPlayer: opened voice=" + idx + " " + _uriOf[idx]
                + " ready=" + ready + "/" + _voices.Count + " dead=" + dead);
            if (!_pending[idx])
            {
                return;
            }
            _pending[idx] = false;
            Start(idx);
        }

        /// <summary>
        /// Reports an effect the platform would not open. Wire-only: no recovery, because there
        /// is nothing to recover *to* - the URI came from phira's own asset tree and a decode
        /// failure is a property of the file and the codec, not of this call.
        ///
        /// The point is that it is reported at all, and that the voice is retired rather than
        /// retried. A <see cref="MediaElement"/> that cannot open its source raises this and
        /// nothing else: `MediaOpened` stays unraised, `_pending` stays set, and `Play` has
        /// already returned believing the sound started. Silence with no log line and silence
        /// because a sound was never requested are the same picture on screen, and only this
        /// separates them.
        /// </summary>
        private void OnVoiceFailed(object sender, Windows.UI.Xaml.ExceptionRoutedEventArgs e)
        {
            int idx = IndexOf(sender);
            if (idx >= 0 && !_dead[idx])
            {
                _dead[idx] = true;
                Dead++;
                _open[idx] = false;
            }
            AppLog.Write("SfxPlayer.MediaFailed voice=" + idx
                + " uri=" + (idx >= 0 ? _uriOf[idx] : null) + " dead=" + Dead
                + " :: " + (e == null ? "(no args)" : e.ErrorMessage));
        }

        private int IndexOf(object sender)
        {
            MediaElement m = sender as MediaElement;
            if (m == null)
            {
                return -1;
            }
            for (int i = 0; i < _voices.Count; i++)
            {
                if (ReferenceEquals(_voices[i], m))
                {
                    return i;
                }
            }
            return -1;
        }

        private void SayOnce(string key, string line)
        {
            if (_saidOnce.Add(key))
            {
                AppLog.Write(line);
            }
        }

        /// <summary>
        /// Picks the next voice for <paramref name="uri"/>, skipping any the platform has retired.
        ///
        /// Round-robin *within the sound's own ring*, never across sounds - that is what keeps a
        /// burst from queueing behind the voice that is still finishing the previous hit, while
        /// still guaranteeing that no hit ever has to retarget a Source.
        /// </summary>
        private int Pick(string uri)
        {
            List<int> ring;
            if (!_ring.TryGetValue(uri, out ring))
            {
                // Not in the preload table. Binding now costs one Source assignment mid-game -
                // the thing this class exists to avoid - but doing it once for a sound nobody
                /// anticipated beats staying silent for it for the rest of the session.
                if (BindVoice(uri) < 0)
                {
                    return -1;
                }
                if (!_ring.TryGetValue(uri, out ring))
                {
                    return -1;
                }
            }

            int n = ring.Count;
            int start = 0;
            _cursor.TryGetValue(uri, out start);
            for (int k = 0; k < n; k++)
            {
                int cand = ring[(start + k) % n];
                if (!_dead[cand])
                {
                    _cursor[uri] = (start + k + 1) % n;
                    return cand;
                }
            }
            SayOnce("dead:" + uri, "SfxPlayer: every voice for '" + uri
                + "' is dead - the sound is disabled for this session");
            return -1;
        }

        private void Start(int idx)
        {
            MediaElement m = _voices[idx];
            if (_open[idx])
            {
                // Rewind only once the element is genuinely open. Seeking an element that has not
                // opened its source is the operation that wedges it, and a voice that has just
                // finished a sound is at its end - where `Play()` alone would do nothing.
                try
                {
                    m.Position = TimeSpan.Zero;
                }
                catch (Exception ex)
                {
                    _dead[idx] = true;
                    Dead++;
                    AppLog.Write("SfxPlayer.rewind voice=" + idx, ex);
                    return;
                }
            }
            m.Volume = Volume;
            m.Play();
        }

        /// <summary>
        /// Plays <paramref name="uri"/>.
        /// </summary>
        public void Play(string uri)
        {
            if (!Enabled || string.IsNullOrEmpty(uri))
            {
                return;
            }

            // XAudio2 first, and the reason is the whole point of it: a hit sound that goes through
            // the mixer never touches the platform's media session, so it cannot silence the music
            // the way opening a source for a MediaElement did - and it starts in one command
            // instead of opening a file first, which is where the delay came from. The MediaElement
            // path below stays as the fallback for a sound whose PCM was never ingested (the
            // chart's click/drag/flick do not exist until the chart has loaded) and for a phone
            // where XAudio2 is absent at all.
            if (PrprPort.MixReady && PrprPort.MixSfxPlay(uri, (float)Volume))
            {
                Played++;
                if (!_mixDiagLogged)
                {
                    _mixDiagLogged = true;
                    AppLog.Write(PrprPort.MixDiag() + " (first mixer hit, voice=" + uri + ")");
                }
                return;
            }

            // The state the music is in when a hit lands. Recorded rather than asserted: whether
            // the hit *changed* it is the whole question, and that is answered across the call,
            // not from one side of it.
            MediaElementState musicBefore =
                Music == null ? MediaElementState.Closed : Music.CurrentState;
            _musicWasPlaying = musicBefore == MediaElementState.Playing;

            int idx = Pick(uri);
            if (idx < 0)
            {
                return;
            }

            try
            {
                // Not open yet: `Play()` would be dropped by the platform, so arm the retry that
                // `OnVoiceOpened` picks up. This only happens for a hit that lands before the
                // preload has finished, which on device means the first hit of the session.
                _pending[idx] = !_open[idx];
                Start(idx);
                Played++;

                if (Music != null)
                {
                    MediaElementState musicAfter = Music.CurrentState;
                    if (musicAfter != _lastMusicState)
                    {
                        MusicStateChanges++;
                        AppLog.Write("SfxPlayer: music state " + _lastMusicState + " -> " + musicAfter
                            + " on hit " + Played + " (was " + musicBefore + ")");
                        _lastMusicState = musicAfter;
                    }
                    // The hit just knocked the music out of Playing and nobody asked it to: on
                    // this platform a sfx voice's open/seek rebuilds the shared audio session and
                    // the music element comes back Paused. Nothing else will ever restart it -
                    // the next hit is the only other writer and it does not know the music was
                    // running - so the fix lives here, one frame of silence rather than the rest
                    // of the song.
                    if (_musicWasPlaying
                        && musicBefore == MediaElementState.Playing
                        && musicAfter != MediaElementState.Playing)
                    {
                        AppLog.Write("SfxPlayer: hit knocked music to " + musicAfter + ", resuming");
                        try
                        {
                            Music.Play();
                        }
                        catch (Exception ex)
                        {
                            AppLog.Write("SfxPlayer: music resume", ex);
                        }
                    }
                }
            }
            catch (Exception ex)
            {
                // A hit sound is cosmetic; never let a media failure break the frame loop. The
                // voice is retired rather than retried: on device one broken voice threw this
                // ten times in fourteen seconds, and the sound came out none of those times.
                _dead[idx] = true;
                Dead++;
                AppLog.Write("SfxPlayer.Play voice=" + idx + " " + uri + " dead=" + Dead, ex);
            }
        }

        public void Stop()
        {
            for (int i = 0; i < _voices.Count; i++)
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
