using System;
using System.Collections.Generic;
using System.Text;
using System.Threading.Tasks;
using Windows.Storage;

namespace Phirura
{
    /// <summary>
    /// A minimal on-disk log.
    ///
    /// Debug.WriteLine only reaches the Visual Studio Output pane, which is useless once
    /// the app is on a phone (and it vanishes entirely when the process is killed). Each
    /// line here is appended to `ApplicationData\LocalFolder\phirura.log`; the file is
    /// opened and closed per write, so entries survive a hard crash.
    ///
    /// Pull the file with the VS "Local Folder" browser, IsoStoreSpy, or WP Power Tools.
    /// Writes are queued and drained in order on the thread pool, so logging never blocks
    /// the UI thread. The drain is batched: one broker round-trip per few dozen lines,
    /// not per line - a per-line task storm backed the queue up minutes behind the game
    /// during play, which is what made pulls look "stale" and ISETool collide forever
    /// with a file the app never stopped holding.
    /// </summary>
    public static class AppLog
    {
        // Public: LogServer opens the same file by name. Public rather than a duplicated literal
        // because a literal here that ever drifted from this one would make the HTTP channel
        // serve an empty file forever while the log filled up normally - a silent, total failure
        // of the thing it exists for, with nothing on screen to say so.
        public const string FileName = "phirura.log";
        // 4 MB rather than 256 KB. Rotation keeps only the tail, so at this build's write rate
        // 256 KB discarded roughly the first eight minutes of every session - which is exactly
        // where a reproduction lands. The run that was pulled to investigate "the hit sound cuts
        // the music" had already rotated: what survived was the chart *browser*, with no gameplay,
        // no hits and no `sfx_play` in it at all, so the moment the report describes was
        // unrecoverable by construction. A log that cannot contain the event it is meant to
        // capture is not a diagnostic.
        private const ulong MaxBytes = 4 * 1024 * 1024;

        private static readonly object Gate = new object();
        private static Task _tail = Task.FromResult(0);
        // Lines waiting for the next drain. Swapped out whole under Gate so the drain writes
        // one joined batch per store instead of one broker round-trip per line.
        private static List<string> _pending = new List<string>();

        // Cached file objects, one per store. Fetching them used to cost two broker round
        // trips (CreateFile + GetBasicProperties) on EVERY drain, and the drain is serial -
        // so at play-time line rates the chain never caught up and the frame-stats
        // `WriteSync`, which waits on it once a second, ate the full 500 ms bound every
        // time: worstMs ~520 ms on nearly every fps line, a visible hitch every second or
        // two with rust/draw/present all near zero. A cached StorageFile is not an open
        // file - FileIO opens and closes per append - so this keeps the ISETool windows the
        // open-close rhythm used to provide. Nulled on any failure, refetched next drain.
        private static StorageFile _localFile;
        private static StorageFile _tempFile;
        // Estimated bytes on disk per store, for rotation without a properties call per
        // drain. -1 means unknown (fetch once). UTF-8 counted exactly at append time; the
        // estimate only decides rotation, so drift is harmless.
        private static long _localBytes = -1;
        private static long _tempBytes = -1;

        /// <summary>
        /// Until then, appends are skipped because the last one threw.
        ///
        /// This used to be a plain latch, and that was a mistake worth naming: one failed append
        /// silenced the log for the rest of the process, so a run that died and a run whose logging
        /// had broken produced byte-for-byte the same file - a log that stops mid-sentence and says
        /// nothing about why. The shell now has diagnostics arriving from Rust while the UI thread
        /// is blocked inside native code, which is exactly when an append is most likely to fail, so
        /// a permanent latch would take out the channel that exists to explain a crash.
        ///
        /// A cool-down keeps the original guarantee - "logging must never be the reason the app
        /// dies" - while allowing recovery: at most one failed attempt every few seconds, and once
        /// an attempt succeeds the window simply stays in the past, which is the recovered state.
        ///
        /// Ticks rather than a <see cref="DateTime"/>, and read through <c>Interlocked</c>: this is
        /// written by the append thread and read by any thread that happens to log, and a torn
        /// read of an eight-byte timestamp is not a hypothetical - it is the one bug that could pin
        /// this field in the far future and mute the log permanently, which is the exact failure
        /// this field exists to stop.
        /// </summary>
        private static long _mutedUntilTicks;

