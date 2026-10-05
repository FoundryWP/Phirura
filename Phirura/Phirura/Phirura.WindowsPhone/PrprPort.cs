using System;
using System.Runtime.InteropServices;

namespace Phirura
{
    /// <summary>
    /// Bridge to <c>wp81prpr.dll</c> - the real Rust <c>prpr</c>, compiled for Windows Phone 8.1.
    ///
    /// This is the Path B arrangement: the game logic is Rust, the platform work stays here. The
    /// Rust side cannot call into C# directly (a C# app cannot export a native entry point), so the
    /// direction that matters is reversed - this class hands Rust a table of function pointers at
    /// startup, and Rust calls through them.
    ///
    /// Everything in <see cref="HostCallbacks"/> must stay in step with the <c>#[repr(C)]</c>
    /// struct of the same name in <c>rust-wp81/prpr-port/src/lib.rs</c>. Field order and types are
    /// the ABI; a mismatch does not fail to compile, it corrupts the stack at the first call.
    /// </summary>
    public static class PrprPort
    {
        private const string Dll = "wp81prpr.dll";

        // --- the C# -> Rust direction -------------------------------------

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_port_version();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_parse_pec(byte[] data, int len);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_chart_info(out ChartInfo info);

        // --- the renderer drain -------------------------------------------
        //
        // The Rust macroquad shim records every draw call instead of issuing GL. Once per frame
        // the shell flattens that recording (`encode`), reads the records through the two
        // pointers, replays them, then clears (`reset`). The pointers address the DLL's own
        // statics, so they are re-read every frame and never cached.

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_draw_encode();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_draw_commands();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_draw_vertices();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr wp81_draw_indices();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_draw_reset();

        /// <summary>
        /// How many vertices and indices the last <c>wp81_draw_encode</c> produced.
        ///
        /// Read this *before* <see cref="Drain"/>, not after: <c>Drain</c> reads exactly
        /// <c>vertexCapacity</c> elements, so a capacity larger than the truth copies arena bytes no
        /// command refers to - and <c>OnPortDraw</c> rasterises those, which makes the result a frame
        /// of garbage geometry rather than a shorter frame - while a smaller one truncates a batch
        /// mid-triangle. Neither shows up in a command count.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_draw_counts(out uint vertices, out uint indices);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_draw_selftest();

        // --- the texture registry ------------------------------------------
        //
        // A Texture record in the drain carries only a u32 id. The Rust shim allocates those ids
        // where it has pixels rather than filenames, so the shell cannot resolve one by itself;
        // these calls are how it does. The shell reads each texture once, builds a CanvasBitmap and
        // keeps it, so the copy happens once per texture per session rather than once per frame.

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_texture_count();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_texture_id_at(uint index);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_texture_data(uint id, out uint width, out uint height, IntPtr outBytes, uint cap);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_texture_overflow();

        // The incremental half of that same registry.
        //
        // The four calls above answer "what does this texture look like", which is all chart art
        // needs: those bytes are uploaded once and never change afterwards. The glyph atlas is the
        // opposite. `prpr` fills it a rectangle at a time through `glTexSubImage2D`, and a word's
        // glyph is rasterised the first time that word is *shown* - so the atlas keeps changing
        // while the player is playing, and a reader that runs once can only ever see the glyphs
        // that happened to exist at start-up.
        //
        // WHY A DIRTY RECTANGLE AND NOT A VERSION NUMBER. A version answers "something changed"
        // and nothing else, and the only thing the shell can do with that is rebuild the whole
        // carrier: a 1024x1024 atlas is a million `Color.FromArgb` plus one `CreateFromColors`,
        // tens of milliseconds of hitch at the exact moment judgement text appears on screen. The
        // rectangle *is* the change - unioned on the Rust side across every upload in a frame - so
        // one sync per frame touches only the pixels that moved, and a new glyph costs a few
        // hundred pixels instead of a million.
        //
        // WHY READING IT CLEARS IT. A "last synced" stamp kept here in C# would be a second ledger
        // of the same fact, and two ledgers drift: the discrepancy never reproduces on the desktop
        // and only ever shows up on the phone. Consuming the flag on read leaves all the state on
        // the side that owns the pixels, so neither side has to remember what the other saw.

        /// <summary>
        /// The rectangle written to <c>id</c> since the previous call that passed a real buffer.
        ///
        /// Returns 1 and writes `(x, y, w, h)` as four `u32`s at <paramref name="outRect"/>, 0 when
        /// nothing is pending, -1 when the id is not registered. A newly registered texture reports
        /// its whole extent on its first call, which is what lets the caller treat "new" and
        /// "changed" as one rule. The read consumes: the next call for the same id answers 0 unless
        /// something was written in between.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_texture_dirty(uint id, IntPtr outRect);

        /// <summary>
        /// Copies the RGBA bytes of one sub-rectangle of <c>id</c>.
        ///
        /// Returns the byte count, -1 for an unknown id, -2 when <paramref name="cap"/> is too small
        /// (or <paramref name="outBytes"/> is null), -3 when the rectangle lies outside the texture.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_texture_rect_data(uint id, uint x, uint y, uint w, uint h, IntPtr outBytes, uint cap);

        // --- the game host -------------------------------------------------
        //
        // This is the real thing: `wp81_game_create` builds a `prpr::scene::GameScene` from the
        // chart on disk and wraps it in a `Main`; `wp81_game_frame` advances and renders exactly one
        // frame and leaves the recorded draw commands for `Drain`. The scene lives in the DLL, so
        // these are the only three calls the shell makes per session plus one per frame.

        /// <summary>
        /// Builds the game. Both strings are UTF-16 and NUL-terminated; the first is the chart's
        /// folder name under the shell's local <c>charts\</c> folder, the second the chart file.
        ///
        /// Returns 0 on success, or a negative code:
        ///   -1 nothing to load, -2 the chart or scene failed to build, -3 a game is already
        ///   running, -100 the scene panicked.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Unicode)]
        private static extern int wp81_game_create(string folder, string chartName);

        /// <summary>
        /// Boots phira itself - its own <c>MainScene</c> and library UI - rather than a chart.
        ///
        /// Takes no arguments because phira owns its own configuration: it reads its data directory,
        /// fonts and settings through hooks the shell already provides, so there is nothing a chart
        /// name would tell it that it does not look up itself. Afterwards it is driven by exactly
        /// the same <c>wp81_game_frame</c> call as a chart is.
        ///
        /// Returns 0 on success, or -3 a game is already running, -2 construction never reached
        /// phira's first frame, -100 construction panicked. The reason for -2 comes back through
        /// <c>wp81_game_error</c>.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Unicode)]
        private static extern int wp81_phira_boot(string dataDir, string chartLocalPath, int autoPlay);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_frame(double dt);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_game_destroy();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern long wp81_game_frames();

        /// <summary>
        /// FFI mirror of Rust's <c>host::PlayResultAbi</c>.
        ///
        /// **Field order is the ABI.** It is deliberately not the order <c>PlayResult</c> uses, and
        /// reordering either side would not fail anywhere - it would land values on the wrong
        /// columns of the result screen.
        ///
        /// Every field is 4 bytes: the accuracy is carried as a <c>float</c> on purpose, which
        /// makes the struct 72 bytes with no padding at all, so neither side has to agree on an
        /// alignment rule first. That is the whole point - the usual way an <c>double</c> in the
        /// middle of an FFI struct goes wrong is C rounding its offset to 8 and .NET to 4 (or the
        /// other way), and there is nothing here to round.
        /// </summary>
        [StructLayout(LayoutKind.Sequential)]
        public struct PlayResultAbi
        {
            public float Accuracy;
            public uint Score;
            public uint MaxCombo;
            public uint NumOfNotes;
            public uint Count0;
            public uint Count1;
            public uint Count2;
            public uint Count3;
            public uint Early;
            public uint Late;
            public uint EarlyKind0;
            public uint EarlyKind1;
            public uint EarlyKind2;
            public uint EarlyKind3;
            public uint LateKind0;
            public uint LateKind1;
            public uint LateKind2;
            public uint LateKind3;
        }

