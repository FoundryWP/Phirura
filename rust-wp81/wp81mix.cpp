// wp81mix.cpp - the XAudio2 mixer that both the music and the hit sounds go through.
//
// WHY THIS EXISTS, and why it is C++ instead of C# or Rust:
//
//   On this device a hit sound played through a MediaElement could stop the music. The two are
//   separate media pipelines competing for the same platform session, and the measurements were
//   unambiguous: the music element sat in `Playing` while its position stood still, and the
//   sounds themselves arrived late because a hit had to *open a file* before it could start.
//   WP8.1 gives a C#/XAML app no way to mix - Windows.Media.Audio.AudioGraph is Windows 10 only
//   and is not in the WP8.1 winmd at all - so the only mixer on this platform is XAudio2, which
//   the WP8.1 SDK ships (Include\xaudio2.h, lib\ARM\XAudio2.lib, and the header is guarded by
//   WINAPI_PARTITION_APP, i.e. it is meant for apps, not just for desktop).
//
//   XAudio2 mixes PCM. It cannot decode. It does not have to: prpr's sasa shim already runs
//   symphonia over every asset and hands the decoded interleaved f32 to `wp81_audio_pcm`, which
//   used to be a no-op that threw it away. This file is the other half of that contract.
//
//   The header is used for the *interface declarations* only. `xaudio2_8.dll` is resolved at
//   runtime with LoadLibraryExW + GetProcAddress (LoadLibraryW does not exist on WP8.1 - see
//   rust-wp81/compat), so the finished DLL's import table gains nothing: if the audio device is
//   unavailable we simply report not-ready and the shell keeps its MediaElement path. A link-time
//   dependency here would mean the whole application failing to load on a phone without it, which
//   is a far worse failure than silence we can log.
//
// CONSTRAINTS this file must respect:
//   * /NODEFAULTLIB - no CRT. No malloc, memcpy, memset, strlen, new, exceptions or RTTI. Every
//     byte of memory comes from HeapAlloc, and every copy is a loop. The build fails loudly at
//     link time if this is broken, which is why the risk is acceptable.
//   * Everything runs on the UI thread; XAudio2's voice methods are thread-safe, but keeping a
//     single owner removes the need to prove it.
//   * No std::, no static initialisers with constructors.

#include <windows.h>
#include <xaudio2.h>

// LoadLibraryExW and GetProcAddress come from <libloaderapi.h>, which windows.h reaches through
// winbase.h - declaring them again here was tried and only produced C4273 for inconsistent dll
// linkage, because the SDK marks them dllimport.
//
// The pair matters more than it looks. LoadLibraryW is *declared* in winbase.h, yet no import
// library on this platform provides it, so code written against it compiles and then fails to
// link; LoadLibraryExW comes from WindowsPhoneCore.lib, which is already on the link line, and it
// is the same pair rust-wp81/compat uses to reach bcrypt. This is also what keeps xaudio2_8.dll
// out of the import table: the module is opened at run time, so a phone without it makes this
// file report not-ready instead of making the whole application fail to load.

// --------------------------------------------------------------------------
// dynamic XAudio2 entry point
// --------------------------------------------------------------------------

// The export really takes three arguments (Flags, then the processor mask). Calling it through a
// two-argument typedef would leave the third argument holding whatever happened to be in r2, which
// on ARM is not "the default processor" - it is whatever the previous call left behind.
typedef HRESULT (WINAPI *PFN_XAudio2Create)(IXAudio2 **ppXAudio2, UINT32 Flags,
                                            XAUDIO2_PROCESSOR Processor);

static PFN_XAudio2Create s_create = 0;
static IXAudio2 *s_xa = 0;
static IXAudio2MasteringVoice *s_master = 0;
static int s_initState = 0;   // 0 = never tried, 1 = ready, -1 = failed
static int s_initError = 0;   // the HRESULT, or the step that failed

static void *loadProc(const wchar_t *dll, const char *name)
{
    void *h = LoadLibraryExW(dll, 0, 0);
    if (!h) {
        return 0;
    }
    return (void *)GetProcAddress((HMODULE)h, name);
}

