using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Compression;
using System.Threading.Tasks;
using Phirura.Game;
using Phirura.Parse;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Streams;
using Windows.UI.Xaml.Media.Imaging;

namespace Phirura
{
    /// <summary>
    /// Loads the charts sitting in `LocalFolder\charts`: the ones the user imported, plus the
    /// one `SeedRootAsync` copies out of the package on first launch. There is no bundled demo
    /// card any more - see the note at the top of `LoadAllAsync`.
    ///
    /// Imported charts are *copied* into `ApplicationData\LocalFolder\charts\&lt;id&gt;`
    /// rather than referenced in place, because a picked StorageFolder is not guaranteed
    /// to stay reachable across launches (SD cards, OneDrive placeholders, ...).
    /// </summary>
    public static class ChartLibrary
    {
        private const string LocalChartsFolder = "charts";

        /// <summary>Uri of the local chart root; a chart's directory name follows it.</summary>
        private const string LocalBaseUri = "ms-appdata:///local/charts/";

        public static async Task<List<ChartEntry>> LoadAllAsync()
        {
            // `MakeDemoEntry()` used to run unconditionally here and made this the first card in
            // the list. It pointed at `ms-appx:///Assets/demo`, which is no longer packaged, so
            // the entry could never be opened - a card that is always present and always fails
            // reads to the player as "the app is broken", not as "this one is missing". The
            // bundled chart needs no special case any more: `EnsureBundledChartAsync` puts
            // `Assets\charts\190` into `data/charts/custom` and phira scans that itself, while
            // this list picks up the seeded copy under `LocalFolder\charts` by enumeration.
            List<ChartEntry> list = new List<ChartEntry>();

            StorageFolder root;
            try
            {
                root = await ApplicationData.Current.LocalFolder.CreateFolderAsync(
                    LocalChartsFolder, CreationCollisionOption.OpenIfExists);
            }
            catch (Exception)
            {
                return list;
            }

            IReadOnlyList<StorageFolder> folders = await root.GetFoldersAsync();
            for (int i = 0; i < folders.Count; i++)
            {
                ChartEntry e = await TryLoadEntryAsync("ms-appdata:///local/" + LocalChartsFolder + "/" + folders[i].Name, true, "local:" + folders[i].Name);
                if (e != null)
                {
                    list.Add(e);
                }
            }
            return list;
        }

        private static async Task<ChartEntry> TryLoadEntryAsync(string baseUri, bool isLocal, string id)
        {
            try
            {
                List<string> names = await ListFilesAsync(baseUri);
                if (names.Count == 0)
                {
                    return null;
                }
                string chartFile = ChartParser.PickChartFile(names);
                if (chartFile == null)
                {
                    return null;
                }
                string audioFile = ChartParser.PickAudioFile(names);
                string illustration = ChartParser.PickIllustration(names);
                string infoFile = ChartParser.PickInfoFile(names);

                ChartInfo info = null;
                if (infoFile != null)
                {
                    string text = await ReadTextAsync(baseUri, infoFile);
                    info = ChartParser.ParseInfoYaml(text);
                }
                return ChartEntry.FromInfo(id, baseUri, isLocal, chartFile, audioFile, illustration, info);
            }
            catch (Exception)
            {
                return null;
            }
        }

        private static async Task<List<string>> ListFilesAsync(string baseUri)
        {
            List<string> names = new List<string>();
            StorageFolder f = await StorageFolder.GetFolderFromPathAsync(LocalPathOf(baseUri));
            IReadOnlyList<StorageFile> fs = await f.GetFilesAsync();
            for (int i = 0; i < fs.Count; i++)
            {
                names.Add(fs[i].Name);
            }
            return names;
        }

        private static string LocalPathOf(string baseUri)
        {
            // ms-appdata:///local/charts/<name> -> <LocalFolder>\charts\<name>
            string tail = baseUri.Substring("ms-appdata:///local/".Length);
            return ApplicationData.Current.LocalFolder.Path + "\\" + tail.Replace('/', '\\');
        }

        public static async Task<string> ReadTextAsync(string baseUri, string file)
        {
            StorageFile f = await GetFileAsync(baseUri, file);
            return await FileIO.ReadTextAsync(f);
        }

        public static async Task<StorageFile> GetFileAsync(string baseUri, string file)
        {
            Uri uri = new Uri(baseUri + "/" + file);
            return await StorageFile.GetFileFromApplicationUriAsync(uri);
        }

