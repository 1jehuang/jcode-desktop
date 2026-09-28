"""Google Sheets access shared by the MCP server and the applet provider.

Everything goes through the `gog` CLI, which already holds the user's Google
login, so neither process ever sees an OAuth token. `gog` keeps its refresh
token in an encrypted file keyring. Non-interactive processes unlock it with
GOG_KEYRING_PASSWORD, read from gog's local password file when unset.
Standard library only.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path

# Overridable so tests can substitute a fake gog.
GOG = os.environ.get("JCODE_SHEETS_GOG") or shutil.which("gog") or str(Path.home() / ".local/bin/gog")
PASSWORD_FILE = Path(
    os.environ.get("JCODE_SHEETS_GOG_PASSWORD_FILE", Path.home() / ".local/share/gogcli/local-keyring-password")
)
TIMEOUT = 45

_ID = re.compile(r"/spreadsheets/d/([A-Za-z0-9_-]+)")


class SheetsError(RuntimeError):
    pass


def spreadsheet_id(value: str) -> str:
    """Accept a bare id or any Google Sheets URL."""
    value = (value or "").strip()
    match = _ID.search(value)
    if match:
        return match.group(1)
    if not re.fullmatch(r"[A-Za-z0-9_-]{10,}", value):
        raise SheetsError(f"Not a spreadsheet id or URL: {value!r}")
    return value


def sheet_url(sid: str, gid: int | None = None) -> str:
    url = f"https://docs.google.com/spreadsheets/d/{sid}/edit"
    return f"{url}#gid={gid}" if gid is not None else url


def _env() -> dict:
    env = dict(os.environ)
    if not env.get("GOG_KEYRING_PASSWORD") and PASSWORD_FILE.is_file():
        env["GOG_KEYRING_PASSWORD"] = PASSWORD_FILE.read_text().strip()
    return env


def gog(*args: str) -> dict:
    """Run `gog --json sheets ...` and return its parsed JSON output."""
    if not GOG or not Path(GOG).exists() and not shutil.which(GOG):
        raise SheetsError("The gog CLI is not installed. Install gogcli and run `gog auth add`.")
    try:
        done = subprocess.run(
            [GOG, "--json", "--no-input", "sheets", *args],
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            env=_env(),
        )
    except subprocess.TimeoutExpired:
        raise SheetsError("Google Sheets did not respond in time. Retry.") from None
    if done.returncode != 0:
        detail = (done.stderr or done.stdout).strip().splitlines()
        message = detail[-1] if detail else f"gog exited with {done.returncode}"
        if "keyring" in message.lower() or "token" in message.lower():
            message += " (sign in with `gog auth add <email> --services sheets`)"
        raise SheetsError(message)
    out = done.stdout.strip()
    if not out:
        return {}
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        return {"output": out}


def quote_tab(tab: str) -> str:
    """A tab name usable in A1 notation."""
    if re.fullmatch(r"[A-Za-z0-9_]+", tab):
        return tab
    return "'" + tab.replace("'", "''") + "'"


def split_range(a1: str) -> tuple[str | None, str | None]:
    """'CRM!A1:C9' -> ('CRM', 'A1:C9'), 'A1:C9' -> (None, 'A1:C9'), 'CRM' -> ('CRM', None)."""
    a1 = (a1 or "").strip()
    if not a1:
        return None, None
    if "!" in a1:
        tab, cells = a1.rsplit("!", 1)
        tab = tab.strip()
        if tab.startswith("'") and tab.endswith("'"):
            tab = tab[1:-1].replace("''", "'")
        return tab or None, cells or None
    if re.fullmatch(r"\$?[A-Za-z]{1,3}\$?\d*(:\$?[A-Za-z]{1,3}\$?\d*)?", a1):
        return None, a1
    return a1, None


# -- operations -----------------------------------------------------------------


def info(sid: str) -> dict:
    meta = gog("metadata", sid)
    tabs = []
    for sheet in meta.get("sheets") or []:
        props = sheet.get("properties") or {}
        grid = props.get("gridProperties") or {}
        tabs.append(
            {
                "title": props.get("title", ""),
                "gid": props.get("sheetId"),
                "rows": grid.get("rowCount"),
                "columns": grid.get("columnCount"),
            }
        )
    return {"spreadsheet_id": sid, "title": meta.get("title", ""), "url": sheet_url(sid), "tabs": tabs}


def read(sid: str, a1: str) -> dict:
    data = gog("get", sid, a1)
    return {"range": data.get("range", a1), "values": data.get("values") or []}


def write(sid: str, a1: str, values: list, raw: bool = False) -> dict:
    return gog(
        "update", sid, a1, "--values-json", json.dumps(values), "--input", "RAW" if raw else "USER_ENTERED"
    )


def append(sid: str, a1: str, values: list, raw: bool = False) -> dict:
    return gog(
        "append", sid, a1, "--values-json", json.dumps(values), "--input", "RAW" if raw else "USER_ENTERED"
    )


def clear(sid: str, a1: str) -> dict:
    return gog("clear", sid, a1)


def create(title: str, tabs: list[str] | None = None) -> dict:
    args = ["create", title]
    if tabs:
        args += ["--sheets", ",".join(tabs)]
    data = gog(*args)
    sid = data.get("spreadsheetId", "")
    return {"spreadsheet_id": sid, "title": data.get("title", title), "url": data.get("spreadsheetUrl") or sheet_url(sid)}


def add_tab(sid: str, name: str) -> dict:
    data = gog("add-tab", sid, name)
    return {"spreadsheet_id": sid, "tab": data.get("title", name), "gid": data.get("sheetId")}


def freeze(sid: str, tab: str, rows: int) -> dict:
    return gog("freeze", sid, "--sheet", tab, "--rows", str(rows))
