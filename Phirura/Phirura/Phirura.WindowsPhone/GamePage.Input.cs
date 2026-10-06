using System;
using Phirura.Core;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;

namespace Phirura
{
    // The shell half of the text-input bridge, in its own file: one sink interface, one overlay,
    // two buttons - see the note in Phirura.WindowsPhone.csproj for why it is not in GamePage.cs.
    //
    // The other half is `PrprPort.ITextInputSink` / `PrprPort.CompleteTextInput`, and the far end
    // is `HostCallbacks::text_input` and `wp81_input_complete` in Rust. The shape is deliberately
    // the same as the HTTP bridge: a request travels out with an id, and the answer comes back
    // later with that id. Nothing here blocks, and nothing here runs while phira is still inside
    // `request_input`.
    public sealed partial class GamePage
    {
        /// <summary>Request id of the open dialog, or null when there is none.</summary>
        private uint? _inputRequest;

        /// <summary>Whether the open dialog masks its characters (inputbox mode 1).</summary>
        private bool _inputSecret;

        /// <summary>Whether the open dialog is a multiline box (inputbox mode 2).</summary>
        private bool _inputMultiline;

        /// <summary>
        /// phira asked for a text field - open the dialog.
        ///
        /// Runs on the UI thread, because phira calls <c>request_input</c> from inside its frame
        /// loop. The hop through the dispatcher is still there on purpose: it lets the frame that
        /// is in progress finish before a modal dialog lands on top of it, and it means the
        /// "already on the UI thread" case does not have to be reasoned about separately.
        ///
        /// Nothing here calls back into Rust. <c>show_inputbox</c> is still on the stack - it has
        /// registered the callback but has not returned - so answering now would run that callback
        /// while <c>request_input</c> is still setting the request up. Everything else in this file
        /// runs from a button or from navigation, both of which are well clear of that stack.
        /// </summary>
        public void ShowTextInput(uint req, int mode, string title, string prompt,
            string defaultValue, string okLabel, string cancelLabel)
        {
            if (_inputRequest.HasValue)
            {
                // phira opens one dialog at a time, so a second request means something upstream
                // fired twice. Refuse it rather than overwriting `_inputRequest`, which would orphan
                // the first one - a callback nobody ever calls leaves phira's `take_input()`
                // returning nothing on every subsequent frame, forever. The refusal itself is
                // deferred for the reason in the doc comment above: this is still inside
                // `execute_async`.
                var skip = Dispatcher.RunAsync(Windows.UI.Core.CoreDispatcherPriority.Normal, () =>
                {
                    AppLog.Write("input: req=" + req + " refused: req=" + _inputRequest.Value
                        + " still open");
                    PrprPort.CompleteTextInput(req, null, false);
                });
                return;
            }

            _inputRequest = req;
            _inputSecret = mode == 1;
            _inputMultiline = mode == 2;

            var shown = Dispatcher.RunAsync(Windows.UI.Core.CoreDispatcherPriority.Normal, () =>
            {
                // Cancelled (page navigating away) between this call and the dispatcher getting
                // there. Checked rather than assumed: `OnNavigatedFrom` clears `_inputRequest`, and
                // re-showing an overlay for a request nobody is waiting on would strand a modal
                // dialog over a dead page.
                if (_inputRequest != req)
                {
                    AppLog.Write("input: req=" + req + " dropped: cancelled while queued");
                    return;
                }

                InputTitle.Text = title ?? string.Empty;
                InputPrompt.Text = prompt ?? string.Empty;
                InputPrompt.Visibility = string.IsNullOrEmpty(prompt) ? Visibility.Collapsed : Visibility.Visible;

                // The labels come from Rust, not from the XAML: `inputbox` localises them through
                // phira's `ttl!`, and the XAML's "OK"/"Cancel" is only a placeholder that keeps the
                // buttons laid out before the first request.
                InputCancelButton.Content = string.IsNullOrEmpty(cancelLabel) ? "Cancel" : cancelLabel;
                InputOkButton.Content = string.IsNullOrEmpty(okLabel) ? "OK" : okLabel;

                // Two boxes rather than one, because neither half of "a masked multiline field" is
                // available in WinRT: `PasswordBox` has no `PasswordChar` and `TextBox` has no way to
                // be masked. `AcceptsReturn` is likewise TextBox-only, and a multiline field also
                // needs a height or it renders as one line with a scrollbar inside a box that
                // looks empty.
                InputField.Visibility = _inputSecret ? Visibility.Collapsed : Visibility.Visible;
                InputSecretField.Visibility = _inputSecret ? Visibility.Visible : Visibility.Collapsed;
                InputField.AcceptsReturn = _inputMultiline;
                InputField.TextWrapping = _inputMultiline ? TextWrapping.Wrap : TextWrapping.NoWrap;
                InputField.Height = _inputMultiline ? 120 : double.NaN;
                ScrollViewer.SetVerticalScrollBarVisibility(InputField, _inputMultiline
                    ? ScrollBarVisibility.Auto
                    : ScrollBarVisibility.Hidden);

                InputField.Text = defaultValue ?? string.Empty;
                InputSecretField.Password = defaultValue ?? string.Empty;

                InputOverlay.Visibility = Visibility.Visible;

                // Focus immediately so the soft keyboard comes up with the dialog. Without this the
                // user has to tap the field first, and on a phone that first tap reads as "the
                // dialog did nothing".
                (_inputSecret ? (Control)InputSecretField : InputField).Focus(FocusState.Programmatic);

                AppLog.Write("input: req=" + req + " dialog shown secret=" + _inputSecret
                    + " multiline=" + _inputMultiline + " default_len="
                    + (defaultValue == null ? 0 : defaultValue.Length));
            });
        }

