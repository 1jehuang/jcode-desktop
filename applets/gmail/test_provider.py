#!/usr/bin/env python3
"""Offline test for the Gmail applet provider.

Runs provider.py against a fake Gmail API and token endpoint, drives it over
stdio like Desktop does, and checks the documents and API calls.
Run: python3 applets/gmail/test_provider.py
"""
from __future__ import annotations

import base64
import http.server
import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CALLS: list[str] = []
BODIES: list[dict] = []


def b64(text: str) -> str:
    return base64.urlsafe_b64encode(text.encode()).decode().rstrip("=")


def meta(mid: str, subject: str, labels: list[str]) -> dict:
    return {"id": mid, "threadId": f"t{mid}", "labelIds": labels, "snippet": f"About {subject} &amp; more",
            "internalDate": str(int(time.time() * 1000)),
            "payload": {"headers": [{"name": "From", "value": "Ana Lee <ana@example.com>"},
                                    {"name": "Subject", "value": subject},
                                    {"name": "Date", "value": "Thu, 25 Sep 2026 10:00:00 -0700"}]}}


class Fake(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args) -> None:  # quiet
        pass

    def reply(self, body: dict, code: int = 200) -> None:
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        CALLS.append(f"GET {self.path}")
        assert self.headers["Authorization"] == "Bearer fresh-token", self.headers["Authorization"]
        if self.path.startswith("/gmail/messages?"):
            self.reply({"messages": [{"id": "m1"}, {"id": "m2"}]})
        elif self.path.startswith("/gmail/messages/m1?") and "format=full" in self.path:
            msg = meta("m1", "Launch plan", ["INBOX", "UNREAD", "IMPORTANT"])
            msg["payload"].update({"mimeType": "multipart/alternative", "parts": [
                {"mimeType": "text/html", "body": {"data": b64("<p>Hello <b>there</b></p>")}},
                {"mimeType": "text/plain", "body": {"data": b64("Hello there, plain body.")}}]})
            msg["payload"]["headers"].append({"name": "Message-ID", "value": "<abc@example.com>"})
            self.reply(msg)
        elif self.path.startswith("/gmail/messages/m1?"):
            self.reply(meta("m1", "Launch plan", ["INBOX", "UNREAD", "IMPORTANT"]))
        elif self.path.startswith("/gmail/messages/m2?"):
            self.reply(meta("m2", "Receipt", ["INBOX", "CATEGORY_UPDATES"]))
        else:
            self.reply({"error": {"message": "not found"}}, 404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        CALLS.append(f"POST {self.path}")
        if self.path == "/token":
            self.reply({"access_token": "fresh-token", "expires_in": 3600})
            return
        BODIES.append(json.loads(raw or b"{}"))
        self.reply({"id": "ok"})


def main() -> None:
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fake)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}"
    home = Path(tempfile.mkdtemp(prefix="gmail-applet-"))
    # Expired token: the provider must refresh before calling the API.
    (home / "google_oauth.json").write_text(json.dumps({
        "access_token": "stale", "refresh_token": "r1", "expires_at": 0, "tier": "full", "email": "me@example.com"}))
    (home / "google_credentials.json").write_text(json.dumps({"installed": {"client_id": "c", "client_secret": "s"}}))
    env = {**os.environ, "JCODE_HOME": str(home), "JCODE_GMAIL_API": f"{base}/gmail",
           "JCODE_GOOGLE_TOKEN_URL": f"{base}/token"}
    proc = subprocess.Popen([sys.executable, str(HERE / "provider.py")], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, text=True, env=env)
    lines: queue.Queue = queue.Queue()
    threading.Thread(target=lambda: [lines.put(json.loads(l)) for l in proc.stdout], daemon=True).start()
    docs: list[dict] = []

    def send(message: dict) -> None:
        proc.stdin.write(json.dumps(message) + "\n")
        proc.stdin.flush()

    def wait(what: str, check) -> dict:
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                message = lines.get(timeout=0.2)
            except queue.Empty:
                continue
            if message["type"] == "mount":
                docs.append(message["document"])
            if check(message):
                return message
        raise SystemExit(f"timed out waiting for {what}")

    def latest() -> str:
        return json.dumps(docs[-1]["view"]) if docs else ""

    register = wait("register", lambda m: m["type"] == "register")
    manifest = register["manifest"]
    assert manifest["id"] == "gmail" and manifest["icon"] == "mail", manifest
    assert {l["trigger"] for l in manifest["launchers"]} == {"sidebar", "command"}

    send({"type": "launch", "instance": "g1", "launcher": 0})
    wait("inbox rows", lambda m: m["type"] == "mount" and "Receipt" in json.dumps(m))
    inbox = latest()
    for expected in ["Launch plan", "Ana Lee", "About Launch plan & more", "unread", "updates", "Inbox 1"]:
        assert expected in inbox, f"inbox lacks {expected}"
    saved = json.loads((home / "google_oauth.json").read_text())
    assert saved["access_token"] == "fresh-token" and saved["refresh_token"] == "r1", saved
    assert (home / "google_oauth.json").stat().st_mode & 0o777 == 0o600

    send({"type": "action", "instance": "g1", "action": {"action": "open", "args": {"id": "m1"}}, "state": {}})
    wait("detail", lambda m: m["type"] == "mount" and "plain body" in json.dumps(m))
    detail = latest()
    for expected in ["Open in Gmail", "host.start_chat", "Archive", "Save reply draft", "important"]:
        assert expected in detail, f"detail lacks {expected}"
    assert any(c.startswith("POST /gmail/messages/m1/modify") for c in CALLS), "opening marks read"
    assert BODIES[-1] == {"removeLabelIds": ["UNREAD"]}, BODIES

    send({"type": "action", "instance": "g1", "action": {"action": "draft_reply", "args": {"id": "m1"}},
          "state": {"reply": "Sounds good"}})
    wait("draft toast", lambda m: m["type"] == "toast" and "Drafts" in m["text"])
    draft = BODIES[-1]["message"]
    raw = base64.urlsafe_b64decode(draft["raw"] + "==").decode()
    assert draft["threadId"] == "tm1"
    for expected in ["To: Ana Lee <ana@example.com>", "Subject: Re: Launch plan",
                     "In-Reply-To: <abc@example.com>", "Sounds good"]:
        assert expected in raw, f"draft lacks {expected}: {raw}"

    send({"type": "action", "instance": "g1", "action": {"action": "trash", "args": {"id": "m1"}}, "state": {}})
    time.sleep(0.5)
    assert not any("/trash" in c for c in CALLS), "delete needs confirmation first"
    send({"type": "action", "instance": "g1", "action": {"action": "ask_trash", "args": {"id": "m1"}}, "state": {}})
    wait("confirm", lambda m: m["type"] == "mount" and "Confirm delete" in json.dumps(m))
    send({"type": "action", "instance": "g1", "action": {"action": "trash", "args": {"id": "m1"}}, "state": {}})
    wait("trash toast", lambda m: m["type"] == "toast" and "Trash" in m["text"])
    assert "POST /gmail/messages/m1/trash" in CALLS

    proc.stdin.close()
    proc.wait(timeout=5)
    server.shutdown()
    dump = os.environ.get("GMAIL_APPLET_DUMP")
    if dump:
        Path(dump).write_text(json.dumps({"manifest": manifest, "documents": docs}))
    print(f"ok: {len(docs)} documents, {len(CALLS)} API calls")


if __name__ == "__main__":
    main()
