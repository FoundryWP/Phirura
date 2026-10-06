using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading.Tasks;
using Windows.Media.Core;
using Windows.Media.MediaProperties;
using Windows.Storage;
using Windows.Storage.Streams;

namespace Phirura
{
    /// <summary>
    /// One audio stream for the whole game: the chart music and every hit sound, mixed sample by
    /// sample and fed to a single <see cref="MediaStreamSource"/>. The system sees exactly one
    /// stream, which is the entire point - the WP8.1 audio session rebuilds every stream sharing
    /// a session when any one of them re-opens, and that rebuild is what knocked the music out of
    /// the speaker on every hit. With one stream there is nothing to rebuild against.
    ///
    /// Latency is the MSS buffer, roughly 50-150 ms on this device: audible against the tap, not
    /// against the note.
    /// </summary>
    public sealed class AudioMixer : IDisposable
    {
        public const int SampleRate = 44100;
        public const int Channels = 2;
        private const int BytesPerFrame = Channels * 2; // s16 stereo

        private readonly MediaStreamSource _mss;
        private MediaStreamSourceSampleRequest _pending;

        private float[] _music;
        private long _musicPos;
        private bool _musicPlaying;
        private float _musicVolume = 1.0f;
        private float _sfxVolume = 0.7f;

        private readonly List<float[]> _sfxLibrary = new List<float[]>();
        private readonly List<SfxVoice> _active = new List<SfxVoice>();
        private const int MaxVoices = 8;

        private readonly object _gate = new object();

        private struct SfxVoice
        {
            public float[] Buf;
            public int Pos;
            public float Vol;
        }

        /// <summary>Diagnostics: samples dropped because the music was shorter than the request.</summary>
        public long MusicUnderflow;

        public AudioMixer()
        {
            var props = AudioEncodingProperties.CreatePcm(SampleRate, Channels, 16);
            var descriptor = new AudioStreamDescriptor(props);
            _mss = new MediaStreamSource(descriptor);
            // A bounded buffer: large enough that a slow frame does not starve the speaker,
            // small enough that a hit is audible while it still looks connected to the note.
            _mss.BufferTime = TimeSpan.FromMilliseconds(100);
            _mss.SampleRequested += OnSampleRequested;
            _mss.Starting += OnStarting;
        }

        /// <summary>The element the page plays. AutoPlay is left to the caller.</summary>
        public MediaStreamSource Source { get { return _mss; } }

        // ------------------------------------------------------------------
        // Loading
        // ------------------------------------------------------------------

        /// <summary>
        /// Reads a WAV file into interleaved float samples, converting from s16 or f32 and
        /// downmixing or duplicating to stereo. Only PCM WAVs are accepted - anything else is a
        /// transcode the Rust side owns.
        /// </summary>
        public static async Task<float[]> LoadWavAsync(string path)
        {
            StorageFile f = await StorageFile.GetFileFromPathAsync(path);
            using (IRandomAccessStream s = await f.OpenReadAsync())
            using (var dr = new DataReader(s))
            {
                byte[] bytes = new byte[s.Size];
                await dr.LoadAsync((uint)s.Size);
                dr.ReadBytes(bytes);
                return ParseWav(bytes);
            }
        }

        /// <summary>
        /// Minimal RIFF/WAVE reader: PCM16 and IEEE float32, any channel count, resampled to the
        /// mixer's rate only by nearest-sample when the file disagrees. A chart that ships a 48
        /// kHz file plays 9% fast rather than not at all - a wrong note is diagnosable, silence
        /// is not.
        /// </summary>
        private static float[] ParseWav(byte[] b)
        {
            if (b.Length < 44 || b[0] != 'R' || b[8] != 'W')
            {
                throw new InvalidDataException("not a WAV");
            }
            int pos = 12;
            int fmtTag = 0, channels = 0, rate = 0, bits = 0;
            int dataPos = -1, dataLen = 0;
            while (pos + 8 <= b.Length)
            {
                string id = "" + (char)b[pos] + (char)b[pos + 1] + (char)b[pos + 2] + (char)b[pos + 3];
                int sz = BitConverter.ToInt32(b, pos + 4);
                if (id == "fmt ")
                {
                    fmtTag = BitConverter.ToInt16(b, pos + 8);
                    channels = BitConverter.ToInt16(b, pos + 10);
                    rate = BitConverter.ToInt32(b, pos + 12);
                    bits = BitConverter.ToInt16(b, pos + 22);
                }
                else if (id == "data")
                {
                    dataPos = pos + 8;
                    dataLen = Math.Min(sz, b.Length - dataPos);
                    break;
                }
                pos += 8 + sz + (sz & 1);
            }
            if (dataPos < 0 || channels == 0)
            {
                throw new InvalidDataException("no data chunk");
            }

            int frames = dataLen / (channels * (bits / 8));
            float[] outp = new float[frames * 2];
            for (int i = 0; i < frames; i++)
            {
                int src = dataPos + i * channels * (bits / 8);
                float l, r;
                if (fmtTag == 1 && bits == 16)
                {
                    l = BitConverter.ToInt16(b, src) / 32768f;
                    r = channels > 1 ? BitConverter.ToInt16(b, src + 2) / 32768f : l;
                }
                else if (fmtTag == 3 && bits == 32)
                {
                    l = BitConverter.ToSingle(b, src);
                    r = channels > 1 ? BitConverter.ToSingle(b, src + 4) : l;
                }
                else
                {
                    throw new InvalidDataException("unsupported WAV format " + fmtTag + "/" + bits);
                }
                outp[i * 2] = l;
                outp[i * 2 + 1] = r;
            }
            if (rate != SampleRate && rate > 0)
            {
                outp = Resample(outp, rate, SampleRate);
            }
            return outp;
        }

