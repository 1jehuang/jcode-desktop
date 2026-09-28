#!/usr/bin/env python3
"""Google Sheets tool cards for Jcode Desktop.

Speaks `jcode.applet/1` newline-delimited JSON on stdio. The manifest claims
the Sheets MCP server's tool calls (`mcp__sheets__*`, and `mcp_call` with
server "sheets"). While a call runs the card shows progress. When it finishes
the card fetches the affected cells live from Google Sheets and shows them as
a table, with tab pills, refresh, and a link to open the sheet.

All Google access goes through `gsheets.py` (the `gog` CLI), so this process
never sees an OAuth token. Standard library only.
"""
from __future__ import annotations

import json
import os
import re
import sys
import threading
import time

import gsheets
from gsheets import SheetsError

APPLET_ID = os.environ.get("JCODE_APPLET_ID", "sheets")
SCHEMA = os.environ.get("JCODE_APPLET_SCHEMA", "jcode.applet/1")
SERVER = os.environ.get("JCODE_SHEETS_MCP_SERVER", "sheets")
TOOLS = ("info", "read", "write", "append", "clear", "create", "add_tab")
MAX_ROWS = 200
MAX_COLS = 12
MAX_CELL = 80
WINDOW = "A1:Z60"  # what "show tab" reads when the call named no cells

VERBS = {
    "info": "Looking at",
    "read": "Reading",
    "write": "Writing to",
    "append": "Adding rows to",
    "clear": "Clearing",
    "create": "Creating",
    "add_tab": "Adding a tab to",
}

_out_lock = threading.Lock()


def send(message: dict) -> None:
    with _out_lock:
        sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def log(text: str) -> None:
    sys.stderr.write(f"[applet {APPLET_ID}] {text}\n")
    sys.stderr.flush()


def manifest() -> dict:
    return {
        "schema": SCHEMA,
        "id": APPLET_ID,
        "title": "Google Sheets",
        "icon": "folder",
        "description": "Shows Google Sheets tool calls as live tables.",
        "tool_cards": [{"tool": f"mcp__{SERVER}__{name}"} for name in TOOLS] + [{"tool": "mcp_call"}],
        "capabilities": ["open_url"],
    }


# -- call parsing ---------------------------------------------------------------


def resolve(tool: str, raw_input) -> tuple[str, dict] | None:
    """(sheets tool, arguments) for a claimed call, or None when it isn't ours."""
    tool = tool.removeprefix("functions.")
    args = raw_input if isinstance(raw_input, dict) else {}
    if tool == "mcp_call":
        if args.get("server") != SERVER:
            return None
        name, inner = str(args.get("tool") or ""), args.get("arguments")
        if isinstance(inner, str):
            try:
                inner = json.loads(inner)
            except json.JSONDecodeError:
                inner = {}
        return (name, inner if isinstance(inner, dict) else {}) if name in TOOLS else None
    prefix = f"mcp__{SERVER}__"
    if tool.startswith(prefix) and tool[len(prefix):] in TOOLS:
        return tool[len(prefix):], args
    return None


_URL = re.compile(r"https://docs\.google\.com/spreadsheets/d/[A-Za-z0-9_-]+\S*")
_UPDATED = re.compile(r"(?:Updated|Appended \d+ rows at|Cleared) (\S+)")


def summary_line(output: str) -> str:
    for line in (output or "").splitlines()[1:]:
        line = line.strip()
        if line:
            return line
    return ""


def affected_range(output: str) -> str | None:
    match = _UPDATED.search(output or "")
    return match.group(1).rstrip(".:") if match else None


def spreadsheet_from(args: dict, output: str) -> str | None:
    for candidate in (args.get("spreadsheet"), *(_URL.findall(output or ""))):
        if candidate:
            try:
                return gsheets.spreadsheet_id(str(candidate))
            except SheetsError:
                continue
    return None


# -- cells ----------------------------------------------------------------------

_CELL = re.compile(r"^\$?([A-Za-z]{1,3})\$?(\d*)$")


def col_index(letters: str) -> int:
    n = 0
    for ch in letters.upper():
        n = n * 26 + (ord(ch) - 64)
    return n - 1


def col_letters(index: int) -> str:
    out = ""
    index += 1
    while index:
        index, rem = divmod(index - 1, 26)
        out = chr(65 + rem) + out
    return out


def origin(cells: str | None) -> tuple[int, int]:
    """Zero-based (row, col) of a range's top-left cell."""
    first = (cells or "A1").split(":")[0]
    match = _CELL.match(first)
    if not match:
        return 0, 0
    row = int(match.group(2)) - 1 if match.group(2) else 0
    return max(row, 0), col_index(match.group(1))


def clip(value) -> str:
    text = "" if value is None else str(value)
    text = " ".join(text.split())
    return text if len(text) <= MAX_CELL else text[: MAX_CELL - 1] + "…"


