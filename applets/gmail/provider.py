#!/usr/bin/env python3
"""Gmail inbox applet for Jcode Desktop.

Speaks `jcode.applet/1` newline-delimited JSON on stdio. Uses the Google login
Jcode already stores (`jcode login google`): `~/.jcode/google_oauth.json` for
tokens and `~/.jcode/google_credentials.json` for the OAuth client. Expired
access tokens are refreshed with the same client and written back in the same
format, so the CLI and Desktop keep working. Standard library only.
"""
from __future__ import annotations

import base64
import concurrent.futures
import datetime as dt
import email.utils
import html
import json
import os
import re
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from email.message import EmailMessage
from pathlib import Path

APPLET_ID = os.environ.get("JCODE_APPLET_ID", "gmail")
SCHEMA = os.environ.get("JCODE_APPLET_SCHEMA", "jcode.applet/1")
# Overridable so tests can point the provider at a local fake.
API = os.environ.get("JCODE_GMAIL_API", "https://gmail.googleapis.com/gmail/v1/users/me")
TOKEN_URL = os.environ.get("JCODE_GOOGLE_TOKEN_URL", "https://oauth2.googleapis.com/token")
REFRESH_SECONDS = 300
LIST_LIMIT = 30
NOT_SIGNED_IN = "Gmail is not connected. Run `jcode login google` in a terminal, then press Refresh."

# Inbox sections: id -> (label, Gmail search query).
SECTIONS = {
    "inbox": ("Inbox", "in:inbox"),
    "unread": ("Unread", "in:inbox is:unread"),
    "starred": ("Starred", "is:starred"),
    "important": ("Important", "in:inbox is:important"),
}

_out_lock = threading.Lock()
_token_lock = threading.Lock()


def send(message: dict) -> None:
    with _out_lock:
        sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def log(text: str) -> None:
    sys.stderr.write(f"[applet {APPLET_ID}] {text}\n")
    sys.stderr.flush()


# ---------------------------------------------------------------------------
# Auth and API
# ---------------------------------------------------------------------------


def jcode_dir() -> Path:
    home = os.environ.get("JCODE_HOME")
    return Path(home) if home else Path.home() / ".jcode"


def load_tokens() -> dict:
    try:
        return json.loads((jcode_dir() / "google_oauth.json").read_text())
    except (OSError, ValueError):
        raise RuntimeError(NOT_SIGNED_IN) from None


def load_client() -> tuple[str, str]:
    try:
        data = json.loads((jcode_dir() / "google_credentials.json").read_text())
    except (OSError, ValueError):
        raise RuntimeError(NOT_SIGNED_IN) from None
    inner = data.get("installed") or data.get("web") or data
    return inner["client_id"], inner["client_secret"]


def save_tokens(tokens: dict) -> None:
    path = jcode_dir() / "google_oauth.json"
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".google_oauth.")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as handle:
            json.dump(tokens, handle, indent=2)
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def access_token(force_refresh: bool = False) -> str:
    with _token_lock:
        tokens = load_tokens()
        now_ms = int(time.time() * 1000)
        if not force_refresh and tokens.get("expires_at", 0) > now_ms + 60_000:
            return tokens["access_token"]
        client_id, client_secret = load_client()
        body = urllib.parse.urlencode({
            "grant_type": "refresh_token",
            "client_id": client_id,
            "client_secret": client_secret,
            "refresh_token": tokens["refresh_token"],
        }).encode()
        try:
            with urllib.request.urlopen(urllib.request.Request(TOKEN_URL, data=body), timeout=20) as resp:
                fresh = json.load(resp)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"Google sign-in expired ({error.code}). Run `jcode login google` again.") from None
        tokens["access_token"] = fresh["access_token"]
        tokens["refresh_token"] = fresh.get("refresh_token") or tokens["refresh_token"]
        tokens["expires_at"] = now_ms + int(fresh.get("expires_in", 3600)) * 1000
        save_tokens(tokens)
        return tokens["access_token"]


