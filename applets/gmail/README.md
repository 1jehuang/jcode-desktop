# Gmail applet

Your Gmail inbox in a Jcode Desktop panel.

- Tabs: **Inbox** (unread count), **Unread**, **Starred**, **Important**, and free **Search** using
  Gmail search syntax (`from:alice has:attachment`).
- Detail view: sender, recipients, date, labels and the message body (plain text, or HTML
  stripped to text). Opening a message marks it read.
- Actions: open in Gmail, **Ask Jcode** (starts a chat that summarizes the email and drafts a
  reply), star, mark read or unread, archive, delete (moves to Trash, with confirmation), and
  **Save reply draft** (a threaded reply saved to Gmail Drafts, never sent).
- Refreshes every 5 minutes while the inbox is open.

Uses the Google login Jcode already has (`jcode login google`). Star, read state, archive and
delete need the full access tier. A read and draft login shows only the read and draft actions.
The provider refreshes expired access tokens with the same OAuth client and writes them back in
Jcode's format, so the CLI keeps working. Standard-library Python 3.9+.

## Install

```sh
scripts/install-applet.sh gmail
```

This copies the applet to `~/.jcode/applets/gmail`. Desktop starts it on launch (or on the next
Ctrl+R) and adds a **Gmail** pill to the sidebar launcher row. The first launch asks you to approve
its capabilities: `open_url`, `start_chat`.

## Test

```sh
python3 applets/gmail/test_provider.py
```

Runs the provider against a local fake Gmail API, so it never touches real mail.
