using System;
using System.Collections.Generic;
using Phirura.Core;

namespace Phirura.Parse
{
    /// <summary>Chart metadata read from a Phira `info.yml`.</summary>
    public sealed class ChartInfo
    {
        public string Name = string.Empty;
        public string Composer = string.Empty;
        public string Charter = string.Empty;
        public string Illustrator = string.Empty;
        public string Level = string.Empty;
        public string Difficulty = string.Empty;
        public double PreviewStart = 0.0;
        public double PreviewEnd = 0.0;

        public string DisplayTitle
        {
            get
            {
                if (string.IsNullOrEmpty(Name))
                {
                    return "Untitled";
                }
                return Name;
            }
        }

        public string DisplayArtist
        {
            get
            {
                if (string.IsNullOrEmpty(Composer))
                {
                    return string.IsNullOrEmpty(Charter) ? "Unknown" : Charter;
                }
                return Composer;
            }
        }
    }

    public static class ChartParser
    {
        /// <summary>
        /// Parses a chart, choosing the parser by **content** rather than by file name.
        ///
        /// Phira stores PEC charts in its archives under the uploader's file id with a
        /// `.json` suffix (e.g. `24432296.json`), so routing on the extension alone hands a
        /// PEC file to the RPE parser and the entire chart fails to load. The two formats
        /// are trivially distinguishable: RPE is JSON and starts with '{', PEC starts with
        /// the offset as a bare number on line 1.
        /// </summary>
        public static Chart Parse(string text, string extension, List<string> warnings)
        {
            if (text == null)
            {
                throw new ArgumentNullException("text");
            }
            string ext = (extension ?? string.Empty).TrimStart('.').ToLowerInvariant();

            // Trim a BOM as well as whitespace — an uploaded chart may carry one.
            string head = text.TrimStart('\uFEFF', ' ', '\t', '\r', '\n');
            if (head.Length > 0)
            {
                char c = head[0];
                if (c == '{')
                {
                    return RpeParser.Parse(text, warnings);
                }
                if (c == '-' || (c >= '0' && c <= '9'))
                {
                    return PecParser.Parse(text, warnings);
                }
            }

            // No recognisable signature — fall back to the extension.
            if (ext == "pec")
            {
                return PecParser.Parse(text, warnings);
            }
            if (ext == "json" || ext == "rpe")
            {
                return RpeParser.Parse(text, warnings);
            }
            return PecParser.Parse(text, warnings);
        }

        /// <summary>Picks the chart file out of a folder listing.</summary>
        public static string PickChartFile(IEnumerable<string> fileNames)
        {
            string best = null;
            int bestRank = int.MaxValue;
            foreach (string name in fileNames)
            {
                string lower = name.ToLowerInvariant();
                int rank;
                if (lower == "chart.json") rank = 0;
                else if (lower == "chart.pec") rank = 1;
                else if (lower.EndsWith(".json") && lower != "info.json") rank = 2;
                else if (lower.EndsWith(".pec")) rank = 3;
                else continue;
                if (rank < bestRank)
                {
                    bestRank = rank;
                    best = name;
                }
            }
            return best;
        }

        private static readonly string[] AudioExtensions = new string[] { ".wav", ".mp3", ".m4a", ".wma", ".aac" };

        /// <summary>Picks a playable audio file (WP8.1's MediaElement cannot decode ogg/opus).</summary>
        public static string PickAudioFile(IEnumerable<string> fileNames)
        {
            string fallback = null;
            foreach (string name in fileNames)
            {
                string lower = name.ToLowerInvariant();
                if (lower == "music.wav" || lower == "music.mp3")
                {
                    return name;
                }
                for (int i = 0; i < AudioExtensions.Length; i++)
                {
                    if (lower.EndsWith(AudioExtensions[i]))
                    {
                        if (fallback == null)
                        {
                            fallback = name;
                        }
                    }
                }
            }
            return fallback;
        }

        public static string PickIllustration(IEnumerable<string> fileNames)
        {
            string fallback = null;
            foreach (string name in fileNames)
            {
                string lower = name.ToLowerInvariant();
                if (lower == "illustration.png" || lower == "illustration.jpg" || lower == "background.png" || lower == "background.jpg")
                {
                    return name;
                }
                // Phira's own archives name the artwork after its file id (e.g. 56695308.jpg)
                // rather than "illustration.png", so fall back to the first image present.
                if (fallback == null &&
                    (lower.EndsWith(".png") || lower.EndsWith(".jpg") || lower.EndsWith(".jpeg")))
                {
                    fallback = name;
                }
            }
            return fallback;
        }

        public static string PickInfoFile(IEnumerable<string> fileNames)
        {
            foreach (string name in fileNames)
            {
                if (string.Equals(name, "info.yml", StringComparison.OrdinalIgnoreCase) ||
                    string.Equals(name, "info.yaml", StringComparison.OrdinalIgnoreCase))
                {
                    return name;
                }
            }
            return null;
        }

        /// <summary>
        /// Minimal `key: value` YAML reader — enough for Phira's info.yml, which is a
        /// flat map of scalars. Avoids taking a YAML dependency into the WP8.1 head.
        /// </summary>
        public static ChartInfo ParseInfoYaml(string text)
        {
            ChartInfo info = new ChartInfo();
            if (string.IsNullOrEmpty(text))
            {
                return info;
            }
            string[] lines = text.Replace("\r\n", "\n").Replace('\r', '\n').Split('\n');
            for (int i = 0; i < lines.Length; i++)
            {
                string line = lines[i];
                int hash = line.IndexOf('#');
                if (hash >= 0)
                {
                    line = line.Substring(0, hash);
                }
                int colon = line.IndexOf(':');
                if (colon <= 0)
                {
                    continue;
                }
                string key = line.Substring(0, colon).Trim().ToLowerInvariant();
                string value = line.Substring(colon + 1).Trim().Trim('"', '\'');
                switch (key)
                {
                    case "name": info.Name = value; break;
                    case "composer": info.Composer = value; break;
                    case "charter": info.Charter = value; break;
                    case "illustrator": info.Illustrator = value; break;
                    case "level": info.Level = value; break;
                    case "difficulty": info.Difficulty = value; break;
                    case "previewstart":
                        {
                            double ps;
                            if (double.TryParse(value, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out ps))
                            {
                                info.PreviewStart = ps;
                            }
                            break;
                        }
                    case "previewend":
                        {
                            double pe;
                            if (double.TryParse(value, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out pe))
                            {
                                info.PreviewEnd = pe;
                            }
                            break;
                        }
                }
            }
            return info;
        }
    }
}