        public static Uri UriOf(string baseUri, string file)
        {
            return new Uri(baseUri + "/" + Uri.EscapeUriString(file));
        }

        /// <summary>Loads an image and its natural pixel size (needed to scale judge-line textures).</summary>
        public static async Task<ChartTexture> LoadTextureAsync(string baseUri, string file)
        {
            StorageFile f = await GetFileAsync(baseUri, file);
            using (IRandomAccessStream stream = await f.OpenReadAsync())
            {
                BitmapDecoder decoder = await BitmapDecoder.CreateAsync(stream);
                ChartTexture tex = new ChartTexture();
                tex.Path = file;
                tex.Width = decoder.PixelWidth;
                tex.Height = decoder.PixelHeight;
                BitmapImage bmp = new BitmapImage();
                stream.Seek(0);
                await bmp.SetSourceAsync(stream);
                tex.Source = bmp;
                return tex;
            }
        }

        public static async Task<ChartTexture> TryLoadTextureAsync(string baseUri, string file)
        {
            try
            {
                return await LoadTextureAsync(baseUri, file);
            }
            catch (Exception)
            {
                return null;
            }
        }

        /// <summary>
        /// Win2D variant of <see cref="LoadTextureAsync"/>: same ChartTexture, but the
        /// image lands in <c>Tag</c> as a CanvasBitmap instead of a BitmapImage.
        ///
        /// Loaded at 96 DPI so Bounds and SizeInPixels agree, which lets the renderer treat
        /// Width/Height as DIPs and pixels interchangeably.
        /// </summary>
        public static async Task<ChartTexture> LoadCanvasTextureAsync(
            Microsoft.Graphics.Canvas.ICanvasResourceCreator resourceCreator, string baseUri, string file)
        {
            StorageFile f = await GetFileAsync(baseUri, file);
            using (IRandomAccessStream stream = await f.OpenReadAsync())
            {
                Microsoft.Graphics.Canvas.CanvasBitmap bmp =
                    await Microsoft.Graphics.Canvas.CanvasBitmap.LoadAsync(resourceCreator, stream, 96.0f);
                ChartTexture tex = new ChartTexture();
                tex.Path = file;
                tex.Width = bmp.SizeInPixels.Width;
                tex.Height = bmp.SizeInPixels.Height;
                tex.Tag = bmp;
                return tex;
            }
        }

        public static async Task<ChartTexture> TryLoadCanvasTextureAsync(
            Microsoft.Graphics.Canvas.ICanvasResourceCreator resourceCreator, string baseUri, string file)
        {
            try
            {
                return await LoadCanvasTextureAsync(resourceCreator, baseUri, file);
            }
            catch (Exception)
            {
                return null;
            }
        }

        // ------------------------------------------------------------------
        // Import
        // ------------------------------------------------------------------

        /// <summary>
        /// Copies the files the user picked into the app's local chart store and returns the
        /// new entry.
        ///
        /// WP8.1 cannot hand a picker result's *containing folder* to the app (StorageFile
        /// has no GetParentAsync, and documentsLibrary would require a file-type-association
        /// declaration the 8.1 manifest schema rejects), so the import flow asks the user to
        /// multi-select every file that belongs to the chart instead. That needs no
        /// capability at all and still gives us the audio, the illustration and the chart.
        /// </summary>
        public static async Task<ChartEntry> ImportAsync(IReadOnlyList<StorageFile> files)
        {
            if (files == null || files.Count == 0)
            {
                throw new InvalidOperationException("没有选择任何文件");
            }

            StorageFile chart = null;
            for (int i = 0; i < files.Count; i++)
            {
                string lower = files[i].Name.ToLowerInvariant();
                if (lower.EndsWith(".json") || lower.EndsWith(".pec"))
                {
                    chart = files[i];
                    break;
                }
            }
            if (chart == null)
            {
                throw new InvalidOperationException("选中的文件里没有 chart.json 或 chart.pec");
            }

            string id = Sanitize(System.IO.Path.GetFileNameWithoutExtension(chart.Name));
            StorageFolder root = await ApplicationData.Current.LocalFolder.CreateFolderAsync(
                LocalChartsFolder, CreationCollisionOption.OpenIfExists);
            StorageFolder target = await root.CreateFolderAsync(id, CreationCollisionOption.OpenIfExists);

            for (int i = 0; i < files.Count; i++)
            {
                StorageFile src = files[i];
                if (!IsChartFile(src.Name))
                {
                    continue;
                }
                await src.CopyAsync(target, src.Name, NameCollisionOption.ReplaceExisting);
            }

            ChartEntry entry = await TryLoadEntryAsync("ms-appdata:///local/" + LocalChartsFolder + "/" + id, true, "local:" + id);
            if (entry == null)
            {
                throw new InvalidOperationException("导入失败：目标文件夹里没有找到可识别的谱面（chart.json / chart.pec）");
            }
            return entry;
        }