def api(method: str, path: str, query: dict | None = None, body: dict | None = None) -> dict:
    url = f"{API}/{path}"
    if query:
        url += "?" + urllib.parse.urlencode(query, doseq=True)
    data = json.dumps(body).encode() if body is not None else None
    for attempt in range(2):
        request = urllib.request.Request(url, data=data, method=method)
        request.add_header("Authorization", f"Bearer {access_token(force_refresh=attempt > 0)}")
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=30) as resp:
                raw = resp.read()
                return json.loads(raw) if raw else {}
        except urllib.error.HTTPError as error:
            if error.code == 401 and attempt == 0:
                continue
            detail = ""
            try:
                detail = json.loads(error.read()).get("error", {}).get("message", "")
            except Exception:  # noqa: BLE001 - best effort message
                pass
            if error.code == 403 and "scope" in detail.lower():
                detail = "Your Google login does not allow this. Run `jcode login google` with full access."
            raise RuntimeError(detail or f"Gmail returned HTTP {error.code}") from None
        except urllib.error.URLError as error:
            raise RuntimeError(f"Could not reach Gmail: {error.reason}") from None
    raise RuntimeError("Gmail rejected the login. Run `jcode login google` again.")


def can_modify() -> bool:
    try:
        return load_tokens().get("tier") == "full"
    except RuntimeError:
        return False


def header(message: dict, name: str) -> str:
    for item in (message.get("payload") or {}).get("headers") or []:
        if item.get("name", "").lower() == name.lower():
            return item.get("value", "")
    return ""


def decode_part(data: str) -> str:
    padded = data + "=" * (-len(data) % 4)
    return base64.urlsafe_b64decode(padded).decode("utf-8", errors="replace")


def html_to_text(markup: str) -> str:
    markup = re.sub(r"(?is)<(script|style|head)[^>]*>.*?</\1>", "", markup)
    markup = re.sub(r"(?i)<br\s*/?>|</p>|</div>|</tr>|</h[1-6]>|</li>", "\n", markup)
    markup = re.sub(r"<[^>]+>", "", markup)
    text = html.unescape(markup)
    text = re.sub(r"[ \t\r\f\v]+", " ", text)
    return re.sub(r"\n\s*\n\s*\n+", "\n\n", text).strip()


def body_text(payload: dict) -> str:
    """Prefer text/plain, fall back to stripped text/html."""
    plain: list[str] = []
    rich: list[str] = []

    def walk(part: dict) -> None:
        mime = part.get("mimeType", "")
        data = (part.get("body") or {}).get("data")
        if data and mime == "text/plain":
            plain.append(decode_part(data))
        elif data and mime == "text/html":
            rich.append(decode_part(data))
        for child in part.get("parts") or []:
            walk(child)

    walk(payload)
    if plain:
        return "\n".join(plain).strip()
    if rich:
        return html_to_text("\n".join(rich))
    return ""


def summarize(message: dict) -> dict:
    labels = message.get("labelIds") or []
    category = next((l.removeprefix("CATEGORY_").capitalize() for l in labels if l.startswith("CATEGORY_")), "")
    return {
        "id": message["id"],
        "thread": message.get("threadId", message["id"]),
        "from": header(message, "From") or "Unknown sender",
        "subject": header(message, "Subject") or "(no subject)",
        "date": header(message, "Date"),
        "internal": int(message.get("internalDate") or 0),
        "snippet": html.unescape(message.get("snippet") or ""),
        "unread": "UNREAD" in labels,
        "starred": "STARRED" in labels,
        "important": "IMPORTANT" in labels,
        "inbox": "INBOX" in labels,
        "category": "" if category in ("Personal", "Primary") else category,
    }


def list_messages(query: str) -> list[dict]:
    listing = api("GET", "messages", {"q": query, "maxResults": LIST_LIMIT})
    ids = [item["id"] for item in listing.get("messages") or []]

    def fetch(message_id: str) -> dict:
        return api("GET", f"messages/{message_id}", {
            "format": "metadata", "metadataHeaders": ["From", "Subject", "Date"]})

    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        return [summarize(message) for message in pool.map(fetch, ids)]


def fetch_detail(message_id: str) -> dict:
    message = api("GET", f"messages/{message_id}", {"format": "full"})
    detail = summarize(message)
    detail.update({
        "to": header(message, "To"),
        "cc": header(message, "Cc"),
        "reply_to": header(message, "Reply-To"),
        "message_id": header(message, "Message-ID"),
        "references": header(message, "References"),
        "body": body_text(message.get("payload") or {}) or detail["snippet"],
    })
    return detail


