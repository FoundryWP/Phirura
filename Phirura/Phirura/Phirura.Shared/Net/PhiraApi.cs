using System;
using System.Collections.Generic;
using System.Globalization;
using System.Net.Http;
using System.Text;
using System.Threading.Tasks;
using Phirura.Parse;
using Windows.Data.Json;
using Windows.Storage;

namespace Phirura.Net
{
    /// <summary>Signed-in Phira account.</summary>
    public sealed class PhiraUser
    {
        public int Id;
        public string Name = string.Empty;
        public string AvatarUrl;
        public double Rks;
        public int Roles;
    }

    /// <summary>
    /// One entry of the online chart library (`GET /chart`).
    ///
    /// Bound members must be properties — WinRT XAML binding ignores public fields.
    /// </summary>
    public sealed class PhiraChart
    {
        public int Id { get; set; }

        /// <summary>
        /// Chart title.
        ///
        /// Deliberately NOT named `Name`: WinRT's binding engine cannot resolve a member
        /// called `Name` on a plain .NET data object, so `{Binding Name}` fails with
        /// "path error: 'Name' property not found" even though the property is present in
        /// the assembly and every sibling property on the same type binds fine.
        /// `ChartEntry` sidesteps this by calling its title `Title`.
        /// </summary>
        public string ChartName { get; set; }

        public string Level { get; set; }
        public double Difficulty { get; set; }
        public string Charter { get; set; }
        public string Composer { get; set; }
        public string Illustrator { get; set; }
        public string Description { get; set; }
        public bool Ranked { get; set; }
        public bool Reviewed { get; set; }
        public bool Stable { get; set; }

        /// <summary>URL of the chart archive (a zip holding info.yml / chart.json / audio).</summary>
        public string FileUrl { get; set; }

        public string IllustrationUrl { get; set; }

        /// <summary>Community rating, or NaN when the chart is unranked.</summary>
        public double Rating { get; set; }

        public PhiraChart()
        {
            ChartName = "Untitled";
            Level = string.Empty;
            Charter = string.Empty;
            Composer = string.Empty;
            Illustrator = string.Empty;
            Description = string.Empty;
            Rating = double.NaN;
        }

        public string LevelDisplay
        {
            get
            {
                if (!string.IsNullOrEmpty(Level))
                {
                    return Level;
                }
                return "Lv." + Difficulty.ToString("0.#", CultureInfo.InvariantCulture);
            }
        }

        public string Subtitle
        {
            get
            {
                string s = Composer;
                if (!string.IsNullOrEmpty(Charter))
                {
                    s += (s.Length > 0 ? "  /  " : "") + "谱面 " + Charter;
                }
                if (!string.IsNullOrEmpty(Description))
                {
                    s += (s.Length > 0 ? "\n" : "") + Description;
                }
                return s;
            }
        }

        public string Badge
        {
            get
            {
                if (!double.IsNaN(Rating) && Rating > 0.0)
                {
                    return Rating.ToString("0.0", CultureInfo.InvariantCulture);
                }
                if (Stable)
                {
                    return "STABLE";
                }
                if (Ranked)
                {
                    return "RANKED";
                }
                return Reviewed ? "已过审" : "未过审";
            }
        }
    }

    /// <summary>An error the server reported in the JSON body ({"error": ..., "code": ...}).</summary>
    public sealed class PhiraApiException : Exception
    {
        public string Code;

        public PhiraApiException(string message, string code)
            : base(message)
        {
            Code = code;
        }
    }

    /// <summary>Persisted login state. The token is the only thing that matters across launches.</summary>
    public static class PhiraSession
    {
        private const string KeyToken = "phira.token";
        private const string KeyName = "phira.name";
        private const string KeyUid = "phira.uid";

        public static string Token;
        public static PhiraUser User;

        public static bool IsLoggedIn
        {
            get { return !string.IsNullOrEmpty(Token); }
        }

        public static string DisplayName
        {
            get
            {
                if (User != null && !string.IsNullOrEmpty(User.Name))
                {
                    return User.Name;
                }
                return IsLoggedIn ? "已登录" : "未登录";
            }
        }

