using System;
using System.Threading.Tasks;
using Windows.Networking;
using Windows.Networking.Sockets;
using Windows.Storage;
using Windows.Storage.Streams;

namespace Phirura
{
    /// <summary>
    /// Serves phirura.log over HTTP so it can be read *while the app is running*.
    ///
    /// The alternative has never been quick. `ISETool ts` copies the whole IsolatedStore and
    /// aborts on the first file it cannot open - and that file is the chart's mp3, held by the
    /// MediaElement for exactly as long as the game is playing. So the pull that is most needed
    /// is the one most likely to fail, it only ever returns the file as of the instant it ran, and
    /// it costs a command plus a wait each time. Reading a log you are watching should not cost a
    /// deploy cycle's worth of ceremony. This costs one curl, as often as you like, mid-run.
    ///
    /// HTTP rather than a bespoke protocol because the client already exists everywhere - a
    /// browser, curl, PowerShell - and because the request line carries the path, so one listening
    /// socket can serve the whole file, only its tail, or only what is new since the caller last
    /// asked. That last mode is the one that makes watching a live run practical: a 4 MB log
    /// re-sent every poll would spend most of the Wi-Fi re-sending what was already seen.
    ///
    /// A listener rather than an outbound push because the phone has nowhere to push to: only
    /// ISETool and AppDeployCmd speak to it over USB, and there is no reverse channel. This way
    /// the PC asks when it wants.
    ///
    /// Not gated on a debug flag, deliberately: the whole point is a channel that is already open
    /// when something goes wrong. It exposes a log file and nothing else, on a development phone.
    /// </summary>
    public static class LogServer
    {
        /// <summary>
        /// Fixed rather than ephemeral. A port that changes between runs is a port you have to
        /// look up before every request, which is most of the ceremony this exists to remove.
        /// </summary>
        public const int Port = 8765;

        private static StreamSocketListener _listener;
        private static int _started;
        private static int _requests;
        private static string _state = "not started";

        /// <summary>
        /// The listener's state, for a line that has to say whether the HTTP channel is still
        /// there. Read rather than re-probed: asking the OS instead would answer a different
        /// question (does this machine have an address) from the one worth logging (did this
        /// listener ever bind, and is it still bound).
        /// </summary>
        public static string State
        {
            get { return _state; }
        }

        /// <summary>
        /// Starts the listener. Idempotent, never throws, and safe to call from the UI thread.
        /// </summary>
        public static void Start()
        {
            if (System.Threading.Interlocked.Exchange(ref _started, 1) != 0)
            {
                return;
            }
            try
            {
                StreamSocketListener listener = new StreamSocketListener();
                listener.ConnectionReceived += OnConnection;
                // Held in a field on purpose: the listener is only listening for as long as
                // something owns the wrapper, and nothing else in the app has a reason to keep
                // one - a collector deciding this reference is dead would stop the server.
                _listener = listener;
                _state = "binding";

                // Neither awaited nor `.Result`. This runs on the UI thread inside App's
                // constructor, and WinRT resumes an awaited operation on the context it was
                // started from - so waiting here for a completion that needs this very thread is
                // a hang on the first launch, which would be a far worse bug than no log server.
                // The continuation goes to the pool and reports through AppLog like everything
                // else, including a bind failure, which is a real outcome (port in use).
                listener.BindServiceNameAsync(Port.ToString()).AsTask().ContinueWith(
                    delegate(Task bind)
                    {
                        if (bind.IsFaulted)
                        {
                            _state = "bind failed";
                            AppLog.Write("LogServer: bind " + Port + " failed", bind.Exception);
                            return;
                        }
                        _state = "listening";
                        string url = LocalUrl();
                        // Sync, and explicit about the unreachable case. This is the one line
                        // that says whether the HTTP channel exists; "(no IPv4 yet)" read as a
                        // transient state that would sort itself out, and it does not sort
                        // itself out - there is then no HTTP channel left to ask. The file is
                        // still readable through ISETool, so this is also the one place that
                        // can say so and hand back the command that still works.
                        AppLog.WriteSync("LogServer: listening on port " + Port
                            + (url == null
                                ? " -> unreachable: no non-loopback IPv4 (Wi-Fi off, or no DHCP lease). "
                                  + "Read this file instead: stage-chart.ps1 -PullLog"
                                : " -> " + url + "/log"));
                    }, TaskScheduler.Default);
            }
            catch (Exception ex)
            {
                _state = "start failed";
                AppLog.Write("LogServer: start", ex);
            }
        }