def create_reply_draft(detail: dict, text_body: str) -> None:
    reply = EmailMessage()
    reply["To"] = detail.get("reply_to") or detail["from"]
    subject = detail["subject"]
    reply["Subject"] = subject if subject.lower().startswith("re:") else f"Re: {subject}"
    if detail.get("message_id"):
        reply["In-Reply-To"] = detail["message_id"]
        reply["References"] = " ".join(filter(None, [detail.get("references"), detail["message_id"]]))
    reply.set_content(text_body)
    raw = base64.urlsafe_b64encode(reply.as_bytes()).decode()
    api("POST", "drafts", body={"message": {"raw": raw, "threadId": detail["thread"]}})


# ---------------------------------------------------------------------------
# View building
# ---------------------------------------------------------------------------


def text(value: str, style: str = "body", tone: str = "default", max_lines: int | None = None) -> dict:
    node = {"type": "text", "text": value, "style": style, "tone": tone}
    if max_lines:
        node["max_lines"] = max_lines
    return node


def hstack(*children: dict, gap: str = "xs") -> dict:
    return {"type": "stack", "direction": "horizontal", "gap": gap, "align": "center", "children": list(children)}


def button(label: str, action: str, variant: str = "secondary", icon: str | None = None, **args) -> dict:
    node = {"type": "button", "label": label, "variant": variant, "on_press": {"action": action, "args": args}}
    if icon:
        node["icon"] = icon
    return node


def sender_name(value: str) -> str:
    name, address = email.utils.parseaddr(value)
    return name or address or value


def when(item: dict) -> str:
    stamp = item.get("internal") or 0
    if not stamp:
        return ""
    moment = dt.datetime.fromtimestamp(stamp / 1000)
    now = dt.datetime.now()
    if moment.date() == now.date():
        return moment.strftime("%-I:%M %p")
    if moment.year == now.year:
        return moment.strftime("%b %-d")
    return moment.strftime("%b %-d, %Y")


def gmail_url(thread: str) -> str:
    return f"https://mail.google.com/mail/u/0/#all/{thread}"


def message_row(item: dict, section: str) -> dict:
    badges = []
    if item["unread"]:
        badges.append("unread")
    if item["starred"]:
        badges.append("★")
    if item["category"]:
        badges.append(item["category"].lower())
    return {
        "type": "list_item",
        "key": f"{section}:{item['id']}",
        "title": item["subject"],
        "subtitle": f"{sender_name(item['from'])} · {item['snippet']}"[:200],
        "meta": when(item),
        "leading": {"type": "icon", "name": "mail", "tone": "accent" if item["unread"] else "dim"},
        "badges": badges,
        "emphasized": item["unread"],
        "on_press": {"action": "open", "args": {"id": item["id"]}},
    }


