using System;
using Phirura.Game;
using Windows.Phone.UI.Input;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Navigation;

namespace Phirura
{
    /// <summary>
    /// Post-play summary. Reads the result the gameplay page parked in
    /// <see cref="GamePage.LastResult"/>.
    /// </summary>
    public sealed partial class ResultPage : Page
    {
        public ResultPage()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Disabled;
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            HardwareButtons.BackPressed += OnBackPressed;
            DisplayResult();
        }

        protected override void OnNavigatedFrom(NavigationEventArgs e)
        {
            base.OnNavigatedFrom(e);
            HardwareButtons.BackPressed -= OnBackPressed;
        }

        private void DisplayResult()
        {
            PlayResult r = GamePage.LastResult;
            TitleText.Text = string.IsNullOrEmpty(GamePage.LastChartTitle)
                ? "结算" : GamePage.LastChartTitle;

            if (r == null)
            {
                RankText.Text = string.Empty;
                HintText.Text = "没有可显示的结算数据。";
                return;
            }

            ScoreText.Text = r.Score.ToString("N0");
            AccText.Text = (r.Accuracy * 100.0).ToString("0.00") + "%";
            PerfectText.Text = r.Counts[0].ToString("N0");
            GoodText.Text = r.Counts[1].ToString("N0");
            BadText.Text = r.Counts[2].ToString("N0");
            MissText.Text = r.Counts[3].ToString("N0");
            ComboText.Text = r.MaxCombo.ToString("N0");
            NotesText.Text = r.NumOfNotes.ToString("N0");

            RankText.Text = DescribeRank(r);

            uint totalEarly = r.Early;
            uint totalLate = r.Late;
            EarlyLateText.Text = "早 " + totalEarly.ToString("N0") + "  ·  晚 " + totalLate.ToString("N0");

            if (r.NumOfNotes == 0)
            {
                HintText.Text = string.Empty;
            }
            else if (r.Counts[3] > 0 || r.Counts[2] > 0)
            {
                HintText.Text = "BAD 与 MISS 都会打断连击，注意按键的时机与位置。";
            }
            else if (!r.FullCombo)
            {
                HintText.Text = "全连了前半段，保持住。";
            }
            else
            {
                HintText.Text = "全连！再挑战一次满分吧。";
            }
        }

        /// <summary>Mirrors Phira's result badges (Phi / full combo).</summary>
        private static string DescribeRank(PlayResult r)
        {
            if (r.Score >= 1000000u)
            {
                return "PHI  ·  理论满分";
            }
            if (r.FullCombo)
            {
                return "FC  ·  全连";
            }
            if (r.Accuracy >= 0.995)
            {
                return "接近满分";
            }
            if (r.Accuracy >= 0.95)
            {
                return "表现优秀";
            }
            if (r.Accuracy >= 0.80)
            {
                return "可以更稳一些";
            }
            return "多练几遍就会好起来";
        }

        // ------------------------------------------------------------------
        // Navigation
        // ------------------------------------------------------------------

        private void OnBackPressed(object sender, BackPressedEventArgs e)
        {
            e.Handled = true;
            GoHome();
        }

        private void RetryButton_Click(object sender, RoutedEventArgs e)
        {
            // GamePage is not cached, so navigating to it builds a fresh instance which
            // re-reads MainPage.PendingChart and starts from the beginning.
            this.Frame.Navigate(typeof(GamePage));
            TrimBackStack(1);
        }

        private void BackButton_Click(object sender, RoutedEventArgs e)
        {
            GoHome();
        }

        private void GoHome()
        {
            this.Frame.Navigate(typeof(MainPage));
            TrimBackStack(0);
        }

        /// <summary>
        /// Drops play pages from the back stack after navigating, so pressing Back on the
        /// song list never re-enters a finished game.
        /// </summary>
        private void TrimBackStack(int keep)
        {
            try
            {
                while (this.Frame.BackStack.Count > keep)
                {
                    this.Frame.BackStack.RemoveAt(this.Frame.BackStack.Count - 1);
                }
            }
            catch (Exception)
            {
            }
        }
    }
}