        private static void OnConnection(StreamSocketListener sender,
            StreamSocketListenerConnectionReceivedEventArgs args)
        {
            // Moved to the pool rather than served inline. The handler is raised on a thread the
            // socket implementation picks, and serving blocks on reads below - so doing it inline
            // would tie up whatever thread raised it for as long as the client feels like taking.
            StreamSocket socket = args.Socket;
            Task.Run(delegate { Serve(socket); });
        }

        /// <summary>
        /// Reads one request, writes one response, closes. Connection: close on purpose - a
        /// keep-alive session would mean tracking idle sockets across app suspensions for a
        /// client that, in practice, is a command run by hand.
        /// </summary>
        private static void Serve(StreamSocket socket)
        {
            int started = Environment.TickCount;
            int status = 200;
            string target = "?";
            string store = "n/a";
            string body = "";
            int bytes = 0;
            try
            {
                DataReader reader = new DataReader(socket.InputStream);
                // Partial is required, not an optimisation. With the default options LoadAsync
                // waits for the *whole* count it was given, and an HTTP client that has sent its
                // request line now waits for our reply - so LoadAsync(512) against a 60-byte
                // request blocks until that client gives up. Partial returns the moment any bytes
                // land, which is what makes a single-read request line work at all.
                reader.InputStreamOptions = InputStreamOptions.Partial;
                reader.UnicodeEncoding = UnicodeEncoding.Utf8;

                // Fully qualified rather than `using System.Text`: that namespace also holds a
                // UnicodeEncoding, which would collide with the Windows.Storage.Streams one this
                // file needs for the DataReader - and the collision is a compile error, not a
                // preference. (StringBuilder itself is missing on this profile until named.)
                System.Text.StringBuilder head = new System.Text.StringBuilder(256);
                for (int i = 0; i < 8 && head.ToString().IndexOf('\n') < 0; i++)
                {
                    Task<uint> load = reader.LoadAsync(512).AsTask();
                    if (!load.Wait(3000))
                    {
                        // Bounded: something that connects and says nothing must not pin a pool
                        // thread for the life of the process. The eventual fault is observed on
                        // the pool so it does not surface later as an unobserved-exception line
                        // from an unrelated moment.
                        load.ContinueWith(delegate(Task<uint> t)
                        {
                            if (t.IsFaulted)
                            {
                                AppLog.Write("LogServer: abandoned read", t.Exception);
                            }
                        }, TaskScheduler.Default);
                        status = 408;
                        target = "(silent)";
                        body = "request timed out\r\n";
                        return;
                    }
                    uint n = load.Result;
                    if (n == 0)
                    {
                        break;
                    }
                    head.Append(reader.ReadString(n));
                }

                // "GET /log?tail=300 HTTP/1.1" -> "/log?tail=300". First and last space, not a
                // split, so a target that somehow contains a space still yields the middle piece.
                string line = head.ToString();
                int nl = line.IndexOf('\n');
                if (nl >= 0)
                {
                    line = line.Substring(0, nl);
                }
                line = line.Trim();
                int firstSp = line.IndexOf(' ');
                int lastSp = line.LastIndexOf(' ');
                target = (firstSp >= 0 && lastSp > firstSp)
                    ? line.Substring(firstSp + 1, lastSp - firstSp - 1)
                    : "/";

                string extra;
                int status2;
                Dispatch(target, out status2, out body, out extra, out store);
                status = status2;

                byte[] payload = System.Text.Encoding.UTF8.GetBytes(body);
                string reason = status == 404 ? "Not Found"
                    : (status == 408 ? "Request Timeout" : "OK");
                string headText =
                    "HTTP/1.1 " + status + " " + reason + "\r\n"
                    + "Content-Type: text/plain; charset=utf-8\r\n"
                    + "Content-Length: " + payload.Length + "\r\n"
                    + "Connection: close\r\n"
                    + "Cache-Control: no-store\r\n"
                    + extra
                    + "\r\n";

                DataWriter writer = new DataWriter(socket.OutputStream);
                // UTF-8, not ASCII: Encoding.ASCII does not exist on the Windows Phone profile
                // (the same gap that bit PrprPort's diagnostics). Byte-identical here all the
                // same, because a status line and these header names are pure ASCII - UTF-8 only
                // diverges above U+007F, and nothing above it can legally appear in a header.
                writer.WriteBytes(System.Text.Encoding.UTF8.GetBytes(headText));
                writer.WriteBytes(payload);
                if (writer.StoreAsync().AsTask().Wait(5000))
                {
                    // Only counted as delivered if it actually went out; a 0B line in the log is
                    // then a real signal rather than an optimistic one.
                    bytes = payload.Length;
                }
                writer.Dispose();
            }
            catch (Exception ex)
            {
                AppLog.Write("LogServer: serve " + target, ex);
            }
            finally
            {
                try
                {
                    socket.Dispose();
                }
                catch (Exception)
                {
                }
                int seen = System.Threading.Interlocked.Increment(ref _requests);
                AppLog.Write("LogServer: " + target + " -> " + status + " " + bytes
                    + "B store=" + store + " " + (Environment.TickCount - started) + "ms #" + seen);
            }
        }