class App:
    def __init__(self) -> None:
        self.instance: str | None = None
        self.revision = 0
        self.state = {"tab": "inbox", "query": "", "reply": ""}
        self.lists: dict[str, list[dict]] = {}
        self.list_errors: dict[str, str] = {}
        self.loading: set[str] = set()
        self.fetched_at: dict[str, float] = {}
        self.detail_id: str | None = None
        self.detail: dict | None = None
        self.detail_error: str | None = None
        self.confirm_trash: str | None = None
        self.lock = threading.RLock()

    # -- rendering ----------------------------------------------------------

    def mount(self) -> None:
        with self.lock:
            if not self.instance:
                return
            self.revision += 1
            document = {"revision": self.revision, "title": self.title(), "state": self.state, "view": self.view()}
            send({"type": "mount", "instance": self.instance,
                  "placement": {"kind": "panel", "open": "split"}, "document": document})

    def title(self) -> str:
        if self.detail_id and self.detail:
            return self.detail["subject"][:80]
        return "Gmail"

    def view(self) -> dict:
        return self.detail_view() if self.detail_id else self.inbox_view()

    def inbox_view(self) -> dict:
        tabs = []
        for section, (label, _) in SECTIONS.items():
            items = self.lists.get(section)
            count = sum(1 for m in items if m["unread"]) if section == "inbox" and items else len(items or [])
            tab_label = f"{label} {count}" if items and count else label
            tabs.append({"id": section, "label": tab_label, "children": [self.list_body(section)]})
        tabs.append({"id": "search", "label": "Search", "children": [self.search_body()]})
        toolbar = hstack(
            button("Refresh", "refresh", "compact", "refresh"),
            button("Open Gmail", "host.open_url", "compact", "link", url="https://mail.google.com/"),
            gap="sm",
        )
        return {"type": "stack", "gap": "md", "children": [
            toolbar, {"type": "tabs", "bind": "tab", "tabs": tabs, "on_change": {"action": "tab"}}]}

    def list_body(self, section: str) -> dict:
        if section in self.list_errors:
            return {"type": "error", "message": self.list_errors[section], "retry": {"action": "refresh"}}
        items = self.lists.get(section)
        if items is None:
            return {"type": "progress", "label": "Loading from Gmail"}
        if not items:
            return {"type": "empty", "title": "Nothing here", "detail": "You're all caught up.", "icon": "check"}
        body = [{"type": "list", "children": [message_row(item, section) for item in items]}]
        if section in self.loading:
            body.insert(0, {"type": "progress", "label": "Refreshing"})
        return {"type": "stack", "gap": "sm", "children": body}

    def search_body(self) -> dict:
        controls = hstack(
            {"type": "input", "bind": "query", "placeholder": "Search mail, e.g. from:alice has:attachment",
             "on_submit": {"action": "search"}},
            button("Search", "search", "primary", "search"),
            gap="sm",
        )
        active = "search" in self.lists or "search" in self.list_errors or "search" in self.loading
        hint = text("Uses Gmail search syntax.", "caption", "dim")
        return {"type": "stack", "gap": "md", "children": [controls, self.list_body("search") if active else hint]}

    def detail_view(self) -> dict:
        back = button("Back", "back", "compact", "arrow-left")
        if self.detail_error:
            return {"type": "stack", "gap": "md", "children": [
                back, {"type": "error", "message": self.detail_error, "retry": {"action": "reload_detail"}}]}
        if self.detail is None:
            return {"type": "stack", "gap": "md", "children": [back, {"type": "progress", "label": "Loading"}]}
        d = self.detail
        mid = d["id"]
        chips = []
        if d["unread"]:
            chips.append({"type": "chip", "label": "unread", "tone": "accent"})
        if d["important"]:
            chips.append({"type": "chip", "label": "important", "tone": "warning"})
        if d["category"]:
            chips.append({"type": "chip", "label": d["category"].lower()})
        rows = [{"key": "From", "value": d["from"]}, {"key": "To", "value": d.get("to") or ""}]
        if d.get("cc"):
            rows.append({"key": "Cc", "value": d["cc"]})
        rows.append({"key": "Date", "value": d.get("date") or when(d)})

        actions = [
            button("Open in Gmail", "host.open_url", "primary", "link", url=gmail_url(d["thread"])),
            button("Ask Jcode", "host.start_chat", "secondary", "sparkles", prompt=self.agent_prompt(d)),
        ]
        if can_modify():
            actions += [
                button("Unstar" if d["starred"] else "Star", "star", "compact", "star", id=mid),
                button("Mark unread" if not d["unread"] else "Mark read", "toggle_read", "compact", "mail", id=mid),
            ]
            if d["inbox"]:
                actions.append(button("Archive", "archive", "compact", "folder", id=mid))
            if self.confirm_trash == mid:
                actions += [button("Confirm delete", "trash", "danger", "x", id=mid),
                            button("Cancel", "cancel_trash", "compact")]
            else:
                actions.append(button("Delete", "ask_trash", "compact", "x", id=mid))
        children = [
            hstack(back, gap="sm"),
            text(d["subject"], "heading"),
            *( [hstack(*chips)] if chips else [] ),
            hstack(*actions, gap="sm"),
            {"type": "key_value", "rows": rows},
            {"type": "divider"},
            text(d["body"][:60000], "body"),
            {"type": "divider"},
            {"type": "input", "bind": "reply", "placeholder": "Write a reply. Saved as a Gmail draft.",
             "multiline": True},
            hstack(button("Save reply draft", "draft_reply", "primary", "comment", id=mid), gap="sm"),
        ]
        return {"type": "stack", "gap": "md", "children": children}

    @staticmethod
    def agent_prompt(d: dict) -> str:
        return (f"Help me with this email from {d['from']} with subject \"{d['subject']}\" "
                f"(Gmail message id {d['id']}, thread {d['thread']}). Read it with the gmail tool, "
                "summarize what it asks of me, and draft a reply. Do not send anything without asking me.")

    # -- background work ------------------------------------------------------

    def spawn(self, fn, *args) -> None:
        threading.Thread(target=fn, args=args, daemon=True).start()

    def refresh_section(self, section: str) -> None:
        with self.lock:
            if section in self.loading:
                return
            self.loading.add(section)
            query = SECTIONS[section][1] if section in SECTIONS else self.state.get("query", "").strip()
        self.mount()

        def run() -> None:
            try:
                items = list_messages(query or "in:anywhere")
                with self.lock:
                    self.lists[section] = items
                    self.list_errors.pop(section, None)
                    self.fetched_at[section] = time.time()
            except Exception as error:  # noqa: BLE001 - surfaced in the UI
                with self.lock:
                    self.list_errors[section] = str(error)
            finally:
                with self.lock:
                    self.loading.discard(section)
                self.mount()

        self.spawn(run)

    def refresh_visible(self) -> None:
        tab = self.state.get("tab", "inbox")
        self.refresh_section("inbox")
        if tab in SECTIONS and tab != "inbox":
            self.refresh_section(tab)

    def open_detail(self, message_id: str) -> None:
        with self.lock:
            self.detail_id = message_id
            self.detail = None
            self.detail_error = None
            self.confirm_trash = None
            self.state["reply"] = ""
        self.mount()
        self.load_detail(mark_read=True)

    def load_detail(self, mark_read: bool = False) -> None:
        message_id = self.detail_id
        if not message_id:
            return

        def run() -> None:
            try:
                detail = fetch_detail(message_id)
                if mark_read and detail["unread"] and can_modify():
                    api("POST", f"messages/{message_id}/modify", body={"removeLabelIds": ["UNREAD"]})
                    detail["unread"] = False
                    self.patch_summary(message_id, unread=False)
                with self.lock:
                    if self.detail_id == message_id:
                        self.detail, self.detail_error = detail, None
            except Exception as error:  # noqa: BLE001
                with self.lock:
                    if self.detail_id == message_id:
                        self.detail_error = str(error)
            self.mount()

        self.spawn(run)

    def patch_summary(self, message_id: str, **fields) -> None:
        with self.lock:
            for items in self.lists.values():
                for item in items:
                    if item["id"] == message_id:
                        item.update(fields)

    def modify(self, message_id: str, add: list[str], remove: list[str], done: str, leave: bool = False) -> None:
        instance = self.instance
        send({"type": "busy", "instance": instance, "busy": True})

        def run() -> None:
            try:
                api("POST", f"messages/{message_id}/modify", body={"addLabelIds": add, "removeLabelIds": remove})
                send({"type": "toast", "instance": instance, "text": done, "tone": "success"})
                if leave:
                    with self.lock:
                        self.detail_id = None
                        for items in self.lists.values():
                            items[:] = [m for m in items if m["id"] != message_id]
                    self.mount()
                    self.refresh_visible()
                else:
                    self.load_detail()
                    self.refresh_visible()
            except Exception as error:  # noqa: BLE001
                send({"type": "toast", "instance": instance, "text": f"{done} failed: {error}", "tone": "danger"})
            finally:
                send({"type": "busy", "instance": instance, "busy": False})

        self.spawn(run)

    def trash(self, message_id: str) -> None:
        instance = self.instance
        send({"type": "busy", "instance": instance, "busy": True})

        def run() -> None:
            try:
                api("POST", f"messages/{message_id}/trash")
                send({"type": "toast", "instance": instance, "text": "Moved to Trash", "tone": "success"})
                with self.lock:
                    self.detail_id = None
                    self.confirm_trash = None
                    for items in self.lists.values():
                        items[:] = [m for m in items if m["id"] != message_id]
                self.mount()
            except Exception as error:  # noqa: BLE001
                send({"type": "toast", "instance": instance, "text": f"Delete failed: {error}", "tone": "danger"})
            finally:
                send({"type": "busy", "instance": instance, "busy": False})

        self.spawn(run)

    def draft_reply(self) -> None:
        body = (self.state.get("reply") or "").strip()
        if not body:
            send({"type": "toast", "instance": self.instance, "text": "Write a reply first", "tone": "warning"})
            return
        detail = self.detail
        if not detail:
            return
        instance = self.instance
        send({"type": "busy", "instance": instance, "busy": True})

        def run() -> None:
            try:
                create_reply_draft(detail, body)
                with self.lock:
                    self.state["reply"] = ""
                send({"type": "toast", "instance": instance, "text": "Reply saved to Drafts", "tone": "success"})
                self.mount()
            except Exception as error:  # noqa: BLE001
                send({"type": "toast", "instance": instance, "text": f"Draft failed: {error}", "tone": "danger"})
            finally:
                send({"type": "busy", "instance": instance, "busy": False})

        self.spawn(run)

    # -- host messages --------------------------------------------------------

    def handle(self, message: dict) -> None:
        kind = message.get("type")
        if kind == "launch":
            with self.lock:
                self.instance = message["instance"]
                self.detail_id = None
            self.mount()
            if not self.fetched_at or time.time() - min(self.fetched_at.values()) > 60:
                self.refresh_visible()
        elif kind == "action" and message.get("instance") == self.instance:
            state = message.get("state")
            if isinstance(state, dict):
                with self.lock:
                    self.state.update(state)
            self.on_action(message.get("action") or {})
        elif kind == "resync" and message.get("instance") == self.instance:
            self.mount()
        elif kind == "closed" and message.get("instance") == self.instance:
            with self.lock:
                self.instance = None
                self.detail_id = None
        elif kind == "rejected":
            log(f"host rejected a message: {message.get('reason')}")

    def on_action(self, action: dict) -> None:
        name = action.get("action")
        args = action.get("args") or {}
        mid = args.get("id", "")
        detail = self.detail if self.detail and self.detail["id"] == mid else None
        if name == "open" and mid:
            self.open_detail(mid)
        elif name == "back":
            with self.lock:
                self.detail_id = None
            self.mount()
        elif name == "reload_detail":
            with self.lock:
                self.detail_error = None
            self.mount()
            self.load_detail()
        elif name == "refresh":
            with self.lock:
                self.lists.clear()
                self.list_errors.clear()
            self.refresh_visible()
            if self.state.get("tab") == "search" and self.state.get("query"):
                self.refresh_section("search")
        elif name == "search":
            with self.lock:
                self.state["tab"] = "search"
            self.refresh_section("search")
        elif name == "tab":
            tab = self.state.get("tab")
            if tab in SECTIONS and tab not in self.lists and tab not in self.loading:
                self.refresh_section(tab)
            else:
                self.mount()
        elif name == "star" and detail:
            if detail["starred"]:
                self.modify(mid, [], ["STARRED"], "Unstarred")
            else:
                self.modify(mid, ["STARRED"], [], "Starred")
        elif name == "toggle_read" and detail:
            if detail["unread"]:
                self.modify(mid, [], ["UNREAD"], "Marked read")
            else:
                self.modify(mid, ["UNREAD"], [], "Marked unread", leave=True)
        elif name == "archive" and detail:
            self.modify(mid, [], ["INBOX"], "Archived", leave=True)
        elif name == "ask_trash" and mid:
            with self.lock:
                self.confirm_trash = mid
            self.mount()
        elif name == "cancel_trash":
            with self.lock:
                self.confirm_trash = None
            self.mount()
        elif name == "trash" and mid and self.confirm_trash == mid:
            self.trash(mid)
        elif name == "draft_reply" and detail:
            self.draft_reply()

    def tick(self) -> None:
        """Periodic refresh while the inbox is open."""
        while True:
            time.sleep(REFRESH_SECONDS)
            with self.lock:
                open_inbox = self.instance and not self.detail_id
            if open_inbox:
                self.refresh_visible()


MANIFEST = {
    "schema": SCHEMA,
    "id": APPLET_ID,
    "title": "Gmail",
    "icon": "mail",
    "description": "Your Gmail inbox: read, triage, and draft replies",
    "launchers": [
        {"trigger": "sidebar", "placement": {"kind": "panel", "open": "split"}},
        {"trigger": "command", "label": "Gmail: inbox", "placement": {"kind": "panel", "open": "split"}},
    ],
    "capabilities": ["open_url", "start_chat"],
}


def main() -> None:
    app = App()
    send({"type": "register", "manifest": MANIFEST})
    threading.Thread(target=app.tick, daemon=True).start()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as error:
            log(f"bad host line: {error}")
            continue
        try:
            app.handle(message)
        except Exception as error:  # noqa: BLE001 - never die on one message
            log(f"error handling {message.get('type')}: {error}")


if __name__ == "__main__":
    main()
