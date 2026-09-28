#!/usr/bin/env python3
"""Offline tests for the Sheets MCP server and applet provider.

A fake `gog` stands in for Google, backed by a JSON file, so nothing touches a
real spreadsheet. Run: python3 applets/sheets/test_sheets.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SID = "1AbCdEfGhIjKlMnOpQrStUvWxYz0123456789"

FAKE_GOG = textwrap.dedent(
    r'''
    #!/usr/bin/env python3
    import json, os, re, sys
    store = os.environ["FAKE_SHEETS"]
    data = json.load(open(store))
    args = [a for a in sys.argv[1:] if a not in ("--json", "--no-input")]
    assert args[0] == "sheets", args
    cmd, rest = args[1], args[2:]
    def opt(name):
        return rest[rest.index(name) + 1] if name in rest else None
    def tab_of(a1):
        tab = a1.split("!")[0] if "!" in a1 else "Contacts"
        return tab.strip("'")
    def save():
        json.dump(data, open(store, "w"))
    if data.get("fail"):
        print("googleapi: Error 403: The caller does not have permission", file=sys.stderr); sys.exit(1)
    if cmd == "metadata":
        print(json.dumps({"title": data["title"], "sheets": [
            {"properties": {"title": t, "sheetId": i, "gridProperties": {"rowCount": 1000, "columnCount": 26}}}
            for i, t in enumerate(data["tabs"])]}))
    elif cmd == "get":
        a1 = rest[1]
        rows = data["tabs"][tab_of(a1)]
        cells = a1.split("!")[1] if "!" in a1 else ""
        m = re.match(r"[A-Z]+(\d+)(?::[A-Z]+(\d+))?$", cells)
        if cells == "1:1":
            rows = rows[:1]
        elif m and m.group(2):
            rows = rows[int(m.group(1)) - 1:int(m.group(2))]
        print(json.dumps({"range": a1, "values": rows}))
    elif cmd == "update":
        a1, values = rest[1], json.loads(opt("--values-json"))
        rows = data["tabs"][tab_of(a1)]
        start = int(re.search(r"(\d+)", a1.split("!")[-1]).group(1)) - 1
        for i, row in enumerate(values):
            while len(rows) <= start + i: rows.append([])
            rows[start + i] = row
        save()
        end = start + len(values)
        print(json.dumps({"updatedRange": f"{tab_of(a1)}!A{start+1}:L{end}", "updatedCells": sum(map(len, values)), "updatedRows": len(values)}))
    elif cmd == "append":
        a1, values = rest[1], json.loads(opt("--values-json"))
        rows = data["tabs"][tab_of(a1)]
        start = len(rows)
        rows.extend(values); save()
        print(json.dumps({"updatedRange": f"{tab_of(a1)}!A{start+1}:L{start+len(values)}", "updatedRows": len(values), "updatedCells": sum(map(len, values))}))
    elif cmd == "clear":
        print(json.dumps({"clearedRange": rest[1]}))
    elif cmd == "create":
        print(json.dumps({"spreadsheetId": "NEWSHEET1234567890", "spreadsheetUrl": "https://docs.google.com/spreadsheets/d/NEWSHEET1234567890/edit", "title": rest[0]}))
    elif cmd == "add-tab":
        data["tabs"][rest[1]] = []; save()
        print(json.dumps({"title": rest[1], "sheetId": 99}))
    else:
        print("unknown " + cmd, file=sys.stderr); sys.exit(2)
    '''
).lstrip()


class Env:
    def __init__(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        self.gog = root / "gog"
        self.gog.write_text(FAKE_GOG)
        self.gog.chmod(0o755)
        self.store = root / "store.json"
        self.reset()
        self.env = dict(
            os.environ,
            JCODE_SHEETS_GOG=str(self.gog),
            FAKE_SHEETS=str(self.store),
            JCODE_SHEETS_GOG_PASSWORD_FILE=str(root / "none"),
        )

    def reset(self, **extra) -> None:
        self.store.write_text(
            json.dumps(
                {
                    "title": "Jcode CRM",
                    "tabs": {
                        "Contacts": [
                            ["Company", "Name", "Stage"],
                            ["SUSE", "Rhys Oxenham", "Not contacted"],
                            ["Igalia", "Paulo Matos", "Not contacted"],
                        ],
                        "Accounts": [["Company", "Tier"], ["SUSE", "1"]],
                    },
                    **extra,
                }
            )
        )


class Proc:
    """A line-oriented subprocess."""

    def __init__(self, script: str, env: dict) -> None:
        self.p = subprocess.Popen(
            [sys.executable, str(HERE / script)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            cwd=HERE,
        )

    def send(self, message: dict) -> None:
        self.p.stdin.write(json.dumps(message) + "\n")
        self.p.stdin.flush()

    def recv(self, timeout: float = 10) -> dict:
        import selectors

        sel = selectors.DefaultSelector()
        sel.register(self.p.stdout, selectors.EVENT_READ)
        if not sel.select(timeout):
            raise TimeoutError("no message from " + str(self.p.args))
        return json.loads(self.p.stdout.readline())

    def close(self) -> None:
        self.p.stdin.close()
        self.p.wait(5)
        self.p.stdout.close()
        self.p.stderr.close()


def walk(node: dict):
    yield node
    for child in node.get("children", []):
        yield from walk(child)


class McpServerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.env = Env()
        self.mcp = Proc("mcp_server.py", self.env.env)

    def tearDown(self) -> None:
        self.mcp.close()
        self.env.dir.cleanup()

    def rpc(self, method: str, params: dict | None = None, mid: int = 1) -> dict:
        self.mcp.send({"jsonrpc": "2.0", "id": mid, "method": method, "params": params or {}})
        return self.mcp.recv()

    def call(self, tool: str, **arguments) -> dict:
        return self.rpc("tools/call", {"name": tool, "arguments": arguments})["result"]

    def test_handshake_and_tools(self) -> None:
        init = self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})
        self.assertEqual(init["result"]["serverInfo"]["name"], "jcode-sheets")
        self.mcp.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        names = [t["name"] for t in self.rpc("tools/list", mid=2)["result"]["tools"]]
        self.assertEqual(names, ["info", "read", "write", "append", "clear", "create", "add_tab"])

    def test_read_accepts_urls_and_previews_rows(self) -> None:
        result = self.call("read", spreadsheet=f"https://docs.google.com/spreadsheets/d/{SID}/edit#gid=0", range="Contacts")
        text = result["content"][0]["text"]
        self.assertNotIn("isError", result)
        self.assertTrue(text.startswith(f"Spreadsheet: https://docs.google.com/spreadsheets/d/{SID}/edit"))
        self.assertIn("SUSE\tRhys Oxenham\tNot contacted", text)
        self.assertIn("(3 rows)", text)

    def test_write_append_and_errors(self) -> None:
        text = self.call("append", spreadsheet=SID, range="Contacts", values=[["Arm", "Rob Moran", "New"]])["content"][0]["text"]
        self.assertIn("Appended 1 rows at Contacts!A4:L4", text)
        text = self.call("write", spreadsheet=SID, range="Contacts!A2", values=[["SUSE", "Rhys", "Emailed"]])["content"][0]["text"]
        self.assertIn("Updated Contacts!A2:L2", text)
        bad = self.call("write", spreadsheet=SID, range="Contacts!A2", values="nope")
        self.assertTrue(bad["isError"])
        self.env.reset(fail=True)
        denied = self.call("read", spreadsheet=SID, range="Contacts")
        self.assertTrue(denied["isError"])
        self.assertIn("403", denied["content"][0]["text"])

    def test_info_create_add_tab(self) -> None:
        self.assertIn("- Contacts (gid 0", self.call("info", spreadsheet=SID)["content"][0]["text"])
        self.assertIn("NEWSHEET1234567890", self.call("create", title="CRM")["content"][0]["text"])
        self.assertIn('Added tab "Pipeline"', self.call("add_tab", spreadsheet=SID, name="Pipeline")["content"][0]["text"])


class ProviderTest(unittest.TestCase):
    def setUp(self) -> None:
        self.env = Env()
        self.app = Proc("provider.py", self.env.env)
        self.manifest = self.app.recv()["manifest"]

    def tearDown(self) -> None:
        self.app.close()
        self.env.dir.cleanup()

    def settle(self, predicate, limit: int = 12) -> dict:
        for _ in range(limit):
            message = self.app.recv()
            if message["type"] == "mount" and predicate(message):
                return message
        raise AssertionError("card never reached the expected state")

    def tool_call(self, tool: str, input: dict, output: str | None, done: bool, call_id: str = "c1") -> None:
        self.app.send(
            {"type": "tool_call", "session_id": "s1", "call_id": call_id, "tool": tool, "input": input, "output": output, "done": done}
        )

    def test_manifest_claims_sheets_tools(self) -> None:
        tools = {claim["tool"] for claim in self.manifest["tool_cards"]}
        self.assertIn("mcp__sheets__read", tools)
        self.assertIn("mcp__sheets__append", tools)
        self.assertIn("mcp_call", tools)
        self.assertEqual(self.manifest["capabilities"], ["open_url"])

    def test_read_card_shows_live_table(self) -> None:
        args = {"spreadsheet": SID, "range": "Contacts"}
        self.tool_call("mcp__sheets__read", args, None, False)
        running = self.app.recv()
        self.assertEqual(running["placement"]["anchor"], {"kind": "tool_call", "call_id": "c1"})
        self.assertEqual(running["placement"]["session_id"], "s1")
        self.assertEqual([n["type"] for n in walk(running["document"]["view"])][-1], "progress")

        self.tool_call("mcp__sheets__read", args, f"Spreadsheet: https://docs.google.com/spreadsheets/d/{SID}/edit\nRange: Contacts (3 rows)", True)
        done = self.settle(lambda m: any(n["type"] == "table" for n in walk(m["document"]["view"])))
        view = done["document"]["view"]
        table = next(n for n in walk(view) if n["type"] == "table")
        self.assertEqual(table["columns"], ["#", "A", "B", "C"])
        self.assertEqual(table["rows"][1], ["2", "SUSE", "Rhys Oxenham", "Not contacted"])
        self.assertTrue(all(len(r) == len(table["columns"]) for r in table["rows"]))
        self.assertEqual(view["title"], "Jcode CRM · Contacts")
        chips = [n["label"] for n in walk(view) if n["type"] == "chip"]
        self.assertIn("Accounts", chips)
        opens = [n for n in walk(view) if n.get("label") == "Open in Sheets"]
        self.assertEqual(opens[0]["on_press"]["args"]["url"], f"https://docs.google.com/spreadsheets/d/{SID}/edit#gid=0")
        revisions = done["document"]["revision"]
        self.assertGreater(revisions, running["document"]["revision"])

        self.app.send({"type": "action", "instance": done["instance"], "action": {"action": "tab", "args": {"tab": "Accounts"}}})
        switched = self.settle(lambda m: m["document"]["view"]["title"].endswith("Accounts") and any(n["type"] == "table" for n in walk(m["document"]["view"])))
        table = next(n for n in walk(switched["document"]["view"]) if n["type"] == "table")
        self.assertEqual(table["rows"][1], ["2", "SUSE", "1"])

    def test_append_card_shows_the_new_rows_under_the_header(self) -> None:
        subprocess.run([str(self.env.gog), "sheets", "append", SID, "Contacts", "--values-json", '[["Arm","Rob Moran","New"]]'], env=self.env.env, check=True, capture_output=True)
        output = f"Spreadsheet: https://docs.google.com/spreadsheets/d/{SID}/edit\nAppended 1 rows at Contacts!A4:L4."
        self.tool_call("mcp__sheets__append", {"spreadsheet": SID, "range": "Contacts", "values": [["Arm", "Rob Moran", "New"]]}, output, True)
        done = self.settle(lambda m: any(n["type"] == "table" for n in walk(m["document"]["view"])))
        view = done["document"]["view"]
        table = next(n for n in walk(view) if n["type"] == "table")
        self.assertEqual(table["columns"][:4], ["#", "Company", "Name", "Stage"])
        self.assertEqual(table["rows"], [["4", "Arm", "Rob Moran", "New"] + [""] * (len(table["columns"]) - 4)])
        self.assertIn("Appended 1 rows at Contacts!A4:L4.", [n["label"] for n in walk(view) if n["type"] == "chip"])

    def test_mcp_call_wrapper_and_foreign_calls(self) -> None:
        self.tool_call("mcp_call", {"server": "yc", "tool": "search", "arguments": {}}, "x", True, call_id="other")
        wrapped = {"server": "sheets", "tool": "read", "arguments": {"spreadsheet": SID, "range": "Accounts"}}
        self.tool_call("mcp_call", wrapped, f"Spreadsheet: https://docs.google.com/spreadsheets/d/{SID}/edit", True, call_id="c2")
        first = self.app.recv()
        self.assertEqual(first["placement"]["anchor"]["call_id"], "c2", "foreign MCP calls get no card")

    def test_failures_show_an_error(self) -> None:
        self.tool_call("mcp__sheets__read", {"spreadsheet": SID, "range": "Contacts"}, "Sheets error: googleapi: Error 403", True)
        view = self.app.recv()["document"]["view"]
        errors = [n for n in walk(view) if n["type"] == "error"]
        self.assertEqual(errors[0]["message"], "googleapi: Error 403")


if __name__ == "__main__":
    unittest.main(verbosity=2)