        public static void Write(string message)
        {
            string line = DateTime.Now.ToString("HH:mm:ss.fff") + "  " + message;
            System.Diagnostics.Debug.WriteLine("[Phirura] " + line);
            Remember(line);

            if (System.Threading.Interlocked.Read(ref _mutedUntilTicks) > DateTime.UtcNow.Ticks)
            {
                return;
            }
            lock (Gate)
            {
                _pending.Add(line);
                EnsureDrainLocked();
            }
        }

        /// <summary>
        /// Schedules a drain when the chain is idle. Gate must be held. Split out because
        /// three call sites need exactly this: Write, WriteSync, and the drain's own re-arm.
        /// </summary>
        private static void EnsureDrainLocked()
        {
            if (_tail.IsCompleted)
            {
                _tail = _tail.ContinueWith(delegate { Drain(); }, TaskScheduler.Default);
            }
        }

        /// <summary>
        /// Writes a line and does not return until it is on disk.
        ///
        /// <see cref="Write"/> only *queues* the append, which is the right default but useless
        /// when the very next thing that happens is the process dying: the queued continuation
        /// never runs and the final lines - the interesting ones - are simply absent from the
        /// file. That is exactly what made the native-probe runs so hard to read.
        ///
        /// The continuation must be pinned to <see cref="TaskScheduler.Default"/>. Without it,
        /// <c>ContinueWith</c> uses <c>TaskScheduler.Current</c>, which on the UI thread is the
        /// dispatcher - so the append would be posted *back* to the UI thread and waiting for it
        /// here would deadlock.
        /// </summary>
        public static void WriteSync(string message)
        {
            string line = DateTime.Now.ToString("HH:mm:ss.fff") + "  " + message;
            System.Diagnostics.Debug.WriteLine("[Phirura] " + line);
            Remember(line);

            if (System.Threading.Interlocked.Read(ref _mutedUntilTicks) > DateTime.UtcNow.Ticks)
            {
                return;
            }
            lock (Gate)
            {
                Task previous = _tail;
                _pending.Add(line);
                if (!previous.IsCompleted)
                {
                    // The chain is already wedged: an append is stuck, which is what happens the
                    // moment anything - ISETool, most often - holds phirura.log open. Waiting
                    // again buys nothing, because the line just queued is behind the stuck one and
                    // cannot run until it clears. This is not a micro-optimisation: this runs on
                    // the UI thread with `Gate` held, so one 500 ms wait *per logged line* is what
                    // turned "someone is reading the log" into a frozen screen - the frame never
                    // got past the first second of texture construction, which is also the second
                    // half of why text was on screen as white blocks. At most one wait is now
                    // ever spent on any one wedge. The line is not lost: the running drain
                    // re-arms itself while anything is still pending (see Drain), so it rides the
                    // drain that clears the wedge instead of a wait that cannot.
                    return;
                }
                EnsureDrainLocked();
                try
                {
                    // Bounded: logging must never be the reason the app hangs either.
                    //
                    // 500 ms, not the 3 s it used to be. The append still completes - it is queued -
                    // and the wait exists only so the last few lines survive a crash. Three seconds
                    // was long enough to matter: this runs on the UI thread once a second (the frame
                    // stats line), so a queue backed up by a burst of touch lines blocked the render
                    // loop for seconds at a time, which is exactly the "picture froze but the game
                    // is still running" that showed up on touch.
                    _tail.Wait(500);
                }
                catch (Exception)
                {
                }
            }
        }

        // ===================================================================================
        // The last few lines, for the on-screen panel.
        // ===================================================================================
        //
        // Kept here rather than in the page because the alternatives are both worse: re-reading the
        // file is expensive and the file is held open a lot, and duplicating the text at every call
        // site means the panel and the file drift apart. A ring buffer of already-formatted strings,
        // written under the same lock the appends take, so the panel can never render a half-written
        // line - and it costs a string reference per log call, not a copy.
        private const int RecentMax = 20;
        private static readonly string[] _recent = new string[RecentMax];
        private static int _recentNext;
        private static int _recentTotal;

