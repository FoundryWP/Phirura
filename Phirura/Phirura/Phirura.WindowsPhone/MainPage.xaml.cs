using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Phirura.Game;
using Phirura.Parse;
using Windows.Storage;
using Windows.Storage.Pickers;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Controls.Primitives;
using Windows.UI.Xaml.Navigation;

namespace Phirura
{
    /// <summary>Song select. Also hosts the offset / autoplay / orientation settings.</summary>
    public sealed partial class MainPage : Page
    {
        /// <summary>Handed to GamePage through navigation (arbitrary objects are not serialisable parameters).</summary>
        public static ChartEntry PendingChart;

        /// <summary>Set by pages that install a chart so the list refreshes when we come back.</summary>
        public static bool RequestReload;

        private List<ChartEntry> _entries = new List<ChartEntry>();
        private GameSettings _settings;
        private bool _loading;

        public MainPage()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Required;
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            _settings = GameSettings.Load();
            OffsetSlider.Value = _settings.Offset * 1000.0;
            AutoplayBox.IsChecked = _settings.Autoplay;
            FlipBox.IsChecked = _settings.FlipVertical;
            UpdateOffsetText();
            UpdateAccountText();

            if (RequestReload)
            {
                RequestReload = false;
                _entries.Clear();
                LoadCharts();
            }
            else if (_entries.Count == 0)
            {
                LoadCharts();
            }
        }

        private void UpdateAccountText()
        {
            Net.PhiraSession.Load();
            AccountButton.Content = Net.PhiraSession.IsLoggedIn
                ? "账号：" + Net.PhiraSession.DisplayName
                : "账号 / 登录";
        }

        private void AccountButton_Click(object sender, RoutedEventArgs e)
        {
            this.Frame.Navigate(typeof(LoginPage));
        }

        private void OnlineButton_Click(object sender, RoutedEventArgs e)
        {
            this.Frame.Navigate(typeof(OnlinePage));
        }

        private async void LoadCharts()
        {
            if (_loading)
            {
                return;
            }
            _loading = true;
            StatusText.Text = "正在读取谱面…";
            try
            {
                _entries = await ChartLibrary.LoadAllAsync();
                SongList.ItemsSource = null;
                SongList.ItemsSource = _entries;
                StatusText.Text = _entries.Count <= 1
                    ? "内置了一首示例谱面。点“导入谱面”后，在文件选择器里把谱面文件夹中的 chart.json（或 chart.pec）、音频和图片一起多选进来即可。"
                    : "共 " + _entries.Count + " 首谱面。";
                AppLog.Write("MainPage: loaded " + _entries.Count + " chart(s)");
            }
            catch (Exception ex)
            {
                AppLog.Write("MainPage.LoadCharts", ex);
                StatusText.Text = "读取失败：" + ex.Message;
            }
            _loading = false;
        }

        private void UpdateOffsetText()
        {
            OffsetText.Text = ((int)Math.Round(OffsetSlider.Value)) + " ms";
        }

        private void SongList_ItemClick(object sender, ItemClickEventArgs e)
        {
            ChartEntry entry = e.ClickedItem as ChartEntry;
            if (entry == null)
            {
                return;
            }
            PendingChart = entry;
            AppLog.Write("MainPage: opening \"" + entry.Title + "\" (" + entry.LevelDisplay + ")");
            this.Frame.Navigate(typeof(GamePage));
        }

        private async void ImportButton_Click(object sender, RoutedEventArgs e)
        {
            IReadOnlyList<StorageFile> files = null;
            try
            {
                FileOpenPicker picker = new FileOpenPicker();
                picker.SuggestedStartLocation = PickerLocationId.DocumentsLibrary;
                picker.FileTypeFilter.Add(".json");
                picker.FileTypeFilter.Add(".pec");
                picker.FileTypeFilter.Add(".yml");
                picker.FileTypeFilter.Add(".yaml");
                picker.FileTypeFilter.Add(".wav");
                picker.FileTypeFilter.Add(".mp3");
                picker.FileTypeFilter.Add(".m4a");
                picker.FileTypeFilter.Add(".aac");
                picker.FileTypeFilter.Add(".png");
                picker.FileTypeFilter.Add(".jpg");
                picker.FileTypeFilter.Add(".jpeg");
                picker.FileTypeFilter.Add(".txt");

                AppLog.Write("import: opening picker");
                files = await picker.PickMultipleFilesAsync();
                AppLog.Write("import: picked " + (files == null ? "null" : files.Count.ToString()) + " file(s)");
            }
            catch (Exception ex)
            {
                AppLog.Write("import: picker failed", ex);
                StatusText.Text = "打开文件选择器失败：" + ex.Message
                    + "（可以把谱面放进应用本地文件夹 charts\\ 下，重启后即可看到）";
                return;
            }

            if (files == null || files.Count == 0)
            {
                return;
            }

            try
            {
                StatusText.Text = "正在导入 " + files.Count + " 个文件 …";
                ChartEntry entry = await ChartLibrary.ImportAsync(files);
                _entries.Add(entry);
                SongList.ItemsSource = null;
                SongList.ItemsSource = _entries;
                StatusText.Text = "已导入：" + entry.Title;
            }
            catch (Exception ex)
            {
                AppLog.Write("import: copy failed", ex);
                StatusText.Text = "导入失败：" + ex.Message;
            }
        }

        private async void DeleteButton_Click(object sender, RoutedEventArgs e)
        {
            ChartEntry sel = SongList.SelectedItem as ChartEntry;
            if (sel == null)
            {
                StatusText.Text = "先在列表中点击选中一首谱面，再点删除。";
                return;
            }
            await ChartLibrary.DeleteAsync(sel);
            _entries.Remove(sel);
            SongList.ItemsSource = null;
            SongList.ItemsSource = _entries;
            StatusText.Text = "已删除：" + sel.Title;
        }

        private void RefreshButton_Click(object sender, RoutedEventArgs e)
        {
            _entries.Clear();
            LoadCharts();
        }

        private void OffsetSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
        {
            if (_settings == null)
            {
                return;
            }
            UpdateOffsetText();
            _settings.Offset = OffsetSlider.Value / 1000.0;
            _settings.Save();
        }

        private void AutoplayBox_Changed(object sender, RoutedEventArgs e)
        {
            if (_settings == null)
            {
                return;
            }
            _settings.Autoplay = AutoplayBox.IsChecked == true;
            _settings.Save();
        }

        private void FlipBox_Changed(object sender, RoutedEventArgs e)
        {
            if (_settings == null)
            {
                return;
            }
            _settings.FlipVertical = FlipBox.IsChecked == true;
            _settings.Save();
        }
    }
}
