using System;
using Phirura.Net;
using Windows.Phone.UI.Input;
using Windows.System;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Navigation;

namespace Phirura
{
    /// <summary>Sign in to a Phira account (email + password) and keep the bearer token.</summary>
    public sealed partial class LoginPage : Page
    {
        private const string RegisterUrl = PhiraApi.BaseUrl + "/register";
        private const string ResetUrl = PhiraApi.BaseUrl + "/reset-password";

        private bool _busy;

        public LoginPage()
        {
            this.InitializeComponent();
            this.NavigationCacheMode = NavigationCacheMode.Disabled;
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            HardwareButtons.BackPressed += OnBackPressed;
            PhiraSession.Load();
            RefreshState();
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

        private void RefreshState()
        {
            bool loggedIn = PhiraSession.IsLoggedIn;
            AccountPanel.Visibility = loggedIn ? Visibility.Visible : Visibility.Collapsed;
            FormPanel.Visibility = loggedIn ? Visibility.Collapsed : Visibility.Visible;

            if (loggedIn)
            {
                AccountNameText.Text = PhiraSession.DisplayName;
                PhiraUser u = PhiraSession.User;
                AccountInfoText.Text = u != null
                    ? "UID " + u.Id + "  ·  RKS " + u.Rks.ToString("0.00")
                    : "已保存登录令牌";
            }
        }

        private void SetBusy(bool busy, string message)
        {
            _busy = busy;
            LoginButton.IsEnabled = !busy;
            BusyBar.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
            StatusText.Text = message ?? string.Empty;
        }

        private async void LoginButton_Click(object sender, RoutedEventArgs e)
        {
            if (_busy)
            {
                return;
            }
            string email = EmailBox.Text == null ? string.Empty : EmailBox.Text.Trim();
            string password = PasswordBox.Password == null ? string.Empty : PasswordBox.Password;

            if (email.Length == 0 || password.Length == 0)
            {
                SetBusy(false, "请先填写邮箱和密码。");
                return;
            }

            SetBusy(true, "正在登录 …");
            try
            {
                string token = await PhiraApi.LoginAsync(email, password);
                PhiraSession.Token = token;

                SetBusy(true, "正在读取账号信息 …");
                PhiraUser me = null;
                try
                {
                    me = await PhiraApi.GetMeAsync(token);
                }
                catch (Exception)
                {
                    // The token is still valid even if /me hiccups; keep the session.
                }
                PhiraSession.User = me;
                PhiraSession.Save();

                PasswordBox.Password = string.Empty;
                SetBusy(false, me != null ? "登录成功：" + me.Name : "登录成功。");
                RefreshState();
            }
            catch (PhiraApiException ex)
            {
                SetBusy(false, "登录失败：" + ex.Message
                    + (string.IsNullOrEmpty(ex.Code) ? string.Empty : "（" + ex.Code + "）"));
            }
            catch (Exception ex)
            {
                SetBusy(false, "登录失败：" + ex.Message);
            }
        }

        private void LogoutButton_Click(object sender, RoutedEventArgs e)
        {
            PhiraSession.Clear();
            PasswordBox.Password = string.Empty;
            StatusText.Text = "已退出登录。";
            RefreshState();
        }

        private void BrowseButton_Click(object sender, RoutedEventArgs e)
        {
            this.Frame.Navigate(typeof(OnlinePage));
        }

        private async void RegisterButton_Click(object sender, RoutedEventArgs e)
        {
            try
            {
                await Launcher.LaunchUriAsync(new Uri(ResetUrl));
            }
            catch (Exception)
            {
                StatusText.Text = "请在浏览器中打开 " + RegisterUrl;
            }
        }
    }
}