        private static void Remember(string line)
        {
            lock (Gate)
            {
                _recent[_recentNext] = line;
                _recentNext = (_recentNext + 1) % RecentMax;
                _recentTotal++;
            }
        }

        /// <summary>
        /// The most recent lines, oldest first, plus a counter that changes whenever they do.
        ///
        /// The counter is what lets the caller poll cheaply: it compares the number and only rebuilds
        /// the string when it moved, so a panel refreshing a few times a second costs nothing on the
        /// frames where no line was logged.
        /// </summary>
        public static string RecentText(out int version)
        {
            lock (Gate)
            {
                version = _recentTotal;
                int n = _recentTotal < RecentMax ? _recentTotal : RecentMax;
                System.Text.StringBuilder sb = new System.Text.StringBuilder(n * 48);
                for (int i = 0; i < n; i++)
                {
                    int idx = ((_recentNext - n + i) % RecentMax + RecentMax) % RecentMax;
                    sb.Append(_recent[idx]).Append('\n');
                }
                return sb.ToString();
            }
        }

        public static void Write(string context, Exception ex)
        {
            if (ex == null)
            {
                Write(context);
                return;
            }
            Write(context + " :: " + Describe(ex));
        }

        /// <summary>Synchronous counterpart of <see cref="Write(string, Exception)"/>.</summary>
        public static void WriteSync(string context, Exception ex)
        {
            if (ex == null)
            {
                WriteSync(context);
                return;
            }
            WriteSync(context + " :: " + Describe(ex));
        }

        /// <summary>
        /// The exception, then everything it wraps, then the stack of the one that actually failed.
        ///
        /// Every `.Result` in this app throws `AggregateException`, whose message is the fixed
        /// string "One or more errors occurred." - so the old `Type + Message + StackTrace` form
        /// printed a stack through `ReadAsset` pointing at four different calls and a message that
        /// could have been any of them. Ten asset requests failed that way on the device and the log
        /// could not say whether the charts folder was absent, a file was missing, or the broker had
        /// refused the path. The chain is the whole content of the line; the stack only says which
        /// `.Result` rethrew it, which the innermost one does better.
        /// </summary>
        private static string Describe(Exception ex)
        {
            System.Text.StringBuilder sb = new System.Text.StringBuilder();
            Exception deepest = ex;
            for (Exception e = ex; e != null; )
            {
                if (sb.Length > 0)
                {
                    sb.Append(" -> ");
                }
                sb.Append(e.GetType().Name).Append(": ").Append(e.Message);
                deepest = e;

                AggregateException agg = e as AggregateException;
                if (agg != null && agg.InnerExceptions.Count > 0)
                {
                    e = agg.InnerExceptions[0];
                    continue;
                }
                e = e.InnerException;
            }

            string stack = deepest.StackTrace;
            if (string.IsNullOrEmpty(stack))
            {
                stack = ex.StackTrace;
            }
            return sb.ToString() + Environment.NewLine + stack;
        }

        /// <summary>Runs on a thread-pool thread; the WinRT I/O is awaited there, never on the UI thread.</summary>
        private static void Drain()
        {
            List<string> batch;
            lock (Gate)
            {
                if (_pending.Count == 0)
                {
                    return;
                }
                batch = _pending;
                _pending = new List<string>();
            }
            StringBuilder sb = new StringBuilder(batch.Count * 48);
            for (int i = 0; i < batch.Count; i++)
            {
                sb.Append(batch[i]);
                sb.Append("\r\n");
            }
            string text = sb.ToString();

            // Two stores, each in its own try: one failing must not cost the other.
            //
            // The second copy is not redundancy for its own sake. `ISETool ts` copies the WHOLE
            // Local store and *aborts* on the first file it cannot open - and that file is the
            // chart's music, held by the MediaElement for as long as the game is playing. So the log
            // is the one file a pull most needs and the one it is least likely to reach: every
            // "no log" was really "the copy died at the mp3, which sorts before it". The temporary
            // store holds nothing but this file, so `ts appxfolder:Temp` is reliable and instant -
            // no 22 MB of chart art to walk first.
            TryAppend(ApplicationData.Current.LocalFolder, text, ref _localFile, ref _localBytes);
            TryAppend(ApplicationData.Current.TemporaryFolder, text, ref _tempFile, ref _tempBytes);

            // Re-arm: lines that arrived while this drain owned no lock ride the next drain
            // instead of waiting for a Write that may never come. The wedged-WriteSync early
            // return above relies on exactly this - without it a line queued behind a stuck
            // append would sit in `_pending` until some unrelated later line scheduled a drain.
            lock (Gate)
            {
                if (_pending.Count > 0)
                {
                    EnsureDrainLocked();
                }
            }
        }