// Idempotent. Called from wp81_mix_ready, so the first thing the shell asks is enough.
static int ensureEngine(void)
{
    if (s_initState == 1) {
        return 1;
    }
    if (s_initState == -1) {
        return 0;
    }
    s_initState = -1;

    s_create = (PFN_XAudio2Create)loadProc(L"XAudio2_8.dll", "XAudio2Create");
    if (!s_create) {
        s_initError = 1;              // the DLL or the export is not there
        return 0;
    }
    HRESULT hr = s_create(&s_xa, 0, XAUDIO2_DEFAULT_PROCESSOR);
    if (FAILED(hr) || !s_xa) {
        s_initError = (int)hr;
        return 0;
    }
    hr = s_xa->CreateMasteringVoice(&s_master, XAUDIO2_DEFAULT_CHANNELS,
                                    XAUDIO2_DEFAULT_SAMPLERATE, 0, NULL, 0,
                                    AudioCategory_GameMedia);
    if (FAILED(hr) || !s_master) {
        s_initError = (int)hr;
        s_xa->Release();
        s_xa = 0;
        return 0;
    }
    s_initState = 1;
    s_initError = 0;
    return 1;
}

// --------------------------------------------------------------------------
// the clip library
// --------------------------------------------------------------------------

#define MIX_KEY_CAP 96
#define MIX_MAX_CLIPS 32
#define MIX_SFX_SLOTS 3

struct MixClip
{
    char key[MIX_KEY_CAP];
    int  keyLen;
    unsigned rate;
    unsigned ch;
    unsigned long long frames;
    short *pcm;            // interleaved i16, the format XAudio2 is fed
    long  inUse;
};

static MixClip s_clips[MIX_MAX_CLIPS];
static int s_clipCount = 0;
static unsigned long long s_pcmBytes = 0;

// The key is compared as "the part after the last slash, up to the last dot", so
// `button_large.ogg`, `ms-appx:///Assets/game/button_large.wav` and `button_large` are the same
// sound. That is not tidiness: Rust names assets the way the chart packs them and the shell names
// them the way the app package stores them, and a hit sound that fails to match silently
// disappears - which is exactly the class of bug this whole file is meant to remove.
static int keyNorm(const char *key, int keyLen, char *out, int cap)
{
    if (!key) {
        return 0;
    }
    if (keyLen < 0) {
        // A negative length means "NUL-terminated", which is how a P/Invoked `string` reaches us
        // from C#. Counting it here rather than making the caller do it keeps the two sides from
        // ever disagreeing about the byte length of a name that is pure ASCII either way.
        int n = 0;
        while (n < MIX_KEY_CAP && key[n]) {
            n++;
        }
        keyLen = n;
    }
    int start = 0;
    int i;
    for (i = 0; i < keyLen; i++) {
        if (key[i] == '/' || key[i] == '\\') {
            start = i + 1;
        }
    }
    int end = keyLen;
    for (i = keyLen - 1; i > start; i--) {
        if (key[i] == '.') {
            end = i;
            break;
        }
    }
    int len = end - start;
    if (len > cap - 1) {
        len = cap - 1;
    }
    if (len < 0) {
        len = 0;
    }
    for (i = 0; i < len; i++) {
        out[i] = key[start + i];
    }
    out[len] = 0;
    return len;
}

static int keySame(const MixClip *c, const char *norm, int normLen)
{
    if (c->keyLen != normLen) {
        return 0;
    }
    int i;
    for (i = 0; i < normLen; i++) {
        if (c->key[i] != norm[i]) {
            return 0;
        }
    }
    return 1;
}

static MixClip *findClip(const char *norm, int normLen)
{
    int i;
    for (i = 0; i < s_clipCount; i++) {
        if (keySame(&s_clips[i], norm, normLen)) {
            return &s_clips[i];
        }
    }
    return 0;
}

// --------------------------------------------------------------------------
// memory - HeapAlloc only, there is no CRT here
// --------------------------------------------------------------------------

static void *mixAlloc(unsigned long long bytes)
{
    if (bytes == 0 || bytes > 0x40000000ULL) {
        return 0;
    }
    return HeapAlloc(GetProcessHeap(), 0, (SIZE_T)bytes);
}