        public static void Load()
        {
            try
            {
                ApplicationDataContainer c = ApplicationData.Current.LocalSettings;
                if (c.Values.ContainsKey(KeyToken))
                {
                    Token = c.Values[KeyToken] as string;
                }
                if (c.Values.ContainsKey(KeyName) && !string.IsNullOrEmpty(Token))
                {
                    PhiraUser u = new PhiraUser();
                    u.Id = c.Values.ContainsKey(KeyUid) ? (int)c.Values[KeyUid] : 0;
                    u.Name = c.Values[KeyName] as string;
                    User = u;
                }
            }
            catch (Exception)
            {
            }
        }

        public static void Save()
        {
            try
            {
                ApplicationDataContainer c = ApplicationData.Current.LocalSettings;
                if (string.IsNullOrEmpty(Token))
                {
                    c.Values.Remove(KeyToken);
                    c.Values.Remove(KeyName);
                    c.Values.Remove(KeyUid);
                    return;
                }
                c.Values[KeyToken] = Token;
                if (User != null)
                {
                    c.Values[KeyName] = User.Name;
                    c.Values[KeyUid] = User.Id;
                }
            }
            catch (Exception)
            {
            }
        }

        public static void Clear()
        {
            Token = null;
            User = null;
            Save();
        }
    }

    /// <summary>
    /// Thin client for the Phira backend (https://phira.5wyxi.com), mirroring the
    /// endpoints the Rust client uses:
    ///
    ///     POST /login   { email, password, clientVersion } -> { id, token }
    ///     GET  /me                                          -> User
    ///     GET  /chart?page=N&amp;pageNum=M&amp;search=&amp;order= -> { count, results: [Chart] }
    ///     GET  &lt;chart.file.url&gt;                            -> the chart archive (zip)
    ///
    /// Everything is parsed defensively: the server is not versioned and field
    /// presence varies between deployments.
    /// </summary>
    public static class PhiraApi
    {
        public const string BaseUrl = "https://phira.5wyxi.com";

        /// <summary>Sent as `clientVersion`; the server rejects some old values.</summary>
        public const string ClientVersion = "0.6.6";

        private static HttpClient _http;

        private static HttpClient Http
        {
            get
            {
                if (_http == null)
                {
                    HttpClient c = new HttpClient();
                    c.Timeout = TimeSpan.FromSeconds(45);
                    // TryAddWithoutValidation is used for both: the typed ParseAdd helpers
                    // are not available on every Windows Phone 8.1 profile.
                    c.DefaultRequestHeaders.TryAddWithoutValidation("User-Agent", "phira/" + ClientVersion);
                    c.DefaultRequestHeaders.TryAddWithoutValidation("Accept-Language", "zh-CN");
                    c.DefaultRequestHeaders.TryAddWithoutValidation("Accept", "application/json");
                    _http = c;
                }
                return _http;
            }
        }

        // ------------------------------------------------------------------
        // Auth
        // ------------------------------------------------------------------

        /// <summary>Signs in and returns the bearer token. Throws PhiraApiException on refusal.</summary>
        public static async Task<string> LoginAsync(string email, string password)
        {
            JsonObject body = new JsonObject();
            body.SetNamedValue("email", JsonValue.CreateStringValue(email));
            body.SetNamedValue("password", JsonValue.CreateStringValue(password));
            body.SetNamedValue("clientVersion", JsonValue.CreateStringValue(ClientVersion));

            string text = await PostJsonAsync("/login", body, null);
            JsonObject o = ParseObject(text);
            string token = JsonExt.Str(o, "token", null);
            if (string.IsNullOrEmpty(token))
            {
                throw new PhiraApiException("服务器没有返回登录令牌", null);
            }
            return token;
        }