        private static float[] Resample(float[] src, int from, int to)
        {
            int outFrames = (int)((long)(src.Length / 2) * to / from);
            float[] dst = new float[outFrames * 2];
            for (int i = 0; i < outFrames; i++)
            {
                int s = (int)((long)i * from / to);
                dst[i * 2] = src[s * 2];
                dst[i * 2 + 1] = src[s * 2 + 1];
            }
            return dst;
        }

        // ------------------------------------------------------------------
        // Control - called from the port callbacks
        // ------------------------------------------------------------------

        public void SetMusic(float[] samples, float volume)
        {
            lock (_gate)
            {
                _music = samples;
                _musicPos = 0;
                _musicPlaying = false;
                _musicVolume = volume;
            }
        }

        public void SetMusicVolume(float volume)
        {
            lock (_gate) { _musicVolume = volume; }
        }

        public void SetSfxVolume(float volume)
        {
            lock (_gate) { _sfxVolume = volume; }
        }

        public void MusicPlay()
        {
            lock (_gate) { _musicPlaying = true; }
        }

        public void MusicPause()
        {
            lock (_gate) { _musicPlaying = false; }
        }

        public void MusicSeek(double seconds)
        {
            lock (_gate)
            {
                _musicPos = (long)(seconds * SampleRate) * 2;
                if (_music != null && _musicPos > _music.Length)
                {
                    _musicPos = _music.Length;
                }
            }
        }

        public double MusicPosition()
        {
            lock (_gate) { return _musicPos / 2.0 / SampleRate; }
        }

        public bool MusicPaused()
        {
            lock (_gate) { return !_musicPlaying; }
        }

        /// <summary>Registers a hit sound and returns the id <see cref="PlaySfx"/> takes.</summary>
        public int AddSfx(float[] samples)
        {
            lock (_gate)
            {
                _sfxLibrary.Add(samples);
                return _sfxLibrary.Count - 1;
            }
        }

        public void PlaySfx(int id, float amp)
        {
            lock (_gate)
            {
                if (id < 0 || id >= _sfxLibrary.Count)
                {
                    return;
                }
                if (_active.Count >= MaxVoices)
                {
                    _active.RemoveAt(0);
                }
                _active.Add(new SfxVoice { Buf = _sfxLibrary[id], Pos = 0, Vol = amp * _sfxVolume });
            }
        }

        // ------------------------------------------------------------------
        // The mix
        // ------------------------------------------------------------------

        private void OnStarting(MediaStreamSource sender, MediaStreamSourceStartingEventArgs args)
        {
            args.Request.SetActualStartPosition(TimeSpan.Zero);
        }

        private void OnSampleRequested(MediaStreamSource sender,
            MediaStreamSourceSampleRequestedEventArgs args)
        {
            MediaStreamSourceSampleRequest req = args.Request;
            if (req == null)
            {
                return;
            }

            const int frames = 2048;
            byte[] bytes = new byte[frames * BytesPerFrame];

            lock (_gate)
            {
                for (int i = 0; i < frames; i++)
                {
                    float l = 0f, r = 0f;

                    if (_musicPlaying && _music != null && _musicPos + 1 < _music.Length)
                    {
                        l += _music[_musicPos] * _musicVolume;
                        r += _music[_musicPos + 1] * _musicVolume;
                        _musicPos += 2;
                    }

                    for (int v = _active.Count - 1; v >= 0; v--)
                    {
                        SfxVoice sv = _active[v];
                        if (sv.Pos + 1 >= sv.Buf.Length)
                        {
                            _active.RemoveAt(v);
                            continue;
                        }
                        l += sv.Buf[sv.Pos] * sv.Vol;
                        r += sv.Buf[sv.Pos + 1] * sv.Vol;
                        sv.Pos += 2;
                        _active[v] = sv;
                    }

                    // Hard clamp rather than a limiter: the mix is two sources, not a bus, and a
                    // clamp is the whole answer to "loud hit over loud chorus" on a phone speaker.
                    if (l > 1f) l = 1f; else if (l < -1f) l = -1f;
                    if (r > 1f) r = 1f; else if (r < -1f) r = -1f;

                    short sl = (short)(l * 32767f);
                    short sr = (short)(r * 32767f);
                    bytes[i * 4] = (byte)(sl & 0xff);
                    bytes[i * 4 + 1] = (byte)(sl >> 8);
                    bytes[i * 4 + 2] = (byte)(sr & 0xff);
                    bytes[i * 4 + 3] = (byte)(sr >> 8);
                }
            }

            var sample = MediaStreamSample.CreateFromBuffer(bytes.AsBuffer(),
                TimeSpan.FromTicks(0));
            sample.Duration = TimeSpan.FromSeconds((double)frames / SampleRate);
            req.Sample = sample;
        }

        public void Dispose()
        {
            lock (_gate)
            {
                _music = null;
                _active.Clear();
            }
        }
    }
}