        private static void Dispatch(string target, out int status, out string body,
            out string extra, out string store)
        {
            status = 200;
            body = "";
            extra = "";
            store = "n/a";

            string path = target;
            string query = "";
            int q = target.IndexOf('?');
            if (q >= 0)
            {
                path = target.Substring(0, q);
                query = target.Substring(q + 1);
            }
            if (path.Length == 0 || path == "/")
            {
                // Help, not the file. Opening this in a browser is the first thing anyone does,
                // and a 4 MB page of wrapped log lines is a worse first answer than the two
                // commands that were wanted anyway.
                path = "/help";
            }

            if (path == "/log")
            {
                int from = -1;
                int tail = -1;
                string[] parts = query.Split('&');
                for (int i = 0; i < parts.Length; i++)
                {
                    int eq = parts[i].IndexOf('=');
                    if (eq <= 0)
                    {
                        continue;
                    }
                    string key = parts[i].Substring(0, eq);
                    int value;
                    if (!int.TryParse(parts[i].Substring(eq + 1), out value))
                    {
                        continue;
                    }
                    if (key == "from")
                    {
                        from = value;
                    }
                    else if (key == "tail")
                    {
                        tail = value;
                    }
                }

                string which;
                string all = ReadLog(out which);
                store = which;
                int total = all.Length;
                extra = "X-Log-Length: " + total + "\r\n";

                if (from > total)
                {
                    // The caller's offset is older than the file now is: the log rotated and what
                    // it last read is gone. An empty body here would read as "nothing new, ever",
                    // which is the one answer a tail loop must never get - it would sit there
                    // polling a log it had silently fallen behind. Hand back everything instead
                    // and flag it so the client knows to reset its offset.
                    body = all;
                    extra += "X-Log-Rotated: 1\r\n";
                }
                else if (from >= 0)
                {
                    body = all.Substring(from);
                }
                else if (tail > 0)
                {
                    body = TailLines(all, tail);
                }
                else
                {
                    body = all;
                }
                extra += "X-Log-Source: " + which + "\r\n";
                return;
            }

            if (path == "/health")
            {
                string which;
                string all = ReadLog(out which);
                store = which;
                string url = LocalUrl();
                body = "LogServer up port=" + Port
                    + " state=" + _state
                    + " chars=" + all.Length
                    + " store=" + which
                    + " requests=" + _requests
                    + " url=" + (url == null ? "(no IPv4)" : url + "/log")
                    + "\r\n";
                return;
            }

            if (path == "/help")
            {
                body = "Phirura log server\r\n\r\n"
                    + "  GET /log             the whole phirura.log\r\n"
                    + "  GET /log?tail=300    the last 300 lines\r\n"
                    + "  GET /log?from=N      everything written after character N\r\n"
                    + "                       N is the X-Log-Length of the previous reply\r\n"
                    + "  GET /health          one line of status\r\n\r\n"
                    + "curl http://<phone-ip>:" + Port + "/log\r\n"
                    + "curl \"http://<phone-ip>:" + Port + "/log?tail=300\"\r\n";
                return;
            }

            status = 404;
            body = "no such path; try /log, /log?tail=300, /health\r\n";
        }