        /// <summary>
        /// The running game's full result. See <see cref="GamePlayResult"/>.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_result(out PlayResultAbi result);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_running();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_game_set_screen(float width, float height);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint wp81_game_progress();

        // `cap` is `usize`, which is 4 bytes on the only target this DLL is ever loaded into - the
        // same `uint` the texture path already uses for it.
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_last_error(IntPtr buf, uint cap);

        /// <summary>
        /// Queues one touch event for the frame about to be built. Must be called before
        /// <see cref="GameFrame"/> - the frame drains the queue, so anything pushed after
        /// it arrives a frame late.
        ///
        /// <paramref name="phase"/> is a plain number because the shell is C#: 0=Started,
        /// 1=Stationary, 2=Moved, 3=Ended, 4=Cancelled - <c>miniquad::TouchPhase</c>'s
        /// declaration order, which differs from this shell's own <c>TouchPhase</c> enum
        /// (there Moved precedes Stationary). <paramref name="x"/> and <paramref name="y"/>
        /// are screen pixels in the same space as <see cref="GameSetScreen"/>, because
        /// that is what <c>prpr::judge</c> divides by <c>screen_width()</c> /
        /// <c>screen_height()</c> to reach NDC. <paramref name="time"/> is uptime in the
        /// same units as <see cref="GameSetTime"/> - effectively the chart time the
        /// finger was at - because <c>prpr</c> ages the touch with
        /// <c>t - (uptime - time) * speed</c> to rewind it to the chart moment it occurred.
        ///
        /// Returns 0, -1 for an unknown phase, or -2 when this frame's queue is full.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_touch(uint id, uint phase, float x, float y, double time);

        /// <summary>
        /// Sets the engine timeline to an absolute time, before the next
        /// <see cref="GameFrame"/>.
        ///
        /// The shell owns the music clock (it has the audio position), so the Rust side
        /// must not invent one. Call this and then <see cref="GameFrame"/> with
        /// <c>0.0</c> - not <c>dt</c> - because <c>host::frame</c> advances
        /// <c>CLOCK</c> by <c>dt</c> itself, and passing both would advance the chart
        /// twice as fast as the music. Absolute rather than differential because a
        /// cumulative sum starts drifting the moment the shell misses a frame, and a
        /// rhythm game cannot be restarted after a seek without one anyway.
        /// </summary>
        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_game_set_time(double t);

        // --- the Rust -> C# direction -------------------------------------

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate int AudioDecodeFn(IntPtr data, IntPtr len, IntPtr outInfo);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate uint AudioMusicNewFn(uint clip, double rate, float amp, double loopMix);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void AudioMusicIdFn(uint id);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void AudioMusicSeekFn(uint id, double position);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate double AudioMusicPositionFn(uint id);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate int AudioMusicPausedFn(uint id);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void AudioMusicAmpFn(uint id, float amp);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void AudioMusicFadeFn(uint id, double seconds, int outFlag);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate uint AudioSfxNewFn(uint clip);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void AudioSfxPlayFn(uint id, float amp);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate int PickFileFn(IntPtr title, IntPtr outPath, IntPtr cap);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate int SaveFileFn(IntPtr title, IntPtr suggested, IntPtr outPath, IntPtr cap);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void PresentFn();

        /// <summary>
        /// Reads a file the Rust game asked for. Returns the byte count, or -1 if it is not there.
        ///
        /// <c>name</c> is NUL-terminated UTF-16, relative to the app's local <c>charts\</c> folder.
        /// </summary>
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate int LoadAssetFn(IntPtr name, IntPtr outBytes, IntPtr cap);

        /// <summary>
        /// One line of UTF-8 text from Rust for the log.
        ///
        /// <c>msg</c>/<c>len</c> are a borrowed slice rather than a NUL-terminated string: Rust
        /// hands over bytes it already owns and the shell copies them before returning, so there is
        /// nothing to free and no terminator to trust.
        /// </summary>
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void LogFn(IntPtr msg, IntPtr len);

        /// <summary>
        /// Mirrors <c>HostCallbacks</c> in the Rust port.
        ///
        /// Laid out by hand rather than with <c>[MarshalAs]</c> because it is passed by pointer,
        /// not marshalled: the Rust side copies the struct wholesale, so the layout has to be the
        /// natural one for the target. Every field is a function pointer (4 bytes on ARM), which
        /// makes the layout uniform.
        ///
        /// FIELD ORDER IS THE ABI. <c>log</c> is appended last to match the Rust struct - if
        /// either side gains a field in the middle, this compiles and then calls the wrong function
        /// pointer. <see cref="CallbacksLayoutOk"/> is the guard.
        /// </summary>
        [StructLayout(LayoutKind.Sequential)]
        private struct HostCallbacks
        {
            public IntPtr audio_decode;
            public IntPtr audio_music_new;
            public IntPtr audio_music_play;
            public IntPtr audio_music_pause;
            public IntPtr audio_music_seek;
            public IntPtr audio_music_position;
            public IntPtr audio_music_paused;
            public IntPtr audio_music_set_amplifier;
            public IntPtr audio_music_set_low_pass;
            public IntPtr audio_music_fade;
            public IntPtr audio_sfx_new;
            public IntPtr audio_sfx_play;
            public IntPtr pick_file;
            public IntPtr save_file;
            public IntPtr present;
            public IntPtr load_asset;
            public IntPtr log;
        }

        /// <summary>
        /// Number of function pointers in the Rust <c>HostCallbacks</c>. Kept as a literal because
        /// the C# struct cannot be counted at compile time; the point is that a mismatch is caught
        /// by <see cref="CallbacksLayoutOk"/> instead of by a crash inside a Rust callback.
        /// </summary>
        private const int ExpectedCallbackCount = 17;

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_install_callbacks(ref HostCallbacks callbacks);

        /// <summary>Decoded audio metadata, matching <c>DecodedAudio</c> in Rust.</summary>
        [StructLayout(LayoutKind.Sequential)]
        private struct DecodedAudio
        {
            public uint sample_rate;
            public ulong frame_count;
        }

        /// <summary>Chart shape, matching <c>ChartInfo</c> in Rust.</summary>
        [StructLayout(LayoutKind.Sequential)]
        public struct ChartInfo
        {
            public uint lines;
            public uint notes;
            public long duration_ms;
        }

        // --- the drain's wire types ---------------------------------------
        //
        // `#[repr(C)]` tag-then-payload records, matching `PortCmd` / `PortVertex` in the macroquad
        // shim. Both are 32 bytes with no 8-byte members, which is what keeps the ARM marshaller
        // from inventing padding. Field order is the ABI: a mismatch does not fail to compile, it
        // silently scrambles every drawn frame.

