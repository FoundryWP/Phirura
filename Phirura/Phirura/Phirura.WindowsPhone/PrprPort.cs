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
        /// Mirrors <c>HostCallbacks</c> in the Rust port.
        ///
        /// Laid out by hand rather than with <c>[MarshalAs]</c> because it is passed by pointer,
        /// not marshalled: the Rust side copies the struct wholesale, so the layout has to be the
        /// natural one for the target. Every field is a function pointer (4 bytes on ARM), which
        /// makes the layout uniform.
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
        }

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

        private static bool _installed;

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
