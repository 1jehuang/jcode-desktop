# GitHub applet

Your GitHub pull requests, review requests and issues in a Jcode Desktop panel.

- Tabs: **Review requested**, **My PRs**, **Assigned**, **My issues**, **Mentions**, and free **Search**
  (`is:open label:bug`, etc.). Filter every tab by `owner/repo`.
- Detail view: state, review decision, CI checks, branch, diff size, description, latest comments.
- Actions: open on GitHub, copy link, **Ask Jcode** (starts a chat that reviews the PR or works the issue),
  comment, approve, close and reopen.
- Refreshes every 5 minutes while the inbox is open.

Requires the GitHub CLI signed in (`gh auth login`). The provider only calls `gh`, so it never
handles a token. Standard-library Python 3.9+.

## Install

```sh
scripts/install-applet.sh github
```

This copies the applet to `~/.jcode/applets/github`. Desktop starts it on launch (or on the next
Ctrl+R) and adds a **GitHub** pill to the sidebar launcher row. The first launch asks you to approve
its capabilities: `open_url`, `clipboard`, `start_chat`.
