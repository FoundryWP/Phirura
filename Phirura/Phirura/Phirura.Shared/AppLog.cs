using System;
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
    /// the UI thread.
    /// </summary>
    public static class AppLog
    {
        private const string FileName = "phirura.log";
        private const ulong MaxBytes = 256 * 1024;

        private static readonly object Gate = new object();
        private static Task _tail = Task.FromResult(0);
        private static bool _failed;

        public static void Write(string message)
        {
            string line = DateTime.Now.ToString("HH:mm:ss.fff") + "  " + message;
            System.Diagnostics.Debug.WriteLine("[Phirura] " + line);

            if (_failed)
            {
                return;
            }
            lock (Gate)
            {
                _tail = _tail.ContinueWith(delegate { Append(line); }, TaskScheduler.Default);
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

            if (_failed)
            {
                return;
            }
            lock (Gate)
            {
                _tail = _tail.ContinueWith(delegate { Append(line); }, TaskScheduler.Default);
                try
                {
                    // Bounded: logging must never be the reason the app hangs either.
                    _tail.Wait(3000);
                }
                catch (Exception)
                {
                }
            }
        }

        public static void Write(string context, Exception ex)
        {
            if (ex == null)
            {
                Write(context);
                return;
            }
            Write(context + " :: " + ex.GetType().Name + ": " + ex.Message
                + Environment.NewLine + ex.StackTrace);
        }

        /// <summary>Synchronous counterpart of <see cref="Write(string, Exception)"/>.</summary>
        public static void WriteSync(string context, Exception ex)
        {
            if (ex == null)
            {
                WriteSync(context);
                return;
            }
            WriteSync(context + " :: " + ex.GetType().Name + ": " + ex.Message
                + Environment.NewLine + ex.StackTrace);
        }

        /// <summary>Runs on a thread-pool thread; the WinRT I/O is awaited there, never on the UI thread.</summary>
        private static void Append(string line)
        {
            try
            {
                StorageFolder folder = ApplicationData.Current.LocalFolder;
                StorageFile file = folder.CreateFileAsync(
                    FileName, CreationCollisionOption.OpenIfExists).AsTask().Result;

                Windows.Storage.FileProperties.BasicProperties props =
                    file.GetBasicPropertiesAsync().AsTask().Result;
                if (props != null && props.Size > MaxBytes)
                {
                    file.DeleteAsync().AsTask().Wait();
                    file = folder.CreateFileAsync(
                        FileName, CreationCollisionOption.ReplaceExisting).AsTask().Result;
                }

                StringBuilder sb = new StringBuilder(line.Length + 2);
                sb.Append(line);
                sb.Append("\r\n");
                FileIO.AppendTextAsync(file, sb.ToString()).AsTask().Wait();
            }
            catch (Exception)
            {
                // Logging must never be the reason the app dies.
                _failed = true;
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
