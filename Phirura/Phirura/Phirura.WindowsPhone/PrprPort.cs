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

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_frame(double dt);

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_game_destroy();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern long wp81_game_frames();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern int wp81_game_running();

        [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
        private static extern void wp81_game_set_screen(float width, float height);

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
        /// Mirrors <c>HostCallbacks</c> in the Rust port.
        ///
        /// Laid out by hand rather than with <c>[MarshalAs]</c> because it is passed by pointer,
        /// not marshalled: the Rust side copies the struct wholesale, so the layout has to be the
        /// natural one for the target. Every field is a function pointer (4 bytes on ARM), which
        /// makes the layout uniform.
        ///
        /// FIELD ORDER IS THE ABI. <c>load_asset</c> is appended last to match the Rust struct - if
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
        }

        /// <summary>
        /// Number of function pointers in the Rust <c>HostCallbacks</c>. Kept as a literal because
        /// the C# struct cannot be counted at compile time; the point is that a mismatch is caught
        /// by <see cref="CallbacksLayoutOk"/> instead of by a crash inside a Rust callback.
        /// </summary>
        private const int ExpectedCallbackCount = 16;

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
        /// Reports the shell's surface size to the Rust renderer.
        ///
        /// Must be called once after the drawing surface is measured and before the first frame:
        /// <c>prpr</c> projects through statics that default to 0x0, and at 0x0 the aspect ratio is
        /// degenerate so the whole chart collapses to a single point.
        /// </summary>
        public static void GameSetScreen(float width, float height)
        {
            wp81_game_set_screen(width, height);
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

        private static bool _installed;

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
            _musicPlay = OnMusicIdNoop;
            _musicPause = OnMusicIdNoop;
            _musicSeek = OnMusicSeek;
            _musicPosition = OnMusicPosition;
            _musicPaused = OnMusicPaused;
            _musicAmp = OnMusicAmp;
            _musicLowPass = OnMusicAmp;
            _musicFade = OnMusicFade;
            _sfxNew = OnSfxNew;
            _sfxPlay = OnSfxPlay;
            _pickFile = OnPickFile;
            _saveFile = OnSaveFile;
            _present = OnPresent;
            _loadAsset = OnLoadAsset;

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

            wp81_install_callbacks(ref cb);
            _installed = true;
        }

        // ===========================================================================
        // the callbacks themselves
        //
        // Each one is a stub for now, except decode, which is real enough to prove the table works.
        // They are written as the shape the shell needs rather than as placeholders that throw -
        // a throwing callback would cross back into Rust as an unhandled exception and take the
        // process down with no useful message.
        // ===========================================================================

        private static int OnAudioDecode(IntPtr data, IntPtr len, IntPtr outInfo)
        {
            // The shell already knows how to read these files; wiring it up is the next step. Until
            // then report failure so the Rust side surfaces it as an error rather than silence.
            return 1;
        }

        private static uint OnMusicNew(uint clip, double rate, float amp, double loopMix)
        {
            return 0;
        }

        private static void OnMusicIdNoop(uint id)
        {
        }

        private static void OnMusicSeek(uint id, double position)
        {
        }

        private static double OnMusicPosition(uint id)
        {
            return 0.0;
        }

        private static int OnMusicPaused(uint id)
        {
            return 0;
        }

        private static void OnMusicAmp(uint id, float amp)
        {
        }

        private static void OnMusicFade(uint id, double seconds, int outFlag)
        {
        }

        private static uint OnSfxNew(uint clip)
        {
            return 0;
        }

        private static void OnSfxPlay(uint id, float amp)
        {
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
            try
            {
                if (name == IntPtr.Zero || outBytes == IntPtr.Zero)
                {
                    return -1;
                }

                string relative = Marshal.PtrToStringUni(name);
                if (string.IsNullOrEmpty(relative))
                {
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

            Windows.Storage.StorageFile file =
                Windows.Storage.StorageFolder.GetFolderFromPathAsync(root).AsTask().Result
                    .GetFileAsync(normalised).AsTask().Result;

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
