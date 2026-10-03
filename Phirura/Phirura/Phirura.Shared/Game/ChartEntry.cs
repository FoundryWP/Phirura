using System;
using System.Collections.Generic;
using Phirura.Parse;

namespace Phirura.Game
{
    /// <summary>
    /// Where a chart's files live, and its metadata, as shown in the song list.
    ///
    /// Everything the XAML binds to must be a *property*: WinRT data binding resolves
    /// against the type's properties only, so a public field silently produces
    /// "BindingExpression path error: 'X' property not found" and renders blank rows.
    /// C# 5 has no property initialisers, hence the constructor.
    /// </summary>
    public sealed class ChartEntry
    {
        /// <summary>Stable identifier ("appx:demo" or a folder path).</summary>
        public string Id { get; set; }

        /// <summary>Folder URI, e.g. "ms-appx:///Assets/demo" or a StorageFolder path.</summary>
        public string BasePath { get; set; }

        /// <summary>True when BasePath refers to a StorageFolder rather than packaged content.</summary>
        public bool IsLocalFolder { get; set; }

        public string ChartFile { get; set; }
        public string AudioFile { get; set; }
        public string IllustrationFile { get; set; }

        public string Title { get; set; }
        public string Artist { get; set; }
        public string Charter { get; set; }
        public string Level { get; set; }
        public string Difficulty { get; set; }

        public double PreviewStart { get; set; }
        public double PreviewEnd { get; set; }

        public string Format { get; set; }

        /// <summary>Populated after the chart has been parsed (cached for the game page).</summary>
        public Core.Chart ParsedChart { get; set; }
        public List<string> Warnings { get; set; }

        public ChartEntry()
        {
            Id = string.Empty;
            BasePath = string.Empty;
            ChartFile = string.Empty;
            AudioFile = string.Empty;
            Title = "Untitled";
            Artist = string.Empty;
            Charter = string.Empty;
            Level = string.Empty;
            Difficulty = string.Empty;
            Format = "RPE";
            Warnings = new List<string>();
        }

        public string LevelDisplay
        {
            get
            {
                if (!string.IsNullOrEmpty(Level))
                {
                    return Level;
                }
                if (!string.IsNullOrEmpty(Difficulty))
                {
                    return "Lv." + Difficulty;
                }
                return "Lv.?";
            }
        }

        public string Subtitle
        {
            get
            {
                string s = Artist;
                if (!string.IsNullOrEmpty(Charter))
                {
                    s += (s.Length > 0 ? "  /  " : "") + "谱面 " + Charter;
                }
                return s;
            }
        }

        public static ChartEntry FromInfo(string id, string basePath, bool isLocalFolder,
                                          string chartFile, string audioFile, string illustrationFile,
                                          ChartInfo info)
        {
            ChartEntry e = new ChartEntry();
            e.Id = id;
            e.BasePath = basePath;
            e.IsLocalFolder = isLocalFolder;
            e.ChartFile = chartFile;
            e.AudioFile = audioFile;
            e.IllustrationFile = illustrationFile;
            if (info != null)
            {
                e.Title = string.IsNullOrEmpty(info.Name) ? "Untitled" : info.Name;
                e.Artist = info.Composer;
                e.Charter = info.Charter;
                e.Level = info.Level;
                e.Difficulty = info.Difficulty;
                e.PreviewStart = info.PreviewStart;
                e.PreviewEnd = info.PreviewEnd;
            }
            else
            {
                e.Title = System.IO.Path.GetFileNameWithoutExtension(chartFile);
            }
            return e;
        }
    }

    /// <summary>Player-tunable settings, persisted in ApplicationData.</summary>
    public sealed class GameSettings
    {
        /// <summary>Audio/visual offset in seconds; positive means "notes are late".</summary>
        public double Offset;

        public double NoteScale = 1.0;
        public double LineThickness = 1.0;
        public bool Autoplay;
        public bool FlipVertical;
        public bool ShowHitEffects = true;
        public double VolumeMusic = 0.8;
        public double VolumeSfx = 0.7;

        private const string KeyOffset = "phirura.offset";
        private const string KeyAutoplay = "phirura.autoplay";
        private const string KeyFlip = "phirura.flip";
        private const string KeyNoteScale = "phirura.noteScale";
        private const string KeyVolume = "phirura.volume";

        public static GameSettings Load()
        {
            GameSettings s = new GameSettings();
            try
            {
                Windows.Storage.ApplicationDataContainer c = Windows.Storage.ApplicationData.Current.LocalSettings;
                if (c.Values.ContainsKey(KeyOffset)) s.Offset = (double)c.Values[KeyOffset];
                if (c.Values.ContainsKey(KeyAutoplay)) s.Autoplay = (bool)c.Values[KeyAutoplay];
                if (c.Values.ContainsKey(KeyFlip)) s.FlipVertical = (bool)c.Values[KeyFlip];
                if (c.Values.ContainsKey(KeyNoteScale)) s.NoteScale = (double)c.Values[KeyNoteScale];
                if (c.Values.ContainsKey(KeyVolume)) s.VolumeMusic = (double)c.Values[KeyVolume];
            }
            catch (Exception)
            {
            }
            return s;
        }

        public void Save()
        {
            try
            {
                Windows.Storage.ApplicationDataContainer c = Windows.Storage.ApplicationData.Current.LocalSettings;
                c.Values[KeyOffset] = Offset;
                c.Values[KeyAutoplay] = Autoplay;
                c.Values[KeyFlip] = FlipVertical;
                c.Values[KeyNoteScale] = NoteScale;
                c.Values[KeyVolume] = VolumeMusic;
            }
            catch (Exception)
            {
            }
        }
    }
}