        /// <summary>
        /// The last <paramref name="count"/> lines, from the end of the file backwards.
        ///
        /// Walks newline positions rather than splitting, because the whole point of asking for a
        /// tail is that the file is too big to want whole - and a split would allocate one string
        /// per line of it to then throw almost all of them away.
        /// </summary>
        private static string TailLines(string text, int count)
        {
            if (count <= 0 || text.Length == 0)
            {
                return text;
            }
            // Counted forwards, never walked backwards. Walking back is the obvious
            // implementation and it is wrong in exactly the way that shows up as an empty reply:
            // AppLog terminates every appended line with \r\n, so the file's last character *is* a
            // newline, "position of that break + 1" is text.Length, and the next backward search
            // starts at Length-1 and finds that very same break again. The walk never moves, and
            // Substring(text.Length) is "". A request for the last four lines then came back with
            // Content-Length: 0 alongside a correct X-Log-Length - which reads as "nothing new
            // since you last looked", i.e. the one answer a tail loop must never be given.
            int lines = 0;
            for (int i = 0; i < text.Length; i++)
            {
                if (text[i] == '\n')
                {
                    lines++;
                }
            }
            // A file that does not end in a newline still has a last line; the loop above counts
            // only terminators. This file always ends in one, so the two agree here - but when
            // they do not, being off by one means either a dropped line or a repeated one.
            if (text[text.Length - 1] != '\n')
            {
                lines++;
            }
            if (lines <= count)
            {
                // Fewer lines exist than were asked for, so there is no tail to take: hand back
                // all of it rather than a prefix that starts mid-sentence.
                return text;
            }

            int skip = lines - count;
            int seen = 0;
            for (int i = 0; i < text.Length; i++)
            {
                if (text[i] != '\n')
                {
                    continue;
                }
                seen++;
                if (seen == skip)
                {
                    return text.Substring(i + 1);
                }
            }
            // Only reachable if the two counts above ever disagree. Everything is the safe
            // answer: a long reply beats an empty one.
            return text;
        }

        /// <summary>
        /// Reads the log, preferring LocalFolder and falling back to the identical copy in
        /// TemporaryFolder.
        ///
        /// Both are appended by <see cref="AppLog"/>, and the reason the second one exists at all
        /// is this exact case: ISETool holds the Local copy open for the duration of a pull. So
        /// one store can be unreadable while the other is fine, and falling back keeps the channel
        /// alive *while* you are pulling it - which is the situation a live log server is for.
        /// </summary>
        private static string ReadLog(out string store)
        {
            string text = TryRead(ApplicationData.Current.LocalFolder);
            if (text != null)
            {
                store = "Local";
                return text;
            }
            text = TryRead(ApplicationData.Current.TemporaryFolder);
            if (text != null)
            {
                store = "Temp";
                return text;
            }
            // Nothing yet, which is a legitimate state on a fresh launch rather than a failure.
            store = "none";
            return "";
        }

        private static string TryRead(StorageFolder folder)
        {
            try
            {
                // GetFileAsync rather than TryGetItemAsync: the latter has zero occurrences in
                // the platform winmd, so it does not exist on this profile at all. The
                // "not written yet" case therefore arrives as an exception and is answered by the
                // catch below - which is also the locked-file case, so one handler covers both.
                StorageFile file = folder.GetFileAsync(AppLog.FileName).AsTask().Result;
                return FileIO.ReadTextAsync(file).AsTask().Result;
            }
            catch (Exception)
            {
                // A locked file is the expected case, not an error worth a line: the append thread
                // and an ISETool pull both hold it, and logging every collision would put a line
                // in the file we are trying to read.
                return null;
            }
        }

        /// <summary>
        /// The address a PC would actually use, or null when the phone is not on a network.
        ///
        /// Computed per request rather than at bind time: Wi-Fi comes and goes on a phone, and the
        /// address logged once at startup would be the wrong one half the time by the time anyone
        /// read it. Loopback is useless from another machine and APIPA means there was no DHCP
        /// lease, i.e. there is no network to reach this through - both filtered rather than
        /// offered as a URL that cannot work.
        /// </summary>
        private static string LocalUrl()
        {
            try
            {
                // Connectivity's NetworkInformation, not Windows.Networking's. The platform doc
                // index lists GetHostNames under exactly one type - M:Windows.Networking
                // .Connectivity.NetworkInformation.GetHostNames - and Windows.Networking resolves
                // HostName and HostNameType but not this class, which is the CS0103 the build
                // reported. Spelled out rather than a new `using` so the source records which of
                // the two classes it means instead of leaving the reader to work it out.
                foreach (HostName host in Windows.Networking.Connectivity.NetworkInformation.GetHostNames())
                {
                    if (host.Type != HostNameType.Ipv4)
                    {
                        continue;
                    }
                    string name = host.CanonicalName;
                    if (name == null || name.StartsWith("127.") || name.StartsWith("169.254."))
                    {
                        continue;
                    }
                    return "http://" + name + ":" + Port;
                }
            }
            catch (Exception)
            {
            }
            return null;
        }
    }
}