static void mixFree(void *p)
{
    if (p) {
        HeapFree(GetProcessHeap(), 0, p);
    }
}

// --------------------------------------------------------------------------
// voices
// --------------------------------------------------------------------------

struct MixVoice
{
    IXAudio2SourceVoice *voice;
    MixClip *clip;
    XAUDIO2_BUFFER buf;
};

static MixVoice s_music;
static MixVoice s_sfx[MIX_MAX_CLIPS][MIX_SFX_SLOTS];
static int s_voiceCount = 0;

static WAVEFORMATEX clipFormat(const MixClip *c)
{
    WAVEFORMATEX w;
    w.wFormatTag = WAVE_FORMAT_PCM;
    w.nChannels = (WORD)c->ch;
    w.nSamplesPerSec = c->rate;
    w.wBitsPerSample = 16;
    w.nBlockAlign = (WORD)(c->ch * 2);
    w.nAvgBytesPerSec = c->rate * w.nBlockAlign;
    w.cbSize = 0;
    return w;
}

static void buildBuffer(XAUDIO2_BUFFER *b, const MixClip *c)
{
    unsigned long long bytes = c->frames * c->ch * 2;
    b->Flags = XAUDIO2_END_OF_STREAM;
    b->AudioBytes = (UINT32)bytes;
    // pAudioData is a const BYTE*, and C++ will not quietly reinterpret a short* into it.
    b->pAudioData = (const BYTE *)c->pcm;
    b->PlayBegin = 0;
    b->PlayLength = 0;
    b->LoopBegin = 0;
    b->LoopLength = 0;
    b->LoopCount = 0;
    // The field is pContext, not Context - XAUDIO2_BUFFER uses the p-prefix for the one pointer
    // that comes back through IXAudio2VoiceCallback, and this is the only member that does.
    b->pContext = 0;
}

// --------------------------------------------------------------------------
// music state
// --------------------------------------------------------------------------

static double s_musicAmp = 1.0;
static double s_musicRate = 1.0;
static bool s_musicLoop = false;
static bool s_musicPlaying = false;
static long s_musicStarted = 0;

// XAudio2 2.8 as shipped on WP8.1 gives IXAudio2SourceVoice no SetSourcePosition/GetSourcePosition
// - the only way to ask where the voice is comes back through XAUDIO2_VOICE_STATE::SamplesPlayed,
// the count of samples the voice has actually produced. That is a better clock than
// MediaElement.Position ever was: it cannot stand still for 280 seconds while the shell watches,
// because it is the mixer reporting its own progress rather than a pipeline that may have wedged.
static unsigned long long s_samplesBase = 0;
static double s_posSeconds = 0.0;

// The loop window, remembered against the *whole* track. Seeking shortens the buffer that gets
// submitted, so the window has to be re-expressed in the shortened view or the chart would start
// looping over the wrong seconds after a jump.
static UINT32 s_loopBegin = 0;
static UINT32 s_loopLength = 0;
static UINT32 s_loopCount = 0;

// A fade has to be animated by someone, and XAudio2 only sets volume instantly. Rather than run a
// timer thread (which would need a critical section and a shutdown protocol), the ramp is applied
// by whichever side polls `position` - Rust and the shell both do that every frame anyway, so the
// fade costs nothing and stays on the thread that owns the rest of this state.
static bool s_fadeActive = false;
static double s_fadeSeconds = 0.0;
static bool s_fadeOut = false;
static ULONGLONG s_fadeStartMs = 0;
static float s_fadeFrom = 1.0f;

static void applyFade()
{
    if (!s_fadeActive || !s_music.voice) {
        return;
    }
    ULONGLONG now = GetTickCount64();
    double t = 0.0;
    if (s_fadeSeconds > 0.0) {
        t = (double)(now - s_fadeStartMs) / 1000.0 / s_fadeSeconds;
        if (t >= 1.0) {
            t = 1.0;
        }
    }
    float target = s_fadeOut ? 0.0f : (float)s_musicAmp;
    float v = s_fadeFrom + (target - s_fadeFrom) * (float)t;
    s_music.voice->SetVolume(v, XAUDIO2_COMMIT_NOW);
    if (t >= 1.0) {
        s_fadeActive = false;
        s_musicAmp = s_fadeOut ? 0.0 : s_musicAmp;
    }
}