        public static async Task<PhiraUser> GetMeAsync(string token)
        {
            string text = await GetStringAsync("/me", token);
            JsonObject o = ParseObject(text);
            PhiraUser u = new PhiraUser();
            u.Id = JsonExt.Int(o, "id", 0);
            u.Name = JsonExt.Str(o, "name", string.Empty);
            u.Rks = JsonExt.Num(o, "rks", 0.0);
            u.Roles = JsonExt.Int(o, "roles", 0);
            u.AvatarUrl = FileUrlOf(o, "avatar");
            return u;
        }

        // ------------------------------------------------------------------
        // Chart library
        // ------------------------------------------------------------------

        /// <summary>
        /// One page of the chart library. <paramref name="order"/> accepts the values
        /// the official client uses: updated / name / difficulty / rating (prefix with
        /// '-' to reverse).
        /// </summary>
        public static async Task<List<PhiraChart>> SearchChartsAsync(string token, string search, string order, int page, int pageSize)
        {
            StringBuilder q = new StringBuilder("/chart?");
            q.Append("page=").Append(page + 1);
            q.Append("&pageNum=").Append(pageSize);
            if (!string.IsNullOrEmpty(order))
            {
                q.Append("&order=").Append(Uri.EscapeUriString(order));
            }
            if (!string.IsNullOrEmpty(search))
            {
                q.Append("&search=").Append(Uri.EscapeUriString(search));
            }

            string text = await GetStringAsync(q.ToString(), token);
            JsonObject root = ParseObject(text);
            List<PhiraChart> list = new List<PhiraChart>();
            JsonArray results = JsonExt.Arr(root, "results");
            if (results == null)
            {
                return list;
            }
            for (int i = 0; i < results.Count; i++)
            {
                JsonObject o = JsonExt.ObjAt(results, i);
                if (o == null)
                {
                    continue;
                }
                PhiraChart c = new PhiraChart();
                c.Id = JsonExt.Int(o, "id", 0);
                c.ChartName = JsonExt.Str(o, "name", "Untitled");
                c.Level = JsonExt.Str(o, "level", string.Empty);
                c.Difficulty = JsonExt.Num(o, "difficulty", 0.0);
                c.Charter = JsonExt.Str(o, "charter", string.Empty);
                c.Composer = JsonExt.Str(o, "composer", string.Empty);
                c.Illustrator = JsonExt.Str(o, "illustrator", string.Empty);
                c.Description = JsonExt.Str(o, "description", string.Empty);
                c.Ranked = JsonExt.Bool(o, "ranked", false);
                c.Reviewed = JsonExt.Bool(o, "reviewed", false);
                c.Stable = JsonExt.Bool(o, "stable", false);

                IJsonValue rating = JsonExt.Get(o, "rating");
                if (rating != null && rating.ValueType == JsonValueType.Number)
                {
                    c.Rating = rating.GetNumber();
                }

                c.FileUrl = FileUrlOf(o, "file");
                c.IllustrationUrl = FileUrlOf(o, "illustration");
                list.Add(c);
            }
            return list;
        }

        // ------------------------------------------------------------------
        // Transfers
        // ------------------------------------------------------------------

        /// <summary>Downloads a file (chart archive, illustration, ...) with progress in [0, 1].</summary>
        public static async Task<byte[]> DownloadAsync(string token, string url, Action<double> progress)
        {
            if (string.IsNullOrEmpty(url))
            {
                throw new PhiraApiException("这个谱面没有提供下载地址", null);
            }
            HttpRequestMessage req = new HttpRequestMessage(HttpMethod.Get, new Uri(url));
            if (!string.IsNullOrEmpty(token))
            {
                req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + token);
            }

            HttpResponseMessage resp = await Http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead);
            if (!resp.IsSuccessStatusCode)
            {
                throw new PhiraApiException(
                    "下载失败（HTTP " + (int)resp.StatusCode + "）", null);
            }

            long total = resp.Content.Headers.ContentLength.HasValue
                ? resp.Content.Headers.ContentLength.Value : -1L;