        /// <summary>Files worth copying out of a chart folder.</summary>
        private static bool IsChartFile(string name)
        {
            string lower = name.ToLowerInvariant();
            return lower == "info.yml" || lower == "info.yaml" ||
                   lower.EndsWith(".json") || lower.EndsWith(".pec") ||
                   lower.EndsWith(".wav") || lower.EndsWith(".mp3") ||
                   lower.EndsWith(".m4a") || lower.EndsWith(".aac") ||
                   lower.EndsWith(".ogg") || lower.EndsWith(".opus") ||
                   lower.EndsWith(".png") || lower.EndsWith(".jpg") || lower.EndsWith(".jpeg") ||
                   lower.EndsWith(".txt");
        }

        // ------------------------------------------------------------------
        // Online install
        // ------------------------------------------------------------------

        /// <summary>
        /// Unpacks a downloaded chart archive (the zip the Phira backend serves at
        /// `chart.file.url`) into the local chart store and returns the new entry.
        /// </summary>
        public static async Task<ChartEntry> InstallArchiveAsync(byte[] archive, int chartId)
        {
            if (archive == null || archive.Length == 0)
            {
                throw new InvalidOperationException("下载到的文件是空的");
            }

            string id = "online-" + chartId;
            StorageFolder root = await ApplicationData.Current.LocalFolder.CreateFolderAsync(
                LocalChartsFolder, CreationCollisionOption.OpenIfExists);

            // Replace any earlier copy of the same chart.
            try
            {
                StorageFolder stale = await root.GetFolderAsync(id);
                await stale.DeleteAsync();
            }
            catch (Exception)
            {
            }
            StorageFolder target = await root.CreateFolderAsync(id, CreationCollisionOption.OpenIfExists);

            using (MemoryStream ms = new MemoryStream(archive))
            using (ZipArchive zip = new ZipArchive(ms, ZipArchiveMode.Read))
            {
                for (int i = 0; i < zip.Entries.Count; i++)
                {
                    ZipArchiveEntry ze = zip.Entries[i];
                    if (ze.Length == 0 && string.IsNullOrEmpty(ze.Name))
                    {
                        continue;   // directory marker
                    }
                    if (!IsChartFile(ze.Name))
                    {
                        continue;   // e.g. readme.md, __MACOSX junk
                    }
                    StorageFile f = await EnsureFileAsync(target, ze.FullName);
                    if (f == null)
                    {
                        continue;
                    }
                    byte[] data = ReadEntry(ze);
                    await FileIO.WriteBytesAsync(f, data);
                }
            }

            // The archive may nest everything in a single top folder; find the real root.
            StorageFolder chartRoot = await FindChartFolderAsync(target, 2);
            if (chartRoot == null)
            {
                throw new InvalidOperationException("压缩包里没有找到 chart.json / chart.pec");
            }

            string rel = chartRoot.Path.Length > target.Path.Length
                ? chartRoot.Path.Substring(target.Path.Length).Trim('\\')
                : string.Empty;
            string baseUri = "ms-appdata:///local/" + LocalChartsFolder + "/" + id
                + (rel.Length > 0 ? "/" + rel.Replace('\\', '/') : string.Empty);

            ChartEntry entry = await TryLoadEntryAsync(baseUri, true, "local:" + id);
            if (entry == null)
            {
                throw new InvalidOperationException("安装失败：无法识别压缩包里的谱面");
            }
            return entry;
        }

        private static byte[] ReadEntry(ZipArchiveEntry entry)
        {
            long len = entry.Length;
            if (len > int.MaxValue)
            {
                throw new InvalidOperationException("压缩包里的文件过大");
            }
            byte[] data = new byte[(int)len];
            using (Stream s = entry.Open())
            {
                int off = 0;
                while (off < data.Length)
                {
                    int n = s.Read(data, off, data.Length - off);
                    if (n <= 0)
                    {
                        break;
                    }
                    off += n;
                }
            }
            return data;
        }