        /// <summary>
        /// Confirm. The text is read *before* the overlay comes down, because closing clears the
        /// fields - and an empty string here is a real answer, not a cancellation.
        /// </summary>
        private void InputOk_Click(object sender, RoutedEventArgs e)
        {
            if (!_inputRequest.HasValue)
            {
                return;
            }
            uint req = _inputRequest.Value;
            string text = _inputSecret ? InputSecretField.Password : InputField.Text;
            CloseTextInput();
            AppLog.Write("input: req=" + req + " confirmed len=" + (text == null ? 0 : text.Length));
            PrprPort.CompleteTextInput(req, text, true);
        }

        /// <summary>Cancel. No text travels back - Rust reads a null buffer as "dismissed".</summary>
        private void InputCancel_Click(object sender, RoutedEventArgs e)
        {
            if (!_inputRequest.HasValue)
            {
                return;
            }
            uint req = _inputRequest.Value;
            CloseTextInput();
            AppLog.Write("input: req=" + req + " cancelled");
            PrprPort.CompleteTextInput(req, null, false);
        }

        /// <summary>
        /// Cancels whatever dialog this page still has open.
        ///
        /// Called from <c>OnNavigatedFrom</c>. Walking away with a dialog up is the one case where
        /// nobody is going to press a button, and a request that is never answered would leave
        /// phira's <c>take_input()</c> returning nothing on every frame - the field would silently
        /// never accept a value again. Cancel is the right answer: phira reports it as a dismissal,
        /// which is not an error.
        /// </summary>
        private void CancelOpenTextInput()
        {
            if (!_inputRequest.HasValue)
            {
                return;
            }
            uint req = _inputRequest.Value;
            CloseTextInput();
            AppLog.Write("input: req=" + req + " cancelled: page navigating away");
            PrprPort.CompleteTextInput(req, null, false);
        }

        /// <summary>Hides the overlay and forgets the request. Does not answer it.</summary>
        private void CloseTextInput()
        {
            _inputRequest = null;
            InputOverlay.Visibility = Visibility.Collapsed;
            InputField.Text = string.Empty;
            InputSecretField.Password = string.Empty;
        }
    }
}