def table(values: list, first_row: int, first_col: int, header: list | None) -> dict:
    """A table node: a row-number column plus up to MAX_COLS columns."""
    width = min(max((len(r) for r in values), default=0), MAX_COLS)
    if header is not None:
        width = min(max(width, len(header)), MAX_COLS)
    width = max(width, 1)
    if header is not None:
        columns = [clip(header[i]) if i < len(header) and str(header[i]).strip() else col_letters(first_col + i) for i in range(width)]
        body, start = values, first_row
    else:
        columns = [col_letters(first_col + i) for i in range(width)]
        body, start = values, first_row
    rows = []
    for offset, row in enumerate(body[:MAX_ROWS]):
        cells = [clip(row[i]) if i < len(row) else "" for i in range(width)]
        rows.append([str(start + offset + 1)] + cells)
    return {"type": "table", "columns": ["#"] + columns, "rows": rows}


# -- cards ----------------------------------------------------------------------


class Card:
    """One tool call's card."""

    def __init__(self, provider: "Provider", session_id: str, call_id: str) -> None:
        self.provider = provider
        self.session_id = session_id
        self.call_id = call_id
        self.instance = f"{APPLET_ID}#{call_id}"
        self.revision = 0
        self.lock = threading.Lock()
        self.name = ""
        self.args: dict = {}
        self.output = ""
        self.error: str | None = None
        self.done = False
        self.sid: str | None = None
        self.meta: dict | None = None
        self.tab: str | None = None
        self.cells: str | None = None
        self.values: list | None = None
        self.header: list | None = None
        self.load_error: str | None = None
        self.loading = False
        self.loaded_at = 0.0

    # -- state

    def update(self, name: str, args: dict, output: str | None, error: str | None, done: bool) -> None:
        with self.lock:
            self.name, self.args = name, args
            self.output = output or ""
            self.error = error
            self.done = done
            self.sid = spreadsheet_from(args, self.output)
            tab, cells = gsheets.split_range(str(args.get("range") or ""))
            if name in ("write", "append", "clear"):
                touched = affected_range(self.output)
                if touched:
                    tab, cells = gsheets.split_range(touched)
                    tab = tab or gsheets.split_range(str(args.get("range") or ""))[0]
            if name == "add_tab":
                tab, cells = str(args.get("name") or "") or None, None
            self.tab, self.cells = tab, cells
        self.mount()
        if done and not error and self.sid and not self.output.startswith("Sheets error"):
            self.load()

    def load(self, tab: str | None = None) -> None:
        with self.lock:
            if tab is not None:
                self.tab, self.cells, self.name = tab, None, "read"
                self.values, self.header = None, None
            self.loading = True
            self.load_error = None
        self.mount()
        self.provider.spawn(self._load)

    def _load(self) -> None:
        with self.lock:
            sid, tab, cells, name = self.sid, self.tab, self.cells, self.name
        try:
            meta = gsheets.info(sid)
            tabs = [t["title"] for t in meta["tabs"]]
            if not tab or tab not in tabs:
                tab = tabs[0] if tabs else "Sheet1"
            quoted = gsheets.quote_tab(tab)
            if name in ("write", "append") and cells:
                values = gsheets.read(sid, f"{quoted}!{cells}")["values"]
                row, _ = origin(cells)
                header = gsheets.read(sid, f"{quoted}!1:1")["values"] if row > 0 else None
                header = header[0] if header else None
            else:
                values = gsheets.read(sid, f"{quoted}!{cells or WINDOW}")["values"]
                header = None
            with self.lock:
                self.meta, self.tab, self.values, self.header = meta, tab, values, header
                self.loading, self.loaded_at = False, time.time()
        except Exception as error:  # noqa: BLE001
            with self.lock:
                self.loading, self.load_error = False, str(error)
        self.mount()

    # -- rendering

    def title(self) -> str:
        if self.meta and self.meta.get("title"):
            return self.meta["title"]
        if self.name == "create":
            return str(self.args.get("title") or "New spreadsheet")
        return "Google Sheets"

    def view(self) -> dict:
        with self.lock:
            return self._view()

    def _view(self) -> dict:
        children: list[dict] = []
        verb = VERBS.get(self.name, "Using")
        target = self.tab or ("spreadsheet" if self.name != "create" else self.title())
        if not self.done:
            children.append({"type": "progress", "label": f"{verb} {target}…"})
            return self._card(children)
        failure = self.error or (self.output if self.output.startswith("Sheets error") else None)
        if failure:
            children.append({"type": "error", "message": failure.removeprefix("Sheets error: ")})
            return self._card(children)

        summary = summary_line(self.output)
        header_row: list[dict] = []
        if summary and self.name != "read":
            header_row.append({"type": "chip", "label": summary, "tone": "success"})
        if self.name == "read" and self.values is not None:
            header_row.append({"type": "chip", "label": f"{len(self.values)} rows", "tone": "dim"})
        if header_row:
            children.append({"type": "stack", "direction": "horizontal", "gap": "sm", "children": header_row})

        tabs = [t["title"] for t in (self.meta or {}).get("tabs", [])]
        if len(tabs) > 1:
            children.append(
                {
                    "type": "stack",
                    "direction": "horizontal",
                    "gap": "xs",
                    "children": [
                        {
                            "type": "chip",
                            "label": name,
                            "tone": "accent" if name == self.tab else "default",
                            "on_press": {"action": "tab", "args": {"tab": name}},
                        }
                        for name in tabs[:12]
                    ],
                }
            )

        if self.load_error:
            children.append({"type": "error", "message": self.load_error, "retry": {"action": "refresh"}})
        elif self.values is None:
            children.append({"type": "progress", "label": "Loading cells…"})
        elif not self.values:
            children.append({"type": "empty", "title": "No values here yet"})
        else:
            row, col = origin(self.cells if self.name in ("write", "append") else (self.cells or WINDOW))
            children.append(
                {
                    "type": "scroll",
                    "max_height": 360,
                    "children": [table(self.values, row, col, self.header)],
                }
            )
            hidden_rows = len(self.values) - MAX_ROWS
            hidden_cols = max((len(r) for r in self.values), default=0) - MAX_COLS
            notes = []
            if hidden_rows > 0:
                notes.append(f"{hidden_rows} more rows")
            if hidden_cols > 0:
                notes.append(f"{hidden_cols} more columns")
            if notes:
                children.append({"type": "text", "text": " and ".join(notes) + " in the sheet", "style": "caption", "tone": "dim"})

        buttons = []
        if self.sid:
            gid = next((t["gid"] for t in (self.meta or {}).get("tabs", []) if t["title"] == self.tab), None)
            buttons.append(
                {
                    "type": "button",
                    "label": "Open in Sheets",
                    "variant": "secondary",
                    "icon": "link",
                    "on_press": {"action": "host.open_url", "args": {"url": gsheets.sheet_url(self.sid, gid)}},
                }
            )
            buttons.append(
                {
                    "type": "button",
                    "label": "Refreshing…" if self.loading else "Refresh",
                    "variant": "compact",
                    "disabled": self.loading,
                    "on_press": {"action": "refresh"},
                }
            )
        if buttons:
            children.append({"type": "stack", "direction": "horizontal", "gap": "sm", "children": buttons})
        return self._card(children)

    def _card(self, children: list) -> dict:
        subtitle = self.tab if self.tab and self.done else None
        title = self.title() + (f" · {subtitle}" if subtitle else "")
        return {"type": "card", "title": title, "children": children}

    def mount(self) -> None:
        with self.lock:
            self.revision += 1
            revision = self.revision
        send(
            {
                "type": "mount",
                "instance": self.instance,
                "placement": {
                    "kind": "inline",
                    "session_id": self.session_id,
                    "anchor": {"kind": "tool_call", "call_id": self.call_id},
                },
                "lifetime": "persistent",
                "document": {"revision": revision, "title": self.title(), "view": self.view(), "state": {}},
            }
        )

    def on_action(self, action: dict) -> None:
        name = action.get("action")
        args = action.get("args") or {}
        if name == "refresh" and self.sid:
            self.load()
        elif name == "tab" and self.sid and args.get("tab"):
            self.load(str(args["tab"]))


