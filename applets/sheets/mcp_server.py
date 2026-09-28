#!/usr/bin/env python3
"""Google Sheets MCP server (stdio, JSON-RPC 2.0, one message per line).

Register it in ~/.jcode/mcp.json as server "sheets". Tools then appear to the
agent as mcp__sheets__<tool>, and the Sheets applet renders each call as a
live table card in Jcode Desktop. Standard library only.
"""
from __future__ import annotations

import json
import sys
import threading

import gsheets
from gsheets import SheetsError

PROTOCOL = "2025-06-18"
PREVIEW_ROWS = 40

SID = {"type": "string", "description": "Spreadsheet id or full Google Sheets URL."}
RANGE = {"type": "string", "description": "A1 range such as 'Contacts!A1:L50', or a tab name such as 'Contacts'."}
VALUES = {
    "type": "array",
    "description": "Rows of cell values. Strings starting with '=' are formulas unless raw is true.",
    "items": {"type": "array", "items": {}},
}
RAW = {"type": "boolean", "description": "Store values literally instead of parsing them like typed input."}

TOOLS = [
    {
        "name": "info",
        "description": "Spreadsheet title, URL and tabs with their sizes.",
        "inputSchema": {"type": "object", "properties": {"spreadsheet": SID}, "required": ["spreadsheet"]},
    },
    {
        "name": "read",
        "description": "Read cell values from a range or whole tab. The user sees the result as a table.",
        "inputSchema": {
            "type": "object",
            "properties": {"spreadsheet": SID, "range": RANGE},
            "required": ["spreadsheet", "range"],
        },
    },
    {
        "name": "write",
        "description": "Overwrite cells starting at the range's top-left cell.",
        "inputSchema": {
            "type": "object",
            "properties": {"spreadsheet": SID, "range": RANGE, "values": VALUES, "raw": RAW},
            "required": ["spreadsheet", "range", "values"],
        },
    },
    {
        "name": "append",
        "description": "Append rows after the last row of data in a tab.",
        "inputSchema": {
            "type": "object",
            "properties": {"spreadsheet": SID, "range": RANGE, "values": VALUES, "raw": RAW},
            "required": ["spreadsheet", "range", "values"],
        },
    },
    {
        "name": "clear",
        "description": "Clear values in a range (formatting is kept).",
        "inputSchema": {
            "type": "object",
            "properties": {"spreadsheet": SID, "range": RANGE},
            "required": ["spreadsheet", "range"],
        },
    },
    {
        "name": "create",
        "description": "Create a new spreadsheet in the user's Google Drive.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "tabs": {"type": "array", "items": {"type": "string"}, "description": "Tab names to create."},
            },
            "required": ["title"],
        },
    },
    {
        "name": "add_tab",
        "description": "Add a tab to an existing spreadsheet.",
        "inputSchema": {
            "type": "object",
            "properties": {"spreadsheet": SID, "name": {"type": "string"}},
            "required": ["spreadsheet", "name"],
        },
    },
]

_out = threading.Lock()


def send(message: dict) -> None:
    with _out:
        sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def cell(value) -> str:
    text = "" if value is None else str(value)
    text = text.replace("\t", " ").replace("\n", " ")
    return text if len(text) <= 120 else text[:117] + "..."


def preview(values: list, limit: int = PREVIEW_ROWS) -> str:
    """Rows as tab-separated text for the model."""
    if not values:
        return "(no values)"
    lines = ["\t".join(cell(v) for v in row) for row in values[:limit]]
    if len(values) > limit:
        lines.append(f"... {len(values) - limit} more rows (read a narrower range to see them)")
    return "\n".join(lines)


def header(sid: str) -> str:
    return f"Spreadsheet: {gsheets.sheet_url(sid)}"


def values_arg(args: dict) -> list:
    values = args.get("values")
    if not isinstance(values, list) or not all(isinstance(row, list) for row in values):
        raise SheetsError("values must be an array of rows, each an array of cells")
    return values


def call(name: str, args: dict) -> str:
    if name == "create":
        made = gsheets.create(str(args.get("title") or "Untitled"), args.get("tabs") or None)
        return f"{header(made['spreadsheet_id'])}\nCreated \"{made['title']}\" (id {made['spreadsheet_id']})."

    sid = gsheets.spreadsheet_id(str(args.get("spreadsheet") or ""))
    if name == "info":
        data = gsheets.info(sid)
        tabs = "\n".join(
            f"- {t['title']} (gid {t['gid']}, {t['rows']} rows x {t['columns']} columns)" for t in data["tabs"]
        )
        return f"{header(sid)}\nTitle: {data['title']}\nTabs:\n{tabs}"
    if name == "add_tab":
        made = gsheets.add_tab(sid, str(args.get("name") or "Sheet"))
        return f"{header(sid)}\nAdded tab \"{made['tab']}\" (gid {made['gid']})."

    a1 = str(args.get("range") or "").strip()
    if not a1:
        raise SheetsError("range is required")
    if name == "read":
        data = gsheets.read(sid, a1)
        rows = data["values"]
        return f"{header(sid)}\nRange: {data['range']} ({len(rows)} rows)\n\n{preview(rows)}"
    if name == "write":
        done = gsheets.write(sid, a1, values_arg(args), bool(args.get("raw")))
        return f"{header(sid)}\nUpdated {done.get('updatedRange', a1)}: {done.get('updatedCells', 0)} cells."
    if name == "append":
        done = gsheets.append(sid, a1, values_arg(args), bool(args.get("raw")))
        return (
            f"{header(sid)}\nAppended {done.get('updatedRows', 0)} rows at {done.get('updatedRange', a1)}."
        )
    if name == "clear":
        done = gsheets.clear(sid, a1)
        return f"{header(sid)}\nCleared {done.get('clearedRange', a1)}."
    raise SheetsError(f"Unknown tool: {name}")


def handle(message: dict) -> None:
    method = message.get("method")
    mid = message.get("id")
    if mid is None:
        return  # notification (initialized, cancelled, ...)
    if method == "initialize":
        result = {
            "protocolVersion": message.get("params", {}).get("protocolVersion") or PROTOCOL,
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "jcode-sheets", "version": "1.0.0"},
            "instructions": "Read and edit the user's Google Sheets. Jcode Desktop shows each call as a table.",
        }
    elif method == "ping":
        result = {}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        params = message.get("params") or {}
        try:
            text = call(str(params.get("name")), params.get("arguments") or {})
            result = {"content": [{"type": "text", "text": text}]}
        except SheetsError as error:
            result = {"content": [{"type": "text", "text": f"Sheets error: {error}"}], "isError": True}
        except Exception as error:  # noqa: BLE001
            result = {"content": [{"type": "text", "text": f"Sheets error: {error}"}], "isError": True}
    else:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": f"Unknown method {method}"}})
        return
    send({"jsonrpc": "2.0", "id": mid, "result": result})


def main() -> None:
    workers: list[threading.Thread] = []
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            send({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "Parse error"}})
            continue
        # Calls can be slow, so answer them concurrently.
        worker = threading.Thread(target=handle, args=(message,), daemon=True)
        worker.start()
        workers = [w for w in workers if w.is_alive()] + [worker]
    # Finish in-flight calls before exiting on EOF.
    for worker in workers:
        worker.join(gsheets.TIMEOUT + 5)


if __name__ == "__main__":
    main()
