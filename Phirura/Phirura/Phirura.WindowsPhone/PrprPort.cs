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