// Where the voice is right now, made the reference point for the next read.
static void musicBaseline(void)
{
    XAUDIO2_VOICE_STATE st;
    st.pCurrentBufferContext = 0;
    st.BuffersQueued = 0;
    st.SamplesPlayed = 0;
    if (s_music.voice) {
        s_music.voice->GetState(&st, 0);
        s_samplesBase = st.SamplesPlayed;
    }
}

// Hand XAudio2 the music starting at `fromSec`. This SDK version has no in-place seek, so a jump
// is a flush plus one SubmitSourceBuffer over memory that is already resident: no file, no
// decoder, no pipeline rebuild. It is still an order of magnitude cheaper than a MediaElement
// seek, which re-opens a source and is the operation that used to take the music down.
static int musicSubmit(double fromSec)
{
    MixClip *c = s_music.clip;
    if (!c || !c->pcm || !s_music.voice) {
        return 1;
    }
    if (fromSec < 0.0) fromSec = 0.0;
    unsigned long long f = (unsigned long long)(fromSec * (double)c->rate);
    if (f > c->frames) f = c->frames;
    if (c->frames - f < 1) {
        f = 0;   // XAudio2 rejects an empty buffer outright, so never hand it one
    }

    XAUDIO2_BUFFER b;
    buildBuffer(&b, c);
    b.pAudioData = (const BYTE *)(c->pcm + f * c->ch);
    b.AudioBytes = (UINT32)((c->frames - f) * c->ch * 2);
    b.LoopCount = s_loopCount;
    b.LoopBegin = 0;
    b.LoopLength = 0;
    if (s_loopCount != 0 && s_loopLength != 0) {
        unsigned long long lb = s_loopBegin;
        unsigned long long ll = s_loopLength;
        unsigned long long nb = (lb > f) ? (lb - f) : 0;
        unsigned long long ne = (lb + ll > f) ? (lb + ll - f) : 0;
        if (ne < nb) ne = nb;
        b.LoopBegin = (UINT32)nb;
        b.LoopLength = (UINT32)(ne - nb);
        if (b.LoopLength == 0) b.LoopCount = 0;
    }

    s_music.voice->Stop(0, XAUDIO2_COMMIT_NOW);
    s_music.voice->FlushSourceBuffers();
    HRESULT hr = s_music.voice->SubmitSourceBuffer(&b, 0);
    s_music.buf = b;
    s_posSeconds = (double)f / (double)c->rate;
    musicBaseline();
    if (FAILED(hr)) {
        return 2;
    }
    return 0;
}

// --------------------------------------------------------------------------
// exported surface
// --------------------------------------------------------------------------