        private static void TryAppend(StorageFolder folder, string text,
            ref StorageFile cached, ref long bytes)
        {
            try
            {
                StorageFile file = cached;
                if (file == null)
                {
                    file = folder.CreateFileAsync(
                        FileName, CreationCollisionOption.OpenIfExists).AsTask().Result;
                    cached = file;
                }
                if (bytes < 0)
                {
                    Windows.Storage.FileProperties.BasicProperties props =
                        file.GetBasicPropertiesAsync().AsTask().Result;
                    bytes = props == null ? 0 : (long)props.Size;
                }
                int upcoming = System.Text.Encoding.UTF8.GetByteCount(text);
                if ((ulong)(bytes + upcoming) > MaxBytes)
                {
                    // Keep the tail; do not start over. This used to delete the file and recreate it
                    // empty, which threw away the *beginning* of the run - and the beginning is
                    // where the chart load, the texture registrations and the first gameplay frames
                    // are. A ten-minute session therefore lost the whole playable part and left only
                    // the result page, which reads as "the game never rendered anything" and sent a
                    // whole round hunting a bug that had already been fixed. The per-frame lines are
                    // also rate-limited, so a rotation should now be rare rather than routine.
                    string all = Windows.Storage.FileIO.ReadTextAsync(file).AsTask().Result;
                    int keep = all.Length / 2;
                    if (keep > all.Length)
                    {
                        keep = all.Length;
                    }
                    string tail = all.Substring(all.Length - keep);
                    // Start on a line boundary so the file never opens mid-sentence.
                    int nl = tail.IndexOf('\n');
                    if (nl >= 0 && nl + 1 < tail.Length)
                    {
                        tail = tail.Substring(nl + 1);
                    }
                    string rotated = "...(log rotated; kept the last " + tail.Length + " of " + all.Length
                        + " chars)...\r\n" + tail;
                    FileIO.WriteTextAsync(file, rotated).AsTask().Wait();
                    bytes = System.Text.Encoding.UTF8.GetByteCount(rotated);
                }

                FileIO.AppendTextAsync(file, text).AsTask().Wait();
                bytes += upcoming;
            }
            catch (Exception)
            {
                // Drop the cached objects: the next drain refetches both, which is also what
                // recovers from an externally deleted or replaced file. Size resets to unknown
                // for the same reason.
                cached = null;
                bytes = -1;
                // Logging must never be the reason the app dies.
                System.Threading.Interlocked.Exchange(
                    ref _mutedUntilTicks, DateTime.UtcNow.AddSeconds(5).Ticks);
            }
        }

        // ===================================================================================
        // Watchdog-friendly write.
        // ===================================================================================
        //
        // The background watchdog calls this. It must be usable from a pool thread while the UI
        // thread is busy inside native code. `Append` goes through WinRT `StorageFile`/`FileIO`,
        // whose broker serialises calls - but because the CoreWindow bring-up runs on its *own*
        // pool thread (not the UI thread), the UI thread is free and the broker keeps completing.
        // Keeping it on the same proven path is deliberate: an earlier attempt to bypass WinRT with
        // `System.IO.File` does not compile on this profile, and pulling `std::fs` into the native
        // DLL ballooned it 19 KB -> 116 KB and made the DLL fail to load.
        public static void WriteRaw(string message)
        {
            WriteSync(message);
        }
    }
}
