# Optional Jcode account sign-in

Desktop offers **Sign in with email** and **Skip for now** on first launch.
The Jcode account is separate from AI-provider login. Neither choice changes
existing providers, models, sessions, or billing settings.

Sign-in opens the existing Jcode account page in the default browser. Enter an
email address, open the one-time magic link in that same browser, and approve
the waiting device. Desktop checks for approval automatically and stores the
credential in the shared, owner-only Jcode account store. No passwords or email
links are handled by Desktop. Starting the flow alone does not send an email.

- Skip, or press Escape, to continue without an account. The choice is remembered
  as `desktop.workspace.account_sign_in_handled` in the shared Jcode config
  (`workspace.account_sign_in_handled` with `JCODE_DESKTOP_CONFIG`).
- Sign in later from **Settings → Jcode account → Sign in with email**.
- After connecting, the same Settings section links to account management.
- If the browser does not open, use **Open browser again**.
- If the flow expires or is denied, retry or skip. Temporary connection failures
  retry until expiry while leaving Skip available.
- **Cancel and go back**, Skip, or reloading Desktop cancels
  local polling. A new attempt starts a new flow. Already-issued server keys are
  not revoked by cancellation, but late results are never saved locally.

Device codes, credentials, URLs, and account emails are not added to workspace
snapshots, transcripts, or logs. Restored welcome screens start fresh rather
than resuming secret-bearing state. Existing local account credentials suppress
the initial offer without making a startup network request.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib account_sign_in
python3 scripts/screenshot.py target/account-sign-in.png --account-sign-in-interact
python3 scripts/screenshot.py target/account-sign-in-compact.png --account-sign-in --size 640x800
```

The screenshot harness runs the real controls on private Xvfb with offline
approval-waiting fixtures. It never opens a browser, sends email, or writes an
account credential. Shared HTTP-contract, error-redaction, URL-validation, and
credential-permission tests live in Jcode's `account_login` module.