extern "C" {

int wp81_mix_ready(void)
{
    return ensureEngine();
}

// 0 ok, 1 no XAudio2, 2 no mastering voice; the raw value in s_initError.
int wp81_mix_init_status(void)
{
    ensureEngine();
    if (s_initState == 1) {
        return 0;
    }
    return s_initError == 1 ? 1 : 2;
}

int wp81_mix_error(void)
{
    return s_initError;
}

/// tag 0 = music, 1 = hit sound. `frames` is interleaved f32 at `rate`/`ch`; the callee copies it
/// and converts to i16, because keeping two copies of a 177-second track would cost 100 MB on a
/// phone that gives an app a few hundred.
int wp81_mix_pcm(int tag, const char *key, int keyLen,
                 unsigned rate, unsigned ch,
                 const float *frames, unsigned long long frameCount)
{
    if (!frames || frameCount == 0 || rate == 0 || ch == 0 || ch > 8) {
        return 1;
    }
    // If XAudio2 never came up there is nothing to feed, and keeping a 34 MB copy of the track
    // resident for a mixer that does not exist would be the only lasting cost of the failure.
    if (!ensureEngine()) {
        return 4;
    }
    char norm[MIX_KEY_CAP];
    int nlen = keyNorm(key, keyLen, norm, MIX_KEY_CAP);
    if (nlen == 0) {
        return 1;
    }

    // Re-upload of the same sound replaces the old buffer rather than adding an entry: a chart
    // that loads twice (it does - `music_new` fires twice on device) would otherwise hold the
    // track twice.
    MixClip *c = findClip(norm, nlen);
    if (!c) {
        if (s_clipCount >= MIX_MAX_CLIPS) {
            return 2;
        }
        c = &s_clips[s_clipCount++];
        c->keyLen = nlen;
        int i;
        for (i = 0; i < nlen; i++) {
            c->key[i] = norm[i];
        }
        c->key[nlen] = 0;
        c->pcm = 0;
    }
    if (c->pcm) {
        // Voices already built against the old buffer would be left pointing at freed memory, and
        // a hit sound that plays whatever happens to be at a dangling pointer is not a bug you
        // can diagnose from a log. Destroy them here; they are rebuilt on the next play.
        int row = (int)(c - s_clips);
        if (row >= 0 && row < MIX_MAX_CLIPS) {
            int s;
            for (s = 0; s < MIX_SFX_SLOTS; s++) {
                if (s_sfx[row][s].voice) {
                    s_sfx[row][s].voice->DestroyVoice();
                    s_sfx[row][s].voice = 0;
                    s_voiceCount--;
                }
                s_sfx[row][s].clip = 0;
            }
        }
        s_pcmBytes -= c->frames * c->ch * 2;
        mixFree(c->pcm);
        c->pcm = 0;
    }

    unsigned long long samples = frameCount * ch;
    short *dst = (short *)mixAlloc(samples * 2);
    if (!dst) {
        return 3;
    }
    // f32 -> i16. Clamped rather than left to wrap: symphonia can emit values just outside
    // -1..1 on decoded material, and a wrapped sample is a full-scale click in the middle of
    // the music.
    unsigned long long i;
    for (i = 0; i < samples; i++) {
        float v = frames[i];
        if (v > 1.0f) v = 1.0f;
        if (v < -1.0f) v = -1.0f;
        dst[i] = (short)(v * 32767.0f);
    }

    c->pcm = dst;
    c->rate = rate;
    c->ch = ch;
    c->frames = frameCount;
    s_pcmBytes += samples * 2;

    // A re-uploaded music track must have its voice rebuilt against the new format, otherwise
    // the voice keeps the old rate and the song plays at the wrong speed.
    if (tag == 0 && s_music.clip == c) {
        s_music.clip = 0;
        if (s_music.voice) {
            s_music.voice->DestroyVoice();
            s_music.voice = 0;
            s_voiceCount--;
        }
    }
    return 0;
}

int wp81_mix_music_new(const char *key, int keyLen,
                       double rate, float amp, double loopMixTime, double clipSecs)
{
    if (!ensureEngine()) {
        return 1;
    }
    char norm[MIX_KEY_CAP];
    int nlen = keyNorm(key, keyLen, norm, MIX_KEY_CAP);
    MixClip *c = findClip(norm, nlen);
    if (!c || !c->pcm) {
        return 2;
    }

    if (s_music.voice) {
        s_music.voice->Stop(0, XAUDIO2_COMMIT_NOW);
        s_music.voice->DestroyVoice();
        s_music.voice = 0;
        s_voiceCount--;
    }

    WAVEFORMATEX w = clipFormat(c);
    HRESULT hr = s_xa->CreateSourceVoice(&s_music.voice, &w, 0, XAUDIO2_DEFAULT_FREQ_RATIO,
                                         0, 0, 0);
    if (FAILED(hr) || !s_music.voice) {
        s_music.voice = 0;
        return 3;
    }
    s_voiceCount++;

    // prpr's `loop_mix_time`: > 0 loops the last N seconds of the track (the chart's tail), < 0
    // loops the whole file, 0 plays once. All three land on XAudio2's own loop window, so the
    // looping happens inside the mixer and the shell never has to re-submit anything to keep a
    // chart repeating.
    s_loopBegin = 0;
    s_loopLength = 0;
    s_loopCount = 0;
    s_musicLoop = false;
    if (loopMixTime > 0.0) {
        unsigned long long tail = (unsigned long long)(loopMixTime * (double)c->rate);
        if (tail > c->frames) tail = c->frames;
        s_loopBegin = (UINT32)(c->frames - tail);
        s_loopLength = (UINT32)tail;
        s_loopCount = XAUDIO2_LOOP_INFINITE;
        s_musicLoop = true;
    } else if (loopMixTime < 0.0) {
        s_loopCount = XAUDIO2_LOOP_INFINITE;
        s_musicLoop = true;
    }

    // The clip has to be attached before the submit: musicSubmit reads s_music.clip, and a helper
    // that silently did nothing because the caller had not initialised the state yet is exactly
    // the kind of failure that shows up as "no music" with no error anywhere.
    s_music.clip = c;
    if (musicSubmit(0.0) != 0) {
        s_music.clip = 0;
        s_music.voice->DestroyVoice();
        s_music.voice = 0;
        s_voiceCount--;
        return 4;
    }

    s_musicAmp = amp > 0.0f ? (double)amp : 1.0;
    s_musicRate = rate > 0.0 ? rate : 1.0;
    s_musicPlaying = false;
    s_musicStarted = 0;
    s_fadeActive = false;
    s_music.voice->SetVolume((float)s_musicAmp, XAUDIO2_COMMIT_NOW);
    // Nightcore/slow: XAudio2's frequency ratio is the rate control, so a rate change costs a
    // SetFrequencyRatio instead of resampling the buffer.
    if (s_musicRate != 1.0) {
        s_music.voice->SetFrequencyRatio((float)s_musicRate, XAUDIO2_COMMIT_NOW);
    }
    return 0;
}

void wp81_mix_music_play(void)
{
    if (!s_music.voice || !ensureEngine()) {
        return;
    }
    XAUDIO2_VOICE_STATE st;
    st.pCurrentBufferContext = 0;
    st.BuffersQueued = 0;
    st.SamplesPlayed = 0;
    s_music.voice->GetState(&st, 0);
    if (st.BuffersQueued == 0) {
        // Everything was consumed. Start() against an empty queue reports success and produces
        // nothing, so a finished track would come back as "playing" while silent - the exact
        // state the shell could never see from the outside and therefore never recovered from.
        double from = s_posSeconds;
        if (s_loopCount == 0) {
            from = 0.0;
        }
        musicSubmit(from);
    }
    s_music.voice->Start(0, XAUDIO2_COMMIT_NOW);
    s_musicPlaying = true;
    s_musicStarted = 1;
}

void wp81_mix_music_pause(void)
{
    if (!s_music.voice) {
        return;
    }
    s_music.voice->Stop(0, XAUDIO2_COMMIT_NOW);
    s_musicPlaying = false;
}

void wp81_mix_music_seek(double sec)
{
    if (!s_music.voice || !s_music.clip) {
        return;
    }
    bool wasPlaying = s_musicPlaying;
    if (musicSubmit(sec) != 0) {
        return;
    }
    if (wasPlaying) {
        s_music.voice->Start(0, XAUDIO2_COMMIT_NOW);
    }
}

double wp81_mix_music_position(void)
{
    applyFade();
    if (!s_music.voice || !s_music.clip || !s_music.clip->rate) {
        return s_posSeconds;
    }
    XAUDIO2_VOICE_STATE st;
    st.pCurrentBufferContext = 0;
    st.BuffersQueued = 0;
    st.SamplesPlayed = 0;
    s_music.voice->GetState(&st, 0);
    double total = (double)s_music.clip->frames / (double)s_music.clip->rate;
    if (st.BuffersQueued == 0) {
        // The queue drained. For a track that plays once that means it reached the end, and
        // SamplesPlayed may already be past the last frame - reporting a time beyond the audio
        // would read to the shell as the media outrunning the clock and set it chasing again.
        if (s_loopCount == 0 && s_posSeconds < total) {
            s_posSeconds = total;
        }
        return s_posSeconds;
    }
    long long delta = (long long)(st.SamplesPlayed - s_samplesBase);
    if (delta < 0) delta = 0;
    double sec = (double)delta / (double)s_music.clip->rate;
    if (sec > total) sec = total;
    s_posSeconds = sec;
    return sec;
}

int wp81_mix_music_paused(void)
{
    if (!s_music.voice) {
        return 1;
    }
    return s_musicPlaying ? 0 : 1;
}

void wp81_mix_music_amp(float amp)
{
    s_musicAmp = amp > 0.0f ? (double)amp : 0.0;
    if (s_music.voice && !s_fadeActive) {
        s_music.voice->SetVolume((float)s_musicAmp, XAUDIO2_COMMIT_NOW);
    }
}

void wp81_mix_music_lowpass(float cutoff)
{
    if (!s_music.voice) {
        return;
    }
    XAUDIO2_FILTER_PARAMETERS p;
    p.Type = LowPassFilter;
    // XAudio2's `Frequency` is normalised against Nyquist, not in Hz. A cutoff of 0 (prpr's
    // "off") therefore has to map to 1.0, i.e. pass everything - mapping it to 0 would silence
    // the track whenever the chart is not applying a low-pass effect.
    if (cutoff <= 0.0f || cutoff >= 1.0f) {
        p.Frequency = 1.0f;
    } else {
        p.Frequency = cutoff;
    }
    p.OneOverQ = XAUDIO2_DEFAULT_FILTER_ONEOVERQ;
    s_music.voice->SetFilterParameters(&p, XAUDIO2_COMMIT_NOW);
}

void wp81_mix_music_fade(double seconds, int out)
{
    s_fadeActive = true;
    s_fadeSeconds = seconds > 0.0 ? seconds : 0.0;
    s_fadeOut = out != 0;
    s_fadeStartMs = GetTickCount64();
    // GetVolume is a void out-parameter in this header, not a getter.
    float cur = (float)s_musicAmp;
    if (s_music.voice) {
        s_music.voice->GetVolume(&cur);
    }
    s_fadeFrom = cur;
}

/// The shell asks for a hit sound by whatever name it happens to hold.
///
/// The return value is why this is not `void`. C# routes through here only when it gets 0, so a
/// name the mixer was never handed - the chart's click/drag/flick PCM does not exist until the
/// chart finishes loading - comes back as 2 and the shell opens a MediaElement instead. Silently
/// doing nothing is the failure mode this whole file exists to prevent, and it is precisely how
/// "the hit sound plays but the music vanished" gets misdiagnosed when the truth is that nothing
/// played at all.
int wp81_mix_sfx_play(const char *key, int keyLen, float amp)
{
    if (!key) {
        return 2;
    }
    if (!ensureEngine()) {
        return 1;
    }
    char norm[MIX_KEY_CAP];
    int nlen = keyNorm(key, keyLen, norm, MIX_KEY_CAP);
    if (nlen == 0) {
        return 2;
    }
    MixClip *c = findClip(norm, nlen);
    if (!c || !c->pcm) {
        return 2;
    }

    // Find this sound's voice row and the round-robin slot within it.
    int row = -1;
    int i;
    for (i = 0; i < s_clipCount; i++) {
        if (&s_clips[i] == c) {
            row = i;
            break;
        }
    }
    if (row < 0) {
        return 3;
    }

    static int s_slot[MIX_MAX_CLIPS];
    int slot = s_slot[row] % MIX_SFX_SLOTS;
    s_slot[row] = (slot + 1) % MIX_SFX_SLOTS;

    MixVoice *mv = &s_sfx[row][slot];
    if (!mv->voice) {
        WAVEFORMATEX w = clipFormat(c);
        HRESULT hr = s_xa->CreateSourceVoice(&mv->voice, &w, 0, XAUDIO2_DEFAULT_FREQ_RATIO,
                                             0, 0, 0);
        if (FAILED(hr) || !mv->voice) {
            mv->voice = 0;
            return 4;
        }
        s_voiceCount++;
        buildBuffer(&mv->buf, c);
        mv->clip = c;
    }

    // Retrigger: stop, drop anything queued, rewind, set the level, go. No file is opened and no
    // pipeline is rebuilt, which is the whole difference against a MediaElement - the hit sound
    // is one command deep instead of hundreds of milliseconds.
    mv->voice->Stop(0, XAUDIO2_COMMIT_NOW);
    mv->voice->FlushSourceBuffers();
    // No SetSourcePosition here: this header has none, and none is needed. Flushing empties the
    // queue, so the buffer submitted on the next line plays from byte zero of its own data - the
    // rewind is implied by the flush rather than requested separately.
    mv->voice->SubmitSourceBuffer(&mv->buf, 0);
    mv->voice->SetVolume(amp > 0.0f ? amp : 1.0f, XAUDIO2_COMMIT_NOW);
    mv->voice->Start(0, XAUDIO2_COMMIT_NOW);
    return 0;
}

void wp81_mix_sfx_stop_all(void)
{
    int r, s;
    for (r = 0; r < MIX_MAX_CLIPS; r++) {
        for (s = 0; s < MIX_SFX_SLOTS; s++) {
            if (s_sfx[r][s].voice) {
                s_sfx[r][s].voice->Stop(0, XAUDIO2_COMMIT_NOW);
                s_sfx[r][s].voice->FlushSourceBuffers();
            }
        }
    }
}

int wp81_mix_voices(void)
{
    return s_voiceCount;
}

unsigned long long wp81_mix_bytes(void)
{
    return s_pcmBytes;
}

int wp81_mix_clips(void)
{
    return s_clipCount;
}

/// One line for the log: whether the mixer is up, how much PCM it is holding and how many voices
/// it has made. Without this a silent chart and a healthy chart look identical from the outside,
/// which is the same problem the shell's other diag lines exist for.
int wp81_mix_diag(char *buf, int cap)
{
    if (!buf || cap < 8) {
        return 0;
    }
    int n = 0;
    const char *p = "mix engine=";
    while (*p && n < cap - 1) buf[n++] = *p++;
    p = (s_initState == 1) ? "1" : (s_initState == 0 ? "0" : "-1");
    while (*p && n < cap - 1) buf[n++] = *p++;
    p = " err=";
    while (*p && n < cap - 1) buf[n++] = *p++;
    // decimal itoa without a CRT
    int v = s_initError;
    char tmp[12];
    int tl = 0;
    int neg = 0;
    if (v < 0) { neg = 1; v = -v; }
    if (v == 0) tmp[tl++] = '0';
    while (v > 0 && tl < 12) { tmp[tl++] = (char)('0' + (v % 10)); v /= 10; }
    if (neg && tl < 12) tmp[tl++] = '-';
    while (tl > 0 && n < cap - 1) buf[n++] = tmp[--tl];
    p = " clips=";
    while (*p && n < cap - 1) buf[n++] = *p++;
    v = s_clipCount; tl = 0; neg = 0;
    if (v == 0) tmp[tl++] = '0';
    while (v > 0 && tl < 12) { tmp[tl++] = (char)('0' + (v % 10)); v /= 10; }
    while (tl > 0 && n < cap - 1) buf[n++] = tmp[--tl];
    p = " voices=";
    while (*p && n < cap - 1) buf[n++] = *p++;
    v = s_voiceCount; tl = 0;
    if (v == 0) tmp[tl++] = '0';
    while (v > 0 && tl < 12) { tmp[tl++] = (char)('0' + (v % 10)); v /= 10; }
    while (tl > 0 && n < cap - 1) buf[n++] = tmp[--tl];
    p = " pcmMB=";
    while (*p && n < cap - 1) buf[n++] = *p++;
    unsigned long long b = s_pcmBytes / (1024ULL * 1024ULL);
    tl = 0;
    if (b == 0) tmp[tl++] = '0';
    while (b > 0 && tl < 12) { tmp[tl++] = (char)('0' + (int)(b % 10ULL)); b /= 10ULL; }
    while (tl > 0 && n < cap - 1) buf[n++] = tmp[--tl];
    p = " music=";
    while (*p && n < cap - 1) buf[n++] = *p++;
    p = s_music.voice ? "1" : "0";
    while (*p && n < cap - 1) buf[n++] = *p++;
    buf[n] = 0;
    return n;
}

} // extern "C"
