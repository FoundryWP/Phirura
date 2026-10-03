using System;
using System.Collections.Generic;
using Phirura.Game;
using Phirura.Net;
using Windows.Phone.UI.Input;
using Windows.UI.Core;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Input;
using Windows.UI.Xaml.Navigation;

namespace Phirura
{
    /// <summary>
    /// Browses the Phira online chart library and installs a chart by downloading its
    /// archive and unpacking it into the local chart store.
    /// </summary>
    public sealed partial class OnlinePage : Page
    {
        private const int PageSize = 20;

        /// <summary>Order values the official client sends (`-` prefix reverses).</summary>
        private static readonly string[] OrderValues = { "updated", "rating", "-difficulty", "name" };
        private static readonly string[] OrderLabels = { "最近更新", "评分", "难度", "曲名" };

        private readonly List<PhiraChart> _charts = new List<PhiraChart>();
        private int _page;
        private bool _busy;
        private bool _initialised;

        public OnlinePage()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Disabled;
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            HardwareButtons.BackPressed += OnBackPressed;

            PhiraSession.Load();
            AccountText.Text = PhiraSession.IsLoggedIn
                ? "已登录：" + PhiraSession.DisplayName
                : "未登录（仍可浏览公开谱面）";

            if (!_initialised)
            {
                _initialised = true;
                for (int i = 0; i < OrderLabels.Length; i++)
                {
                    OrderBox.Items.Add(new ComboBoxItem { Content = OrderLabels[i] });
                }
                OrderBox.SelectedIndex = 0;
                LoadPage(0);
            }
        }

        protected override void OnNavigatedFrom(NavigationEventArgs e)
        {
            base.OnNavigatedFrom(e);
            HardwareButtons.BackPressed -= OnBackPressed;
        }

        private void OnBackPressed(object sender, BackPressedEventArgs e)
        {
            e.Handled = true;
            if (this.Frame.CanGoBack)
            {
                this.Frame.GoBack();
            }
        }

        // ------------------------------------------------------------------
        // Search / paging
        // ------------------------------------------------------------------

        private void SearchButton_Click(object sender, RoutedEventArgs e)
        {
            LoadPage(0);
        }

        private void SearchBox_KeyDown(object sender, KeyRoutedEventArgs e)
        {
            if (e.Key == Windows.System.VirtualKey.Enter)
            {
                LoadPage(0);
            }
        }

        private void OrderBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
        {
            if (_initialised && !_busy)
            {
                LoadPage(0);
            }
        }

        private void PrevButton_Click(object sender, RoutedEventArgs e)
        {
            if (_page > 0)
            {
                LoadPage(_page - 1);
            }
        }

        private void NextButton_Click(object sender, RoutedEventArgs e)
        {
            LoadPage(_page + 1);
        }

        private void BackButton_Click(object sender, RoutedEventArgs e)
        {
            if (this.Frame.CanGoBack)
            {
                this.Frame.GoBack();
            }
        }

        private void SetBusy(bool busy, string message)
        {
            _busy = busy;
            BusyBar.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
            BusyBar.IsIndeterminate = true;
            PrevButton.IsEnabled = !busy && _page > 0;
            NextButton.IsEnabled = !busy;
            StatusText.Text = message ?? string.Empty;
        }

        private string CurrentOrder()
        {
            int i = OrderBox.SelectedIndex;
            if (i < 0 || i >= OrderValues.Length)
            {
                i = 0;
            }
            return OrderValues[i];
        }

        private async void LoadPage(int page)
        {
            if (_busy)
            {
                return;
            }
            if (page < 0)
            {
                page = 0;
            }
            string search = SearchBox.Text == null ? string.Empty : SearchBox.Text.Trim();

            SetBusy(true, "正在读取谱面列表 …");
            try
            {
                List<PhiraChart> list = await PhiraApi.SearchChartsAsync(
                    PhiraSession.Token, search, CurrentOrder(), page, PageSize);

                _page = page;
                _charts.Clear();
                _charts.AddRange(list);
                ChartList.ItemsSource = null;
                ChartList.ItemsSource = _charts;

                PrevButton.IsEnabled = _page > 0;
                NextButton.IsEnabled = list.Count >= PageSize;
                SetBusy(false, list.Count == 0
                    ? "这一页没有谱面。"
                    : "第 " + (_page + 1) + " 页，共 " + list.Count + " 首。");
            }
            catch (PhiraApiException ex)
            {
                SetBusy(false, "读取失败：" + ex.Message
                    + (string.IsNullOrEmpty(ex.Code) ? string.Empty : "（" + ex.Code + "）"));
            }
            catch (Exception ex)
            {
                SetBusy(false, "读取失败：" + ex.Message);
            }
        }

        // ------------------------------------------------------------------
        // Download
        // ------------------------------------------------------------------

        private async void DownloadButton_Click(object sender, RoutedEventArgs e)
        {
            if (_busy)
            {
                return;
            }
            FrameworkElement el = sender as FrameworkElement;
            PhiraChart chart = el == null ? null : el.DataContext as PhiraChart;
            if (chart == null)
            {
                return;
            }

            SetBusy(true, "正在下载「" + chart.ChartName + "」…");
            BusyBar.IsIndeterminate = false;
            BusyBar.Value = 0;
            try
            {
                byte[] archive = await PhiraApi.DownloadAsync(
                    PhiraSession.Token, chart.FileUrl, OnDownloadProgress);

                SetBusy(true, "正在解压安装 …");
                BusyBar.IsIndeterminate = true;
                ChartEntry entry = await ChartLibrary.InstallArchiveAsync(archive, chart.Id);

                // The song list caches its entries, so make sure the new chart shows up.
                MainPage.RequestReload = true;
                SetBusy(false, "已安装：" + entry.Title + "（" + entry.LevelDisplay + "）");
            }
            catch (PhiraApiException ex)
            {
                SetBusy(false, "下载失败：" + ex.Message
                    + (string.IsNullOrEmpty(ex.Code) ? string.Empty : "（" + ex.Code + "）"));
            }
            catch (Exception ex)
            {
                SetBusy(false, "安装失败：" + ex.Message);
            }
        }

        private void OnDownloadProgress(double p)
        {
            // Downloads report from a background thread. The dispatch is deliberately
            // fire-and-forget: progress updates are droppable UI sugar.
            CoreDispatcher d = this.Dispatcher;
            if (d == null)
            {
                return;
            }
#pragma warning disable 4014
            d.RunAsync(CoreDispatcherPriority.Low, delegate
            {
                if (BusyBar.IsIndeterminate)
                {
                    BusyBar.IsIndeterminate = false;
                }
                BusyBar.Value = p * 100.0;
                StatusText.Text = "正在下载 … " + ((int)(p * 100)).ToString() + "%";
            });
#pragma warning restore 4014
        }
    }
}