        /// <summary>Creates `relativePath` under `root`, making intermediate folders.</summary>
        private static async Task<StorageFile> EnsureFileAsync(StorageFolder root, string relativePath)
        {
            string[] parts = relativePath.Replace('\\', '/').Split('/');
            StorageFolder dir = root;
            for (int i = 0; i < parts.Length - 1; i++)
            {
                string seg = SafeSegment(parts[i]);
                if (seg == null)
                {
                    continue;
                }
                dir = await dir.CreateFolderAsync(seg, CreationCollisionOption.OpenIfExists);
            }
            string leaf = SafeSegment(parts[parts.Length - 1]);
            if (leaf == null)
            {
                return null;
            }
            return await dir.CreateFileAsync(leaf, CreationCollisionOption.ReplaceExisting);
        }

        /// <summary>Rejects traversal segments and strips characters the filesystem refuses.</summary>
        private static string SafeSegment(string segment)
        {
            if (string.IsNullOrEmpty(segment) || segment == "." || segment == "..")
            {
                return null;
            }
            char[] invalid = Path.GetInvalidFileNameChars();
            System.Text.StringBuilder sb = new System.Text.StringBuilder(segment.Length);
            for (int i = 0; i < segment.Length; i++)
            {
                char c = segment[i];
                bool bad = false;
                for (int j = 0; j < invalid.Length; j++)
                {
                    if (invalid[j] == c)
                    {
                        bad = true;
                        break;
                    }
                }
                sb.Append(bad ? '_' : c);
            }
            string s = sb.ToString().Trim();
            return s.Length == 0 ? null : s;
        }

        /// <summary>Finds the folder that actually holds the chart file, up to `depth` levels down.</summary>
        private static async Task<StorageFolder> FindChartFolderAsync(StorageFolder folder, int depth)
        {
            try
            {
                IReadOnlyList<StorageFile> files = await folder.GetFilesAsync();
                for (int i = 0; i < files.Count; i++)
                {
                    if (ChartParser.PickChartFile(new List<string> { files[i].Name }) != null)
                    {
                        return folder;
                    }
                }
            }
            catch (Exception)
            {
            }

            if (depth <= 0)
            {
                return null;
            }
            try
            {
                IReadOnlyList<StorageFolder> subs = await folder.GetFoldersAsync();
                for (int i = 0; i < subs.Count; i++)
                {
                    StorageFolder r = await FindChartFolderAsync(subs[i], depth - 1);
                    if (r != null)
                    {
                        return r;
                    }
                }
            }
            catch (Exception)
            {
            }
            return null;
        }

        /// <summary>
        /// The chart's directory name inside <c>LocalFolder\charts</c> - <c>190</c> for
        /// <c>ms-appdata:///local/charts/190</c>.
        ///
        /// Null for packaged content (the bundled demo), which has no directory in local storage at
        /// all. That distinction matters more than it looks: the Rust engine reads every asset -
        /// chart, illustration, audio, resource pack, UI images - through one
        /// <see cref="PrprPort.AssetRoot"/>, a real filesystem path resolved with
        /// <c>GetFolderFromPathAsync</c>, which cannot address <c>ms-appx:</c> content.
        /// </summary>
        public static string LocalFolderName(ChartEntry entry)
        {
            if (entry == null || !entry.IsLocalFolder)
            {
                return null;
            }
            string tail = entry.BasePath.Substring(LocalBaseUri.Length);
            int slash = tail.IndexOf('/');
            return slash >= 0 ? tail.Substring(0, slash) : tail;
        }

        public static async Task DeleteAsync(ChartEntry entry)
        {
            if (entry == null || !entry.IsLocalFolder)
            {
                return;
            }
            string tail = LocalFolderName(entry);
            StorageFolder root = await ApplicationData.Current.LocalFolder.CreateFolderAsync(
                LocalChartsFolder, CreationCollisionOption.OpenIfExists);
            try
            {
                StorageFolder f = await root.GetFolderAsync(tail);
                await f.DeleteAsync();
            }
            catch (Exception)
            {
            }
        }

        private static string Sanitize(string name)
        {
            char[] invalid = System.IO.Path.GetInvalidFileNameChars();
            System.Text.StringBuilder sb = new System.Text.StringBuilder();
            for (int i = 0; i < name.Length; i++)
            {
                char c = name[i];
                bool bad = false;
                for (int j = 0; j < invalid.Length; j++)
                {
                    if (invalid[j] == c) { bad = true; break; }
                }
                sb.Append(bad ? '_' : c);
            }
            string s = sb.ToString().Trim();
            if (s.Length == 0)
            {
                s = "chart";
            }
            return s;
        }
    }
}
