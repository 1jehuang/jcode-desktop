# Google Sheets applet

Google Sheets access for the agent, shown to you as live tables in Jcode Desktop.

The folder holds two pieces that share `gsheets.py`:

- **`mcp_server.py`**, a stdio MCP server. It gives the agent `info`, `read`, `write`,
  `append`, `clear`, `create` and `add_tab`. Spreadsheets can be named by id or by URL. It's an
  MCP server, not a native Jcode tool, so any MCP client can use it.
- **`provider.py`**, the Desktop applet. Its manifest claims the server's tool calls
  (`mcp__sheets__*`, and `mcp_call` with server `sheets`). Each call is replaced by a card:
  - While the call runs, the card shows progress.
  - When it finishes, the card fetches the affected cells live and shows them as a table.
    Reads show the range that was read. Writes and appends show the rows that changed, under
    the tab's header row.
  - Tab pills, **Refresh** and **Open in Sheets** are on the card.

All Google access goes through the [`gog`](https://github.com/steipete/gogcli) CLI, so neither
process ever holds an OAuth token. Sign in once with `gog auth add <email> --services sheets`.
gog's file keyring needs `GOG_KEYRING_PASSWORD` when no terminal is attached. If that
variable is unset, it's read from `~/.local/share/gogcli/local-keyring-password`.

Standard-library Python 3.9+.

## Install

```sh
scripts/install-applet.sh sheets
```

Then register the MCP server in `~/.jcode/mcp.json`:

```json
{
  "servers": {
    "sheets": {
      "command": "python3",
      "args": ["~/.jcode/applets/sheets/mcp_server.py"],
      "shared": true
    }
  }
}
```

Use the absolute path, since `~` isn't expanded. Desktop starts the applet on launch or on the
next Ctrl+R. The first time, it asks you to approve the `open_url` capability.

## Test

```sh
python3 applets/sheets/test_sheets.py
```

A fake `gog` stands in for Google, so the tests never touch a real spreadsheet.