class Provider:
    def __init__(self) -> None:
        self.cards: dict[str, Card] = {}
        self.lock = threading.Lock()

    def spawn(self, target) -> None:
        threading.Thread(target=target, daemon=True).start()

    def card(self, instance: str) -> Card | None:
        with self.lock:
            return self.cards.get(instance)

    def tool_call(self, message: dict) -> None:
        resolved = resolve(str(message.get("tool") or ""), message.get("input"))
        if not resolved:
            return
        name, args = resolved
        session_id, call_id = str(message.get("session_id") or ""), str(message.get("call_id") or "")
        if not session_id or not call_id:
            return
        instance = f"{APPLET_ID}#{call_id}"
        with self.lock:
            card = self.cards.get(instance)
            if card is None:
                card = self.cards[instance] = Card(self, session_id, call_id)
        card.update(name, args, message.get("output"), message.get("error"), bool(message.get("done")))

    def handle(self, message: dict) -> None:
        kind = message.get("type")
        if kind == "tool_call":
            self.tool_call(message)
        elif kind == "action":
            card = self.card(str(message.get("instance") or ""))
            if card:
                card.on_action(message.get("action") or {})
        elif kind == "resync":
            card = self.card(str(message.get("instance") or ""))
            if card:
                card.mount()
        elif kind == "closed":
            with self.lock:
                self.cards.pop(str(message.get("instance") or ""), None)
        elif kind == "rejected":
            log(f"host rejected a message: {message.get('reason')}")

    def run(self) -> None:
        send({"type": "register", "manifest": manifest()})
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                log(f"bad host line: {line[:200]}")
                continue
            try:
                self.handle(message)
            except Exception as error:  # noqa: BLE001
                log(f"error handling {message.get('type')}: {error}")


if __name__ == "__main__":
    Provider().run()