        /// <summary>Kind tags, matching the <c>KIND_*</c> constants in Rust.</summary>
        public enum PortKind : uint
        {
            Texture = 0,
            Rect = 1,
            Line = 2,
            Circle = 3,
            Geometry = 4,
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct PortCmd
        {
            public uint kind;
            public uint flags;
            public uint vstart;
            public uint vcount;
            public uint istart;
            public uint icount;

            // The payload is fixed at 8 floats so the record size is uniform. Which slots are
            // meaningful depends on `kind`; the rest are zero.
            public float f0, f1, f2, f3, f4, f5, f6, f7;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct PortVertex
        {
            public float x, y, z, u, v, r, g, b;
        }

        /// <summary>One frame of recorded draw calls, already flattened for replay.</summary>
        public sealed class DrainFrame
        {
            public int Count;
            public PortCmd[] Commands;

            /// <summary>Vertices for <see cref="PortKind.Geometry"/> records, indexed by <c>vstart</c>.</summary>
            public PortVertex[] Vertices;

            /// <summary>u16 indices for <see cref="PortKind.Geometry"/> records.</summary>
            public ushort[] Indices;
        }

        /// <summary>
        /// Pulls the frame the Rust shim recorded and resets it.
        ///
        /// Everything is copied out into managed arrays before the reset, because the reset clears
        /// the arenas the pointers address. Copying is also what makes the data safe to touch from
        /// the XAML draw callback on the UI thread.
        /// </summary>
        public static DrainFrame Drain(uint vertexCapacity, uint indexCapacity)
        {
            int n = (int)wp81_draw_encode();
            if (n <= 0)
            {
                // Nothing recorded this frame. Still reset so stale commands cannot leak into the
                // next one if encode ever bails early on a full buffer.
                wp81_draw_reset();
                return new DrainFrame { Count = 0, Commands = new PortCmd[0], Vertices = new PortVertex[0], Indices = new ushort[0] };
            }

            PortCmd[] cmds = new PortCmd[n];
            IntPtr cmdPtr = wp81_draw_commands();
            // Copy by field rather than Marshal.PtrToStructure on the whole array: the latter
            // needs the exact managed size and is slower here than a manual loop over a known
            // 32-byte record.
            for (int i = 0; i < n; i++)
            {
                cmds[i] = ReadCmd(cmdPtr, i);
            }

            PortVertex[] verts = new PortVertex[0];
            if (vertexCapacity > 0)
            {
                verts = new PortVertex[vertexCapacity];
                IntPtr vptr = wp81_draw_vertices();
                for (int i = 0; i < vertexCapacity; i++)
                {
                    verts[i] = ReadVertex(vptr, i);
                }
            }

            ushort[] idx = new ushort[0];
            if (indexCapacity > 0)
            {
                idx = new ushort[indexCapacity];
                IntPtr iptr = wp81_draw_indices();
                for (int i = 0; i < indexCapacity; i++)
                {
                    idx[i] = (ushort)Marshal.ReadInt16(iptr, i * 2);
                }
            }

            wp81_draw_reset();
            return new DrainFrame { Count = n, Commands = cmds, Vertices = verts, Indices = idx };
        }

        // --- the game host's managed surface ------------------------------

        /// <summary>Result codes from <see cref="GameCreate"/>, mirroring the Rust constants.</summary>
        public enum GameResult
        {
            Ok = 0,
            NoChart = -1,
            ParseFailed = -2,
            AlreadyRunning = -3,
            Panicked = -100,
        }

        /// <summary>
        /// Starts the real Rust game for a chart.
        ///
        /// <paramref name="folder"/> is the chart's directory name under the shell's local
        /// <c>charts\</c> folder; <paramref name="chartName"/> is the chart file inside it.
        /// <see cref="AssetRoot"/> must already point at that folder - the chart parse, the
        /// illustration and the audio all come back through <c>load_asset</c>.
        /// </summary>
        public static GameResult GameCreate(string folder, string chartName)
        {
            Install();
            return (GameResult)wp81_game_create(folder, chartName);
        }

        /// <summary>
        /// Starts the real Rust phira itself - its <c>MainScene</c>, its library UI, its settings -
        /// instead of a chart the shell picked out.
        ///
        /// This is the "open straight into the app" path. It is deliberately separate from
        /// <see cref="GameCreate"/> rather than a flag on it: the two differ in whether the shell
        /// hands over a chart location, and folding that into one entry point would put the choice
        /// inside Rust where it cannot report which of the two it actually did.
        ///
        /// Slow for the same reason <see cref="GameCreate"/> is - phira builds its data directory,
        /// loads its fonts and constructs the whole library scene - and, being synchronous, it
        /// gives the shell nothing to watch *while* it runs: this is one uninterrupted call on the
        /// UI thread, so the stage numbers published from inside can only be read in the turn
        /// after. That is enough to be worth logging, because the stage is what separates "died
        /// before phira was ever entered" from "died inside phira", and <see
        /// cref="GameLastError"/> carries the reason for every failure phira reports itself.
        ///
        /// <paramref name="dataDir"/> is the absolute directory that becomes the parent of phira's
        /// <c>data</c> folder. It has to be handed over rather than discovered: phira chooses a data
        /// directory at compile time for ohos and ios and leaves it unset everywhere else, which on
        /// a phone resolves to a relative <c>./data</c> that nothing may write - so the boot fails
        /// before it draws a frame, and the reason was being dropped. Null leaves it unset, which is
        /// what a desktop harness wants.
        ///
        /// <paramref name="chartLocalPath"/> is a chart for phira to open on its own song page once
        /// the library is up, spelled the way phira spells it (<c>custom/190</c>), not as a path;
        /// null stops at the library. <paramref name="autoPlay"/> starts that chart instead of
        /// waiting for a tap. The two stay separate so a build can show the song page first without
        /// having to name a different chart to do it.
        /// </summary>
        public static GameResult PhiraBoot(string dataDir, string chartLocalPath, bool autoPlay)
        {
            Install();
            return (GameResult)wp81_phira_boot(dataDir, chartLocalPath, autoPlay ? 1 : 0);
        }

        /// <summary>
        /// Advances and renders one frame, then returns the recorded draw commands.
        ///
        /// Returns an empty frame when nothing is running, so the caller can call this
        /// unconditionally from the render loop.
        /// </summary>
        public static DrainFrame GameFrame(double dt, uint vertexCapacity, uint indexCapacity)
        {
            int n = wp81_game_frame(dt);
            if (n < 0)
            {
                return new DrainFrame
                {
                    Count = 0,
                    Commands = new PortCmd[0],
                    Vertices = new PortVertex[0],
                    Indices = new ushort[0],
                };
            }
            return Drain(vertexCapacity, indexCapacity);
        }

        /// <summary>
        /// Advances and renders one frame, then drains it at exactly the size that frame needs.
        ///
        /// The capacities are not the caller's to choose. <see cref="Drain"/> reads exactly
        /// <c>vertexCapacity</c> elements, so a capacity larger than the frame's own vertex count
        /// copies arena bytes no command refers to - and <c>Win2DChartRenderer.OnPortDraw</c>
        /// **rasterises** those, which makes the symptom a frame of garbage geometry rather than a
        /// shorter frame - while a smaller one truncates a batch mid-triangle. Both are invisible in
        /// a command count, which is the only thing a caller could otherwise check.
        ///
        /// The counts are read *after* <c>wp81_game_frame</c> because that is the only moment they
        /// exist: <c>wp81_draw_reset</c> zeroes them at the end of the previous drain.
        /// </summary>
        public static DrainFrame GameFrameSized(double dt)
        {
            int n = wp81_game_frame(dt);
            if (n < 0)
            {
                return new DrainFrame
                {
                    Count = 0,
                    Commands = new PortCmd[0],
                    Vertices = new PortVertex[0],
                    Indices = new ushort[0],
                };
            }
            // The counts are read *after* `wp81_game_frame` because that is the only moment they
            // exist: `wp81_draw_reset` zeroes them at the end of the previous drain. Locals are
            // pre-declared because the WP8.1 project compiles as C# 5, which has no inline `out`
            // declarations - a C# 7 syntax error here parses as a cascade starting at the call site.
            uint vcap;
            uint icap;
            wp81_draw_counts(out vcap, out icap);
            return Drain(vcap, icap);
        }

        public static void GameDestroy()
        {
            wp81_game_destroy();
        }

        public static bool GameRunning()
        {
            return wp81_game_running() != 0;
        }

        public static long GameFrames()
        {
            return wp81_game_frames();
        }

        /// <summary>
        /// The running game's full result.
        ///
        /// `prpr` owns the judge on this path, so the shell's own <c>Judge</c> is never advanced and
        /// its <c>Result()</c> would describe a flawless zero-score play - a result page reading
        /// "0 points, 100% accuracy" after a real play is worse than one reading nothing, because it
        /// looks like a real verdict.
        ///
        /// Everything the result screen shows comes back here, counts included: <c>ResultPage</c>
        /// reads <c>Score</c>, <c>MaxCombo</c>, <c>NumOfNotes</c>, <c>Counts</c> and
        /// <c>Early</c>/<c>Late</c>, so a partial answer would put a real score next to "0 notes"
        /// and read as a bug rather than as a known gap.
        ///
        /// Named <c>GamePlayResult</c> rather than <c>GameResult</c> because that name is already
        /// taken by the status enum <see cref="GameCreate"/> returns - two members called
        /// <c>GameResult</c> is CS0102, not an overload.
        /// </summary>
        /// <returns>0 when a game is running, -1 when there is none.</returns>
        public static int GamePlayResult(out Phirura.Game.PlayResult result)
        {
            PlayResultAbi abi;
            int rc = wp81_game_result(out abi);
            result = new Phirura.Game.PlayResult
            {
                Score = abi.Score,
                Accuracy = abi.Accuracy,
                MaxCombo = abi.MaxCombo,
                NumOfNotes = abi.NumOfNotes,
                Counts = new uint[] { abi.Count0, abi.Count1, abi.Count2, abi.Count3 },
                Early = abi.Early,
                Late = abi.Late,
                EarlyKind = new uint[] { abi.EarlyKind0, abi.EarlyKind1, abi.EarlyKind2, abi.EarlyKind3 },
                LateKind = new uint[] { abi.LateKind0, abi.LateKind1, abi.LateKind2, abi.LateKind3 },
            };
            return rc;
        }

        /// <summary>
        /// Reports the shell's surface size to the Rust renderer.
        ///
        /// Must be called once after the drawing surface is measured and before the first frame:
        /// <c>prpr</c> projects through statics that default to 0x0, and at 0x0 the aspect ratio is
        /// degenerate so the whole chart collapses to a single point.
        ///
        /// The size is also kept here, because it is the one figure the renderer needs to convert
        /// those pixels into the DIPs a <see cref="Microsoft.Graphics.Canvas.CanvasDrawingSession"/>
        /// actually works in - and the two only agree at dpi == 96. Re-deriving it on the drawing
        /// side would mean asking the canvas how big it is and hoping it still matches what Rust
        /// was told; recording it where it is decided removes the hope from the calculation.
        /// </summary>
        public static float ScreenPixelWidth { get; private set; }

        /// <summary>See <see cref="ScreenPixelWidth"/>.</summary>
        public static float ScreenPixelHeight { get; private set; }

        public static void GameSetScreen(float width, float height)
        {
            ScreenPixelWidth = width;
            ScreenPixelHeight = height;
            wp81_game_set_screen(width, height);
        }

        /// <summary>
        /// Sets the engine timeline to an absolute time. See <see cref="GameSetTime"/>.
        ///
        /// Absolute rather than differential because <c>host::frame</c> advances <c>CLOCK</c> by
        /// <c>dt</c> itself: the shell owns the music clock (it has the audio position), so it writes
        /// the real time here and then calls the frame with <c>0.0</c>. A cumulative sum would start
        /// drifting the moment the shell missed a frame, and a chart cannot be restarted after a seek
        /// without one anyway.
        /// </summary>
        public static void GameSetTime(double t)
        {
            wp81_game_set_time(t);
        }

        /// <summary>
        /// Queues one finger event for the frame about to render. See <c>wp81_game_touch</c>.
        ///
        /// Returns 0 on success, -1 for an unknown <paramref name="phase"/>, or -2 when this frame's
        /// queue is full (the shell drops the event and the finger is absent from that frame's touch
        /// state, which is recoverable; a `static mut` write past the end would not be).
        /// </summary>
        public static int GameTouch(uint id, uint phase, float x, float y, double time)
        {
            return wp81_game_touch(id, phase, x, y, time);
        }

        /// <summary>
        /// Which step <see cref="GameCreate"/> is on, named.
        ///
        /// Sampled from a timer on a thread that is <em>not</em> the one creating, and read for one
        /// reason: when a create takes the process down there is no return code to inspect and no
        /// line after the last, so the last step published is the whole evidence. The numbers mirror
        /// `host::progress` in Rust - neither side restates the other's, both spell them out.
        /// </summary>
        public static string GameProgressText()
        {
            uint p = wp81_game_progress();
            string name;
            switch (p)
            {
                case 0: name = "idle"; break;
                case 1: name = "entered"; break;
                case 2: name = "fs ready"; break;
                case 3: name = "loading info.yml"; break;
                case 4: name = "info ready"; break;
                case 5: name = "building scene"; break;
                case 6: name = "phira booting"; break;
                case 7: name = "phira ready"; break;
                default: name = "step " + p; break;
            }
            return p + " (" + name + ")";
        }

        /// <summary>
        /// Why the most recent <see cref="GameCreate"/> failed, or null when it has not.
        ///
        /// Read in the turn immediately after a non-zero return: it describes the <em>last</em>
        /// failure and the next attempt overwrites it.
        ///
        /// This exists because Rust reports every construction failure with `tracing::error!` and
        /// there is no tracing subscriber on the phone, so the event is dropped on the floor. The
        /// shell was therefore left holding `-2` and nothing else, while `-2` stood for three
        /// unrelated things - a chart the parser rejects, a `GameScene::new` that dies part-way
        /// through its textures, and a `Main::new` that never got a font. Ten asset requests and a
        /// six-second build looked identical to a missing file.
        /// </summary>
        public static string GameLastError()
        {
            // One message, not a stream: `create` records a single `String`. 4 KiB is well past the
            // longest of the ones seen and keeps this a single fixed allocation.
            const int cap = 4096;
            IntPtr buffer = Marshal.AllocHGlobal(cap);
            try
            {
                int len = wp81_game_last_error(buffer, (uint)cap);
                if (len <= 0)
                {
                    // -1: nothing has failed, or the last attempt succeeded.
                    // -2: would not fit; unreachable at this capacity, but not worth a silent 0 for.
                    return null;
                }
                byte[] bytes = new byte[len];
                Marshal.Copy(buffer, bytes, 0, len);
                return System.Text.Encoding.UTF8.GetString(bytes, 0, len);
            }
            catch (Exception ex)
            {
                return "(reading the error back failed: " + ex.Message + ")";
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }

        // --- the texture registry's managed surface -----------------------

        /// <summary>One registered texture: its Rust id and its RGBA pixels.</summary>
        public sealed class TextureData
        {
            public uint Id;
            public uint Width;
            public uint Height;
            public byte[] Rgba;
        }

        /// <summary>
        /// Reads every texture the Rust shim has registered.
        ///
        /// Returns fewer entries than the registry holds if any texture fails to copy - a texture
        /// that cannot be read is dropped with a log line rather than aborting the whole set, because
        /// a chart with one unreadable texture should still play.
        ///
        /// Call after <c>wp81_game_create</c> (the chart's textures are registered during the scene
        /// build) and rebuild the bitmaps whenever the count grows.
        ///
        /// This is the *fallback*, not the main path: it copies every texture in the registry into
        /// managed memory at once, and it can never see a byte that arrives afterwards - which is
        /// where every glyph comes from. The renderer reaches for it only while the incremental
        /// pair below is not exported yet, and only once. Kept rather than deleted because "read
        /// the whole registry in one go" is still the right answer when there is nothing to poll
        /// with, and a dead branch that still compiles is how that stays true.
        /// </summary>
        public static System.Collections.Generic.List<TextureData> ReadTextures()
        {
            System.Collections.Generic.List<TextureData> list =
                new System.Collections.Generic.List<TextureData>();

            uint count = wp81_texture_count();
            for (uint i = 0; i < count; i++)
            {
                uint id = wp81_texture_id_at(i);
                if (id == 0)
                {
                    continue;
                }

                uint w, h;
                // Metadata first: a null destination returns the byte length, so the buffer is sized
                // exactly rather than guessed at.
                int len = wp81_texture_data(id, out w, out h, IntPtr.Zero, 0);
                if (len <= 0 || w == 0 || h == 0)
                {
                    continue;
                }

                byte[] rgba = new byte[len];
                IntPtr buffer = Marshal.AllocHGlobal(len);
                try
                {
                    int got = wp81_texture_data(id, out w, out h, buffer, (uint)len);
                    if (got != len)
                    {
                        AppLog.WriteSync("PrprPort: texture " + id + " copy returned " + got
                            + ", expected " + len);
                        continue;
                    }
                    Marshal.Copy(buffer, rgba, 0, len);
                }
                finally
                {
                    Marshal.FreeHGlobal(buffer);
                }

                TextureData t = new TextureData();
                t.Id = id;
                t.Width = w;
                t.Height = h;
                t.Rgba = rgba;
                list.Add(t);
            }

            uint overflow = wp81_texture_overflow();
            if (overflow != 0)
            {
                AppLog.WriteSync("PrprPort: texture registry overflowed by " + overflow
                    + " texture(s); some sprites will not draw");
            }
            return list;
        }

        /// <summary>
        /// Registry summary - <c>"N registered, M dropped"</c> - without copying a single pixel.
        ///
        /// <see cref="ReadTextures"/> allocates every texture's RGBA into managed memory, which is
        /// precisely what a boot log must not do on a 512 MB phone, and there is no reason to: the
        /// question a boot log asks is only whether the chart's textures all fitted, and those two
        /// counters answer it. They are also the number the 32 MiB budget was sized from on the
        /// desktop, so this is what says whether that measurement transfers to the device.
        /// </summary>
        public static string TextureSummary()
        {
            return wp81_texture_count() + " registered, " + wp81_texture_overflow() + " dropped";
        }

        // --- the registry's incremental surface ---------------------------
        //
        // The managed half of `wp81_texture_dirty` / `wp81_texture_rect_data`. Shaped for one job:
        // being called from the draw loop every frame, where the honest cost of "nothing changed"
        // has to be a couple of native calls and no allocation at all.

        /// <summary>A rectangle in a texture's own pixel coordinates.</summary>
        public struct TexRect
        {
            public uint X;
            public uint Y;
            public uint W;
            public uint H;
        }

        /// <summary>How many textures the registry holds right now. One native call, nothing copied.</summary>
        public static int TextureCount()
        {
            return (int)wp81_texture_count();
        }

        /// <summary>
        /// Writes the first <paramref name="count"/> registered ids into <paramref name="ids"/>.
        ///
        /// The caller owns the buffer because this runs per frame: handing back a fresh array would
        /// put one allocation in front of every draw for ids that did not change since the last
        /// one. The buffer grows only when the registry does, and the registry is capped at 128
        /// slots on the Rust side, so growth is bounded and happens once.
        /// </summary>
        public static void CopyTextureIds(uint[] ids, int count)
        {
            int n = Math.Min(count, ids.Length);
            for (int i = 0; i < n; i++)
            {
                ids[i] = wp81_texture_id_at((uint)i);
            }
        }

        private static IntPtr _rectBuffer;

        /// <summary>
        /// The rectangle to synchronise for <paramref name="id"/>, or false when it is unchanged.
        ///
        /// Consumes the pending rectangle - the reason is spelled out above
        /// <c>wp81_texture_dirty</c>. Rust's -1 ("no such id") reads as "unchanged" rather than
        /// being thrown: an id that vanished between enumeration and this call is one nothing can
        /// draw any more, and taking the frame down over it would be a poor trade.
        ///
        /// The failure this cannot report is a *missing export* - the Rust side is still landing
        /// these two functions - which arrives as a thrown exception instead; the renderer catches
        /// that once and latches onto the full read rather than throwing once per texture per frame.
        /// </summary>
        public static bool TextureDirty(uint id, out TexRect rect)
        {
            rect = new TexRect();
            if (_rectBuffer == IntPtr.Zero)
            {
                // One 16-byte block for the life of the process rather than one per poll: this call
                // happens once per texture per frame, and an AllocHGlobal each time would spend its
                // time feeding an allocator to hold a rectangle that is read and thrown away again
                // in the same breath.
                _rectBuffer = Marshal.AllocHGlobal(16);
            }

            if (wp81_texture_dirty(id, _rectBuffer) != 1)
            {
                return false;
            }
            rect.X = (uint)Marshal.ReadInt32(_rectBuffer, 0);
            rect.Y = (uint)Marshal.ReadInt32(_rectBuffer, 4);
            rect.W = (uint)Marshal.ReadInt32(_rectBuffer, 8);
            rect.H = (uint)Marshal.ReadInt32(_rectBuffer, 12);
            return true;
        }

        /// <summary>
        /// A texture's dimensions without copying any of its pixels.
        ///
        /// Answers the one question the dirty rectangle cannot: a shell that has just dropped its
        /// carriers (<c>ResetPortTextures</c>) comes back to ids whose pending rectangle was
        /// already drained, so there is nothing to read - but the size is still known to Rust.
        /// </summary>
        public static bool TextureSize(uint id, out uint width, out uint height)
        {
            // A null destination returns the byte length, so the dimensions arrive without a copy
            // and without a buffer to size first.
            int len = wp81_texture_data(id, out width, out height, IntPtr.Zero, 0);
            return len > 0 && width > 0 && height > 0;
        }

        /// <summary>
        /// Copies one rectangle of a texture's RGBA pixels into a new managed array.
        ///
        /// Returns null when Rust refuses - unknown id, rectangle outside the texture, buffer too
        /// small - and logs why. One unreadable patch leaves some glyphs stale for a frame; failing
        /// the whole frame instead would cost a chart that is otherwise playing, which is the same
        /// trade <see cref="ReadTextures"/> makes and for the same reason.
        ///
        /// The <c>AllocHGlobal</c>/<c>FreeHGlobal</c> pair and the <c>got != len</c> check are
        /// copied from <see cref="ReadTextures"/> deliberately. A short copy is not recoverable here
        /// either: the tail of the array would hold whatever the previous call left in the block,
        /// and on WP8.1 an unmanaged block is never returned to the process - so it can only be
        /// read, never guessed at.
        /// </summary>
        public static byte[] ReadTextureRect(uint id, TexRect rect)
        {
            if (rect.W == 0 || rect.H == 0)
            {
                return null;
            }

            long pixels = (long)rect.W * rect.H;
            if (pixels > int.MaxValue / 4)
            {
                // Past anything this port can allocate, and int-sized because AllocHGlobal and
                // Marshal.Copy both are.
                return null;
            }
            int len = (int)pixels * 4;

            byte[] rgba = new byte[len];
            IntPtr buffer = Marshal.AllocHGlobal(len);
            try
            {
                int got = wp81_texture_rect_data(id, rect.X, rect.Y, rect.W, rect.H, buffer, (uint)len);
                if (got != len)
                {
                    AppLog.WriteSync("PrprPort: texture " + id + " rect ("
                        + rect.X + "," + rect.Y + "," + rect.W + "," + rect.H
                        + ") copy returned " + got + ", expected " + len);
                    return null;
                }
                Marshal.Copy(buffer, rgba, 0, len);
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
            return rgba;
        }

        private static PortCmd ReadCmd(IntPtr basePtr, int index)
        {
            int off = index * CmdStride;
            PortCmd c = new PortCmd();
            c.kind = (uint)Marshal.ReadInt32(basePtr, off + 0);
            c.flags = (uint)Marshal.ReadInt32(basePtr, off + 4);
            c.vstart = (uint)Marshal.ReadInt32(basePtr, off + 8);
            c.vcount = (uint)Marshal.ReadInt32(basePtr, off + 12);
            c.istart = (uint)Marshal.ReadInt32(basePtr, off + 16);
            c.icount = (uint)Marshal.ReadInt32(basePtr, off + 20);
            c.f0 = ReadF(basePtr, off + 24);
            c.f1 = ReadF(basePtr, off + 28);
            c.f2 = ReadF(basePtr, off + 32);
            c.f3 = ReadF(basePtr, off + 36);
            c.f4 = ReadF(basePtr, off + 40);
            c.f5 = ReadF(basePtr, off + 44);
            c.f6 = ReadF(basePtr, off + 48);
            c.f7 = ReadF(basePtr, off + 52);
            return c;
        }

        /// <summary>
        /// Byte size of one <c>PortCmd</c> in the DLL's array.
        ///
        /// Six u32 tag fields (24 B) plus eight f32 payload slots (32 B) = 56 B. This is NOT the
        /// size of the managed struct - <see cref="PortCmd"/>'s eight named floats are laid out
        /// identically, but the pointer arithmetic has to use the *native* record size, so it is
        /// derived here rather than from <c>Marshal.SizeOf</c>.
        /// </summary>
        private const int CmdStride = 6 * 4 + 8 * 4;

        /// <summary>Byte size of one <c>PortVertex</c>: eight f32 = 32 B.</summary>
        private const int VertexStride = 8 * 4;

        /// <summary>
        /// Fails loudly (in the log and the self-test summary) if the managed structs stop matching
        /// the native record sizes.
        ///
        /// This is the cheap guard for the one failure mode that is invisible at compile time: a
        /// field added to either side changes the stride, and the reader then walks off the end of
        /// every record and draws garbage. It cost a cycle the first time it happened (the C#
        /// reader assumed a 32-byte `PortCmd` when the native record is 56), so it is asserted
        /// rather than trusted.
        /// </summary>
        public static bool WireLayoutOk()
        {
            return Marshal.SizeOf<PortCmd>() == CmdStride
                && Marshal.SizeOf<PortVertex>() == VertexStride;
        }

        private static PortVertex ReadVertex(IntPtr basePtr, int index)
        {
            int off = index * VertexStride;
            PortVertex v = new PortVertex();
            v.x = ReadF(basePtr, off + 0);
            v.y = ReadF(basePtr, off + 4);
            v.z = ReadF(basePtr, off + 8);
            v.u = ReadF(basePtr, off + 12);
            v.v = ReadF(basePtr, off + 16);
            v.r = ReadF(basePtr, off + 20);
            v.g = ReadF(basePtr, off + 24);
            v.b = ReadF(basePtr, off + 28);
            return v;
        }

        // Int32-bits-to-float rather than Marshal.PtrToStructure<float>: no struct marshalling and
        // no dependence on the marshaller agreeing with the native alignment.
        private static float ReadF(IntPtr p, int off)
        {
            return BitConverter.ToSingle(BitConverter.GetBytes(Marshal.ReadInt32(p, off)), 0);
        }

        // The delegates are held in static fields on purpose. If they were locals the GC could
        // collect them while Rust still holds their function pointers, and the crash that follows
        // is a jump to freed memory with no useful stack.
        private static AudioDecodeFn _audioDecode;
        private static AudioMusicNewFn _musicNew;
        private static AudioMusicIdFn _musicPlay;
        private static AudioMusicIdFn _musicPause;
        private static AudioMusicSeekFn _musicSeek;
        private static AudioMusicPositionFn _musicPosition;
        private static AudioMusicPausedFn _musicPaused;
        private static AudioMusicAmpFn _musicAmp;
        private static AudioMusicAmpFn _musicLowPass;
        private static AudioMusicFadeFn _musicFade;
        private static AudioSfxNewFn _sfxNew;
        private static AudioSfxPlayFn _sfxPlay;
        private static PickFileFn _pickFile;
        private static SaveFileFn _saveFile;
        private static PresentFn _present;
        private static LoadAssetFn _loadAsset;
        private static LogFn _log;

        private static bool _installed;

        /// <summary>
        /// How many times Rust has asked for a file this session. Read from a sequence number
        /// rather than a count of successes, because the question it answers is "did the engine get
        /// as far as asking" - a request that fails still advances it.
        /// </summary>
        private static int _loadSeq;

        /// <summary>
        /// True when the managed <see cref="HostCallbacks"/> has the size the Rust struct expects.
        ///
        /// The struct is all function pointers, so on ARM its size *is* the field count times four.
        /// A drift of one field would otherwise only show up as Rust calling a garbage pointer.
        /// </summary>
        public static bool CallbacksLayoutOk()
        {
            return Marshal.SizeOf<HostCallbacks>() == ExpectedCallbackCount * IntPtr.Size;
        }

        /// <summary>
        /// Hand Rust the callbacks it needs. Safe to call more than once.
        ///
        /// Must run before any other call into the port - the shims' audio and file-dialog entry
        /// points forward through this table, and a null entry degrades to "not implemented"
        /// rather than crashing, but it also does nothing useful.
        /// </summary>
        public static void Install()
        {
            if (_installed)
            {
                return;
            }

            _audioDecode = OnAudioDecode;
            _musicNew = OnMusicNew;
            // Play/pause and amp/low-pass used to share one delegate each, so the log could not
            // tell them apart - and they are exactly the pair whose *order* differs between "the
            // music faded out" and "the music was paused". Split, now that they are logged.
            _musicPlay = OnMusicPlay;
            _musicPause = OnMusicPause;
            _musicSeek = OnMusicSeek;
            _musicPosition = OnMusicPosition;
            _musicPaused = OnMusicPaused;
            _musicAmp = OnMusicAmp;
            _musicLowPass = OnMusicLowPass;
            _musicFade = OnMusicFade;
            _sfxNew = OnSfxNew;
            _sfxPlay = OnSfxPlay;
            _pickFile = OnPickFile;
            _saveFile = OnSaveFile;
            _present = OnPresent;
            _loadAsset = OnLoadAsset;
            _log = OnLog;

            HostCallbacks cb = new HostCallbacks();
            cb.audio_decode = Marshal.GetFunctionPointerForDelegate(_audioDecode);
            cb.audio_music_new = Marshal.GetFunctionPointerForDelegate(_musicNew);
            cb.audio_music_play = Marshal.GetFunctionPointerForDelegate(_musicPlay);
            cb.audio_music_pause = Marshal.GetFunctionPointerForDelegate(_musicPause);
            cb.audio_music_seek = Marshal.GetFunctionPointerForDelegate(_musicSeek);
            cb.audio_music_position = Marshal.GetFunctionPointerForDelegate(_musicPosition);
            cb.audio_music_paused = Marshal.GetFunctionPointerForDelegate(_musicPaused);
            cb.audio_music_set_amplifier = Marshal.GetFunctionPointerForDelegate(_musicAmp);
            cb.audio_music_set_low_pass = Marshal.GetFunctionPointerForDelegate(_musicLowPass);
            cb.audio_music_fade = Marshal.GetFunctionPointerForDelegate(_musicFade);
            cb.audio_sfx_new = Marshal.GetFunctionPointerForDelegate(_sfxNew);
            cb.audio_sfx_play = Marshal.GetFunctionPointerForDelegate(_sfxPlay);
            cb.pick_file = Marshal.GetFunctionPointerForDelegate(_pickFile);
            cb.save_file = Marshal.GetFunctionPointerForDelegate(_saveFile);
            cb.present = Marshal.GetFunctionPointerForDelegate(_present);
            cb.load_asset = Marshal.GetFunctionPointerForDelegate(_loadAsset);
            cb.log = Marshal.GetFunctionPointerForDelegate(_log);

            wp81_install_callbacks(ref cb);
            _installed = true;

            // The struct's size is the one thing that turns "Rust called a callback" into "Rust
            // called the *right* callback": it is all function pointers, so a one-field drift means
            // Rust dereferences a garbage address the moment the first callback fires - on the phone
            // that looks like the process simply disappearing, with no managed exception anywhere.
            // Worth one line at startup, before anything has a chance to call through it.
            AppLog.WriteSync("PrprPort: callbacks=" + Marshal.SizeOf<HostCallbacks>()
                + " bytes expected=" + (ExpectedCallbackCount * IntPtr.Size)
                + " layoutOk=" + CallbacksLayoutOk());
        }

        // ===========================================================================
        // the callbacks themselves
        //
        // Each one is a stub for now, except decode, which is real enough to prove the table works.
        // They are written as the shape the shell needs rather than as placeholders that throw -
        // a throwing callback would cross back into Rust as an unhandled exception and take the
        // process down with no useful message.
        // ===========================================================================

        /// <summary>
        /// Rate gate for the callbacks that fire every frame.
        ///
        /// `audio_music_position` and `audio_music_paused` are read once per frame, and the judge
        /// fires `audio_sfx_play` on every tap; printing them unconditionally would push every
        /// other line out of the log's tail, which is precisely the failure this diagnostic exists
        /// to avoid. One line per second of them is enough to tell a clock that is advancing from
        /// one pinned at 0. Two separate gates, because sharing one would let the music position
        /// starve the sfx line into invisibility.
        /// </summary>
        private static int _musicLogTick = int.MinValue / 2;
        private static int _sfxLogTick = int.MinValue / 2;

        private static bool AudioLogDue(ref int tick)
        {
            int now = Environment.TickCount;
            if (unchecked(now - tick) < 1000)
            {
                return false;
            }
            tick = now;
            return true;
        }

        private static int OnAudioDecode(IntPtr data, IntPtr len, IntPtr outInfo)
        {
            // The shell already knows how to read these files; wiring it up is the next step. Until
            // then report failure so the Rust side surfaces it as an error rather than silence.
            //
            // Logged unconditionally, because this is the single callback that decides whether the
            // chart has a soundtrack at all: the `sasa` shim turns any non-zero rc into `Err`, and
            // `load_local_tuple` propagates it with `?`, so a chart can be launched and drawn while
            // this has never succeeded. "Silent because nothing called it" and "silent because the
            // decode failed" were the same line before this existed.
            AppLog.WriteSync("PrprPort: audio decode <- " + len.ToInt64()
                + " bytes (NOT WIRED, returning 1)");
            return 1;
        }

        private static uint OnMusicNew(uint clip, double rate, float amp, double loopMix)
        {
            AppLog.WriteSync("PrprPort: audio music_new clip=" + clip
                + " rate=" + rate.ToString("0.###")
                + " amp=" + amp.ToString("0.###")
                + " loop=" + loopMix.ToString("0.###")
                + " (NOT WIRED, returning 0)");
            return 0;
        }

        private static void OnMusicPlay(uint id)
        {
            AppLog.WriteSync("PrprPort: audio music_play id=" + id);
        }

        private static void OnMusicPause(uint id)
        {
            AppLog.WriteSync("PrprPort: audio music_pause id=" + id);
        }

        private static void OnMusicSeek(uint id, double position)
        {
            AppLog.WriteSync("PrprPort: audio music_seek id=" + id + " -> " + position.ToString("0.000"));
        }

        private static double OnMusicPosition(uint id)
        {
            if (AudioLogDue(ref _musicLogTick))
            {
                AppLog.WriteSync("PrprPort: audio music_position id=" + id + " -> 0.0 (NOT WIRED)");
            }
            return 0.0;
        }

        private static int OnMusicPaused(uint id)
        {
            if (AudioLogDue(ref _musicLogTick))
            {
                AppLog.WriteSync("PrprPort: audio music_paused id=" + id + " -> 0 (NOT WIRED)");
            }
            return 0;
        }

        private static void OnMusicAmp(uint id, float amp)
        {
            AppLog.WriteSync("PrprPort: audio music_set_amplifier id=" + id
                + " amp=" + amp.ToString("0.###"));
        }

        private static void OnMusicLowPass(uint id, float cutoff)
        {
            AppLog.WriteSync("PrprPort: audio music_set_low_pass id=" + id
                + " cutoff=" + cutoff.ToString("0.###"));
        }

        private static void OnMusicFade(uint id, double seconds, int outFlag)
        {
            AppLog.WriteSync("PrprPort: audio music_fade id=" + id
                + " " + seconds.ToString("0.###") + "s out=" + outFlag);
        }

        private static uint OnSfxNew(uint clip)
        {
            AppLog.WriteSync("PrprPort: audio sfx_new clip=" + clip);
            return 0;
        }

        private static void OnSfxPlay(uint id, float amp)
        {
            if (AudioLogDue(ref _sfxLogTick))
            {
                AppLog.WriteSync("PrprPort: audio sfx_play id=" + id
                    + " amp=" + amp.ToString("0.###"));
            }
        }

        private static int OnPickFile(IntPtr title, IntPtr outPath, IntPtr cap)
        {
            return 0;
        }

        private static int OnSaveFile(IntPtr title, IntPtr suggested, IntPtr outPath, IntPtr cap)
        {
            return 0;
        }

        private static void OnPresent()
        {
        }

        // ===========================================================================
        // asset loading
        //
        // The Rust game asks the shell for a file by a path relative to the app's local `charts\`
        // folder. This is the only file IO the Rust side has - it owns none of its own, which is
        // the whole point of the split: WP8.1's brokered storage is reachable from managed code
        // and from nowhere else.
        //
        // SYNCHRONOUS BY NECESSITY. The call arrives on the UI thread from inside the Rust frame
        // loop, so there is no thread to await on: an `await` here would either deadlock (the
        // continuation needs the UI thread that is blocked in Rust) or, unwound through the FFI
        // boundary, take the process down. `.AsTask().Result` is the one form that works, and it
        // is already the established pattern in `NativeProbe.cs` and `D3D11Probe.cs`.
        //
        // Result codes: byte count on success; -1 when the file is not there; -2 when it exists but
        // does not fit the caller's buffer. The Rust side treats anything negative as "absent", so
        // the distinction is for the log rather than for control flow.
        // ===========================================================================

        /// <summary>Where a chart's files live. Set before the Rust game is created.</summary>
        public static string AssetRoot
        {
            get { return _assetRoot; }
            set
            {
                if (_assetRoot != value)
                {
                    _assetRoot = value;
                    _assetCache.Clear();
                }
            }
        }

        private static string _assetRoot;

        /// <summary>
        /// Whole-file cache, keyed by the relative name.
        ///
        /// The game re-requests the same illustrations and the font on every scene entry (and the
        /// hold texture every frame in the worst case), and each uncached read is a brokered
        /// StorageFile round trip that blocks the UI thread. Charts are a few megabytes, so holding
        /// them is cheap next to the stalls it removes.
        /// </summary>
        private static readonly System.Collections.Generic.Dictionary<string, byte[]> _assetCache =
            new System.Collections.Generic.Dictionary<string, byte[]>();

        private static int OnLoadAsset(IntPtr name, IntPtr outBytes, IntPtr cap)
        {
            // Logged before anything can refuse.
            //
            // The first device run produced *no* asset lines at all while `GameCreate` never
            // returned, and the emptiness was ambiguous: several of the early returns below used to
            // bail silently, so "Rust never asked for anything" and "Rust asked and the pointer was
            // turned away" were byte-for-byte the same empty log. This line separates them, and the
            // sequence number records how far a load got even when the next call never comes back -
            // which is exactly what happened on the phone.
            try
            {
                int seq = System.Threading.Interlocked.Increment(ref _loadSeq);
                string nm = name == IntPtr.Zero
                    ? "(null)"
                    : (Marshal.PtrToStringUni(name) ?? "(not utf16)");
                AppLog.WriteSync("PrprPort: load_asset#" + seq + " " + nm + " cap=" + cap.ToInt64());
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("PrprPort: load_asset entry unreadable", ex);
            }

            try
            {
                if (name == IntPtr.Zero || outBytes == IntPtr.Zero)
                {
                    AppLog.WriteSync("PrprPort: load_asset refused: null name or buffer");
                    return -1;
                }

                string relative = Marshal.PtrToStringUni(name);
                if (string.IsNullOrEmpty(relative))
                {
                    AppLog.WriteSync("PrprPort: load_asset refused: empty name");
                    return -1;
                }

                byte[] data;
                if (!_assetCache.TryGetValue(relative, out data))
                {
                    data = ReadAsset(relative);
                    if (data == null)
                    {
                        AppLog.WriteSync("PrprPort: asset missing: " + relative);
                        return -1;
                    }
                    _assetCache[relative] = data;

                    // Progress, not just success.
                    //
                    // On the first device run `GameCreate` never returned and there was no way to
                    // tell "still working" from "died in the next step": every launch ended at the
                    // same `engine create` line with nothing after it. The last asset named here is
                    // the one whose bytes the engine had just been handed, which narrows "it never
                    // came back" to one file - a JSON that fails to parse, a PNG that dies in the
                    // decoder, a texture upload that trips the GPU - instead of to the whole load.
                    // Zero of these lines is itself an answer: it means the engine never got as far
                    // as asking for anything.
                    //
                    // First read only. The engine re-requests images on every scene entry, and
                    // logging those would bury the one that matters.
                    AppLog.WriteSync("PrprPort: asset " + relative + " -> " + data.Length + " bytes");
                }

                int want = cap.ToInt32();
                if (data.Length > want)
                {
                    // The caller sized its buffer from a promise we cannot see; refusing is better
                    // than copying a prefix and letting it parse a truncated chart.
                    AppLog.WriteSync("PrprPort: asset too large: " + relative
                        + " (" + data.Length + " > " + want + ")");
                    return -2;
                }

                Marshal.Copy(data, 0, outBytes, data.Length);
                return data.Length;
            }
            catch (Exception ex)
            {
                // Never let an exception cross back into Rust: it would unwind through frames that
                // have no idea how to handle it.
                AppLog.WriteSync("PrprPort: load_asset failed", ex);
                return -1;
            }
        }

        /// <summary>
        /// Rust's line for the log.
        ///
        /// Synchronous, and callable from whatever thread Rust happens to be on, because of *when*
        /// it is called: almost all of these arrive while the shell is blocked inside
        /// <c>PhiraBoot</c> waiting on the very code that is producing them. A queued write would
        /// be sitting in memory at the moment the process died, which is precisely when the line
        /// saying which phase was running matters most.
        ///
        /// The <c>rust: </c> prefix is deliberate - a line that came out of Rust must never be
        /// mistaken for one the shell wrote about Rust.
        /// </summary>
        private static void OnLog(IntPtr msg, IntPtr len)
        {
            try
            {
                int n = len.ToInt32();
                if (msg == IntPtr.Zero || n <= 0)
                {
                    return;
                }
                if (n > 8192)
                {
                    // A length that large is a bad pointer or a bug, not a message; refusing keeps
                    // one broken call from turning into a multi-megabyte allocation.
                    n = 8192;
                }
                byte[] bytes = new byte[n];
                Marshal.Copy(msg, bytes, 0, n);
                AppLog.WriteSync("rust: " + System.Text.Encoding.UTF8.GetString(bytes, 0, n));
            }
            catch (Exception)
            {
                // Diagnostics must never be the fault, and an exception unwinding back into Rust
                // has no handler to land in.
            }
        }

        /// <summary>
        /// Reads one asset into memory, synchronously.
        ///
        /// Relative names resolve under <see cref="AssetRoot"/>, which is either a chart folder in
        /// local storage or the bundled `ms-appx:` demo. Both are read through the same broker
        /// calls, so there is no second code path to keep correct.
        /// </summary>
        private static byte[] ReadAsset(string relative)
        {
            string root = _assetRoot;
            if (string.IsNullOrEmpty(root))
            {
                return null;
            }

            string normalised = relative.Replace('/', '\\').TrimStart('\\');
            if (normalised.Length == 0)
            {
                return null;
            }

            // One segment at a time.
            //
            // The engine asks for the *whole* path - `190/info.yml`, `rank/phi.png` - relative to
            // the charts root, so the name handed to WinRT is usually a sub-path. Asking WinRT for
            // that in one call does not resolve the intermediate folder: it reports the file as
            // missing, and because the engine treats any negative result as "absent" that would
            // present as every chart failing to load rather than as an IO error. Walking the
            // segments is the form that works, and it is what `ChartLibrary` does for the same
            // reason. The whole-file cache above means this walk happens once per distinct asset,
            // not once per frame.
            Windows.Storage.StorageFolder folder =
                Windows.Storage.StorageFolder.GetFolderFromPathAsync(root).AsTask().Result;

            string[] parts = normalised.Split(new char[] { '\\' },
                StringSplitOptions.RemoveEmptyEntries);
            for (int i = 0; i + 1 < parts.Length; i++)
            {
                folder = folder.GetFolderAsync(parts[i]).AsTask().Result;
            }
            Windows.Storage.StorageFile file = folder.GetFileAsync(parts[parts.Length - 1]).AsTask().Result;

            Windows.Storage.Streams.IBuffer buffer =
                Windows.Storage.FileIO.ReadBufferAsync(file).AsTask().Result;

            byte[] bytes = new byte[buffer.Length];
            using (Windows.Storage.Streams.DataReader reader =
                   Windows.Storage.Streams.DataReader.FromBuffer(buffer))
            {
                reader.ReadBytes(bytes);
            }
            return bytes;
        }

        // ===========================================================================
        // diagnostics
        // ===========================================================================

        /// <summary>
        /// Drives one synthetic draw call through the drain and checks it came back intact.
        ///
        /// This is the renderer path's equivalent of the PEC parse: the shim records a triangle,
        /// the shell reads it as a <see cref="PortCmd"/>, and the check confirms the kind, the
        /// vertex count and one vertex coordinate. A layout mismatch between the two sides shows
        /// up here as a wrong tag or garbage floats rather than as a blank screen on the device.
        /// </summary>
        public static string DrainSelfTest()
        {
            try
            {
                if (!WireLayoutOk())
                {
                    return "drain: FAILED (managed/native record size mismatch)";
                }

                wp81_draw_selftest();
                DrainFrame frame = Drain(16, 16);

                if (frame.Count != 1)
                {
                    return "drain: FAILED (expected 1 command, got " + frame.Count + ")";
                }

                PortCmd c = frame.Commands[0];
                if (c.kind != (uint)PortKind.Geometry)
                {
                    return "drain: FAILED (kind=" + c.kind + ", expected Geometry)";
                }
                if (c.vcount != 3)
                {
                    return "drain: FAILED (vcount=" + c.vcount + ", expected 3)";
                }

                // Vertex 1 is (0.5, 0.5) with green at full red channel 0 - if the two structures
                // disagree on layout this reads a neighbouring field instead.
                PortVertex v = frame.Vertices[c.vstart + 1];
                if (Math.Abs(v.x - 0.5f) > 0.001f || Math.Abs(v.g - 1.0f) > 0.001f)
                {
                    return "drain: FAILED (vertex mismatch x=" + v.x + " g=" + v.g + ")";
                }

                return "drain: OK (" + frame.Count + " cmd, " + c.vcount + " verts)";
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("PrprPort: drain self-test failed", ex);
                return "drain: FAILED " + ex.GetType().Name;
            }
        }

        /// <summary>
        /// Proves the whole chain: the DLL loads, the callback table installs, and the real Rust
        /// chart parser runs. Logged rather than shown, so the result survives a crash.
        /// </summary>
        public static void RunSelfTest(string pecText)
        {
            string summary;
            try
            {
                AppLog.WriteSync("PrprPort: DLL version = " + wp81_port_version());

                Install();
                AppLog.WriteSync("PrprPort: callbacks installed");

                byte[] bytes = System.Text.Encoding.UTF8.GetBytes(pecText ?? string.Empty);
                long notes = wp81_parse_pec(bytes, bytes.Length);
                AppLog.WriteSync("PrprPort: wp81_parse_pec returned " + notes);

                ChartInfo info;
                int rc = wp81_chart_info(out info);
                AppLog.WriteSync("PrprPort: chart_info rc=" + rc + " lines=" + info.lines
                    + " notes=" + info.notes + " duration=" + info.duration_ms + "ms");

                summary = "Rust prpr: " + (notes >= 0 ? "OK" : "FAILED") + "\nparse returned " + notes
                    + "\nlines=" + info.lines + " notes=" + info.notes
                    + "\nduration=" + info.duration_ms + " ms";

                // The drain is verified here too: the real scene is not driven from Rust yet, so
                // this is the only way to prove the command records actually cross the boundary.
                summary += "\n" + DrainSelfTest();
            }
            catch (Exception ex)
            {
                AppLog.WriteSync("PrprPort: FAILED", ex);
                summary = "Rust prpr FAILED\n" + ex.GetType().Name + ": " + ex.Message;
            }

            try
            {
                Windows.UI.Popups.MessageDialog dlg =
                    new Windows.UI.Popups.MessageDialog(summary, "Rust prpr port");
#pragma warning disable 4014
                dlg.ShowAsync();
#pragma warning restore 4014
            }
            catch (Exception)
            {
            }
        }
    }
}
