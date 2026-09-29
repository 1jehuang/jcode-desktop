# GitHub applet

Your GitHub pull requests, review requests and issues in a Jcode Desktop panel.

- Tabs: **Review requested**, **My PRs**, **Assigned**, **My issues**, **Mentions**, and free **Search**
  (`is:open label:bug`, etc.). Filter every tab by `owner/repo`.
- Detail view: state, review decision, CI checks, branch, diff size, description, latest comments.
- Actions: open on GitHub, copy link, **Ask Jcode** (starts a chat that reviews the PR or works the issue),
  comment, approve, close and reopen.
- Refreshes every 5 minutes while the inbox is open.
- Tool cards: calls to the GitHub MCP server (`mcp__github__*`) render as cards in chat, showing
  pull requests, issues, lists, search results, CI checks, and writes (opened, merged, commented).
  Cards use only the call's own JSON output. Bash `gh` calls are not claimed.

Requires the GitHub CLI signed in (`gh auth login`). The provider only calls `gh`, so it never
handles a token. Standard-library Python 3.9+.

## Install

```sh
scripts/install-applet.sh github
```

This copies the applet to `~/.jcode/applets/github`. Desktop starts it on launch (or on the next
Ctrl+R). Open the inbox from the command palette (**GitHub: issues and pull requests**). The first
launch asks you to approve its capabilities: `open_url`, `clipboard`, `start_chat`.

## GitHub MCP server

The tool cards need the official server (github/github-mcp-server) registered as `github` in
`~/.jcode/mcp.json`. A small wrapper keeps the token out of the config:

```sh
#!/bin/sh
GITHUB_PERSONAL_ACCESS_TOKEN="$(gh auth token)" || exit 1
export GITHUB_PERSONAL_ACCESS_TOKEN
exec github-mcp-server stdio --toolsets=context,repos,issues,pull_requests,actions
```

```json
"github": {"command": "/home/you/.jcode/mcp/github.sh", "args": [], "env": {}, "shared": true}
```