            System.IO.MemoryStream ms = new System.IO.MemoryStream();
            using (System.IO.Stream s = await resp.Content.ReadAsStreamAsync())
            {
                byte[] buf = new byte[16384];
                long read = 0;
                while (true)
                {
                    int n = await s.ReadAsync(buf, 0, buf.Length);
                    if (n <= 0)
                    {
                        break;
                    }
                    ms.Write(buf, 0, n);
                    read += n;
                    if (progress != null && total > 0)
                    {
                        progress(Math.Min(1.0, (double)read / total));
                    }
                }
            }
            if (progress != null)
            {
                progress(1.0);
            }
            return ms.ToArray();
        }

        // ------------------------------------------------------------------
        // Plumbing
        // ------------------------------------------------------------------

        private static async Task<string> GetStringAsync(string pathAndQuery, string token)
        {
            HttpRequestMessage req = new HttpRequestMessage(HttpMethod.Get, new Uri(BaseUrl + pathAndQuery));
            if (!string.IsNullOrEmpty(token))
            {
                req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + token);
            }
            HttpResponseMessage resp = await Http.SendAsync(req);
            return await ReadCheckedAsync(resp);
        }

        private static async Task<string> PostJsonAsync(string path, JsonObject body, string token)
        {
            HttpRequestMessage req = new HttpRequestMessage(HttpMethod.Post, new Uri(BaseUrl + path));
            if (!string.IsNullOrEmpty(token))
            {
                req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + token);
            }
            req.Content = new StringContent(body.Stringify(), Encoding.UTF8, "application/json");
            HttpResponseMessage resp = await Http.SendAsync(req);
            return await ReadCheckedAsync(resp);
        }

        /// <summary>
        /// Reads a response, converting a non-2xx status into a PhiraApiException that
        /// carries the server's own message and code.
        /// </summary>
        private static async Task<string> ReadCheckedAsync(HttpResponseMessage resp)
        {
            string text = string.Empty;
            try
            {
                text = await resp.Content.ReadAsStringAsync();
            }
            catch (Exception)
            {
            }

            if (resp.IsSuccessStatusCode)
            {
                return text;
            }

            string message = null;
            string code = null;
            try
            {
                JsonObject o = JsonObject.Parse(text);
                message = JsonExt.Str(o, "error", null);
                code = JsonExt.Str(o, "code", null);
            }
            catch (Exception)
            {
            }

            if (string.IsNullOrEmpty(message))
            {
                message = "请求失败（HTTP " + (int)resp.StatusCode + "）";
            }
            if ((int)resp.StatusCode == 401 || (int)resp.StatusCode == 403)
            {
                code = string.IsNullOrEmpty(code) ? "UNAUTHORIZED" : code;
            }
            throw new PhiraApiException(message, code);
        }

        private static JsonObject ParseObject(string text)
        {
            try
            {
                return JsonObject.Parse(text);
            }
            catch (Exception ex)
            {
                throw new PhiraApiException("服务器返回了无法解析的内容：" + ex.Message, null);
            }
        }

        /// <summary>
        /// Resolves one of the server's `File` fields (`file`, `illustration`, `preview`,
        /// `avatar`).
        ///
        /// The Rust model declares these as
        /// <c>#[serde(transparent)] struct File { pub url: String }</c>, and `transparent`
        /// means the struct serialises as the inner value — so the wire format is a
        /// **bare URL string**, e.g.
        /// <c>"file": "https://phira.5wyxi.com/files/baba3b80-…"</c>.
        ///
        /// Reading it as <c>{"url": …}</c> (which this used to do) yields null for every
        /// chart, and the download button then reports "这个谱面没有提供下载地址" even though
        /// the URL is right there. The object form is still accepted because the server is
        /// unversioned and has been seen to vary between deployments.
        /// </summary>
        private static string FileUrlOf(JsonObject o, string key)
        {
            IJsonValue v = JsonExt.Get(o, key);
            if (v == null)
            {
                return null;
            }
            if (v.ValueType == JsonValueType.String)
            {
                string s = v.GetString();
                return string.IsNullOrEmpty(s) ? null : s;
            }
            if (v.ValueType == JsonValueType.Object)
            {
                return JsonExt.Str(v.GetObject(), "url", null);
            }
            return null;
        }
    }
}
