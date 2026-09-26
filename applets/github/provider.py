#!/usr/bin/env python3
"""GitHub issues and pull requests applet for Jcode Desktop.

Speaks `jcode.applet/1` newline-delimited JSON on stdio. All GitHub access goes
through the `gh` CLI, so it uses the user's existing `gh auth` login and never
sees a token itself. Standard library only.
"""
from __future__ import annotations

import datetime as dt
import json
import os
import shlex
import shutil
import subprocess
import sys
import threading
import time

APPLET_ID = os.environ.get("JCODE_APPLET_ID", "github")
SCHEMA = os.environ.get("JCODE_APPLET_SCHEMA", "jcode.applet/1")
REFRESH_SECONDS = 300
LIST_LIMIT = 30
BASE_FIELDS = "number,title,repository,author,updatedAt,url,labels,commentsCount,state"
# `gh search issues` has no isDraft field.
SEARCH_FIELDS = {"prs": BASE_FIELDS + ",isDraft", "issues": BASE_FIELDS}

# Inbox sections: id -> (label, gh subcommand, filter args).
SECTIONS = {
    "review": ("Review requested", "prs", ["--review-requested=@me", "--state=open"]),
    "prs": ("My PRs", "prs", ["--author=@me", "--state=open"]),
    "assigned": ("Assigned", "issues", ["--assignee=@me", "--state=open"]),
    "issues": ("My issues", "issues", ["--author=@me", "--state=open"]),
    "mentions": ("Mentions", "issues", ["--mentions=@me", "--state=open"]),
}

_out_lock = threading.Lock()


def send(message: dict) -> None:
    with _out_lock:
        sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def log(text: str) -> None:
    sys.stderr.write(f"[applet {APPLET_ID}] {text}\n")
    sys.stderr.flush()


def gh(*args: str, timeout: float = 30) -> str:
    """Run gh and return stdout. Raises RuntimeError with gh's message."""
    if shutil.which("gh") is None:
        raise RuntimeError("The GitHub CLI (gh) is not installed. Install it, then run `gh auth login`.")
    try:
        done = subprocess.run(
            ["gh", *args],
            capture_output=True,
            text=True,
            timeout=timeout,
            env={**os.environ, "GH_PROMPT_DISABLED": "1", "NO_COLOR": "1"},
        )
    except subprocess.TimeoutExpired:
        raise RuntimeError("GitHub did not respond in time.") from None
    if done.returncode != 0:
        lines = [line.strip() for line in (done.stderr or done.stdout).splitlines() if line.strip()]
        raise RuntimeError(lines[0] if lines else f"gh exited with {done.returncode}")
    return done.stdout


def relative(stamp: str | None) -> str:
    if not stamp:
        return ""
    try:
        when = dt.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    except ValueError:
        return ""
    seconds = max(0, (dt.datetime.now(dt.timezone.utc) - when).total_seconds())
    for unit, size in (("y", 31536000), ("mo", 2592000), ("d", 86400), ("h", 3600), ("m", 60)):
        if seconds >= size:
            return f"{int(seconds // size)}{unit}"
    return "now"


def is_pr_url(url: str) -> bool:
    return "/pull/" in url


def query_terms(query: str) -> list[str]:
    """Split a search box into gh terms. A single arg would be searched as one phrase."""
    try:
        terms = shlex.split(query)
    except ValueError:
        terms = query.split()
    # `--` stops gh reading user text as flags.
    return ["--", *terms] if terms else []


def section_search(section: str, query: str, repo: str) -> list[dict]:
    _, kind, filters = SECTIONS[section]
    args = ["search", kind, *filters, "--limit", str(LIST_LIMIT), "--json", SEARCH_FIELDS[kind]]
    if repo:
        args.append(f"--repo={repo}")
    if query:
        args.extend(query_terms(query))
    return json.loads(gh(*args) or "[]")


def free_search(kind: str, query: str, repo: str) -> list[dict]:
    args = ["search", kind, "--limit", str(LIST_LIMIT), "--json", SEARCH_FIELDS[kind], "--sort", "updated"]
    if repo:
        args.append(f"--repo={repo}")
    if query:
        args.extend(query_terms(query))
    elif not repo:
        args.append("--involves=@me")
    return json.loads(gh(*args) or "[]")


PR_FIELDS = (
    "number,title,url,state,isDraft,author,body,createdAt,updatedAt,baseRefName,headRefName,"
    "additions,deletions,changedFiles,reviewDecision,mergeable,statusCheckRollup,labels,comments,"
    "reviewRequests,assignees"
)
ISSUE_FIELDS = "number,title,url,state,author,body,createdAt,updatedAt,labels,comments,assignees,milestone"


def fetch_detail(url: str) -> dict:
    if is_pr_url(url):
        return json.loads(gh("pr", "view", url, "--json", PR_FIELDS))
    return json.loads(gh("issue", "view", url, "--json", ISSUE_FIELDS))


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


def checks_summary(rollup: list[dict] | None) -> tuple[str, str] | None:
    if not rollup:
        return None
    failed = pending = passed = 0
    for check in rollup:
        result = (check.get("conclusion") or check.get("state") or check.get("status") or "").upper()
        if result in ("SUCCESS", "NEUTRAL", "SKIPPED"):
            passed += 1
        elif result in ("FAILURE", "ERROR", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED", "STARTUP_FAILURE"):
            failed += 1
        else:
            pending += 1
    if failed:
        return f"{failed} failing", "danger"
    if pending:
        return f"{pending} pending", "warning"
    return f"{passed} passing", "success"


def item_row(item: dict, section: str) -> dict:
    url = item.get("url", "")
    pr = is_pr_url(url)
    repo = (item.get("repository") or {}).get("nameWithOwner", "")
    author = (item.get("author") or {}).get("login", "")
    badges = []
    if item.get("isDraft"):
        badges.append("draft")
    badges.extend(label.get("name", "") for label in (item.get("labels") or [])[:3])
    comments = item.get("commentsCount") or 0
    subtitle = f"{repo}#{item.get('number')}"
    if author:
        subtitle += f" · {author}"
    if comments:
        subtitle += f" · {comments} comment{'s' if comments != 1 else ''}"
    return {
        "type": "list_item",
        # Keys are unique per document, and one item can sit in several tabs.
        "key": f"{section}:{url}",
        "title": item.get("title", "(untitled)"),
        "subtitle": subtitle,
        "meta": relative(item.get("updatedAt")),
        "leading": {"type": "icon", "name": "pull-request" if pr else "issue", "tone": "accent" if pr else "success"},
        "badges": [b for b in badges if b],
        "on_press": {"action": "open", "args": {"url": url}},
    }


class App:
    def __init__(self) -> None:
        self.instance: str | None = None
        self.revision = 0
        self.state = {"tab": "review", "query": "", "repo": "", "kind": "prs", "comment": ""}
        self.lists: dict[str, list[dict]] = {}
        self.list_errors: dict[str, str] = {}
        self.loading: set[str] = set()
        self.fetched_at: dict[str, float] = {}
        self.detail_url: str | None = None
        self.detail: dict | None = None
        self.detail_error: str | None = None
        self.confirm_close: str | None = None
        self._viewer: str | None = None
        self.lock = threading.RLock()

    def viewer(self) -> str:
        """The signed-in GitHub login, looked up once."""
        if self._viewer is None:
            try:
                self._viewer = gh("api", "user", "--jq", ".login", timeout=10).strip()
            except Exception:  # noqa: BLE001 - only hides Approve on own PRs
                self._viewer = ""
        return self._viewer

    # -- rendering ----------------------------------------------------------

    def mount(self) -> None:
        with self.lock:
            if not self.instance:
                return
            self.revision += 1
            document = {
                "revision": self.revision,
                "title": self.title(),
                "state": self.state,
                "view": self.view(),
            }
            send({
                "type": "mount",
                "instance": self.instance,
                "placement": {"kind": "panel", "open": "split"},
                "document": document,
            })

    def title(self) -> str:
        if self.detail_url and self.detail:
            return f"#{self.detail.get('number')} {self.detail.get('title', '')}"[:80]
        return "GitHub"

    def view(self) -> dict:
        if self.detail_url:
            return self.detail_view()
        return self.inbox_view()

    def inbox_view(self) -> dict:
        tabs = []
        for section, (label, _, _) in SECTIONS.items():
            count = len(self.lists.get(section, []))
            tab_label = f"{label} {count}" if section in self.lists and count else label
            tabs.append({"id": section, "label": tab_label, "children": [self.list_body(section)]})
        tabs.append({"id": "search", "label": "Search", "children": [self.search_body()]})
        toolbar = hstack(
            {"type": "input", "bind": "repo", "placeholder": "Filter by repo (owner/name)", "on_submit": {"action": "refresh"}},
            button("Refresh", "refresh", "compact", "refresh"),
        )
        return {
            "type": "stack",
            "gap": "md",
            "children": [
                toolbar,
                {"type": "tabs", "bind": "tab", "tabs": tabs, "on_change": {"action": "tab"}},
            ],
        }

    def list_body(self, section: str) -> dict:
        if section in self.list_errors:
            return {"type": "error", "message": self.list_errors[section], "retry": {"action": "refresh"}}
        items = self.lists.get(section)
        if items is None:
            return {"type": "progress", "label": "Loading from GitHub"}
        if not items:
            return {"type": "empty", "title": "Nothing here", "detail": "You're all caught up.", "icon": "check"}
        seen: set[str] = set()
        children = []
        for item in items:
            if item.get("url") not in seen:
                seen.add(item.get("url"))
                children.append(item_row(item, section))
        stamp = self.fetched_at.get(section)
        caption = f"Updated {relative(dt.datetime.fromtimestamp(stamp, dt.timezone.utc).isoformat())} ago" if stamp else ""
        if caption.startswith("Updated now"):
            caption = "Updated just now"
        body = [{"type": "list", "children": children}]
        if caption:
            body.append(text(caption, "caption", "dim"))
        if section in self.loading:
            body.insert(0, {"type": "progress", "label": "Refreshing"})
        return {"type": "stack", "gap": "sm", "children": body}

    def search_body(self) -> dict:
        controls = {
            "type": "stack",
            "gap": "sm",
            "children": [
                hstack(
                    {"type": "select", "bind": "kind", "options": [
                        {"value": "prs", "label": "Pull requests"},
                        {"value": "issues", "label": "Issues"},
                    ], "on_change": {"action": "search"}},
                ),
                hstack(
                    {"type": "input", "bind": "query", "placeholder": "Search, e.g. is:open label:bug",
                     "on_submit": {"action": "search"}},
                    button("Search", "search", "primary", "search"),
                ),
            ],
        }
        return {"type": "stack", "gap": "md", "children": [controls, self.list_body("search") if (
            "search" in self.lists or "search" in self.list_errors or "search" in self.loading
        ) else text("Search across GitHub. Leave empty to see everything involving you.", "caption", "dim")]}

    def detail_view(self) -> dict:
        back = button("Back", "back", "compact", "arrow-left")
        if self.detail_error:
            return {"type": "stack", "gap": "md", "children": [
                back, {"type": "error", "message": self.detail_error, "retry": {"action": "reload_detail"}}]}
        if self.detail is None:
            return {"type": "stack", "gap": "md", "children": [back, {"type": "progress", "label": "Loading"}]}
        d = self.detail
        url = d.get("url", self.detail_url)
        pr = is_pr_url(url)
        state = (d.get("state") or "").lower()
        chips = [{"type": "chip", "label": "draft" if d.get("isDraft") else state,
                  "tone": {"open": "success", "merged": "accent", "closed": "danger"}.get(state, "default")}]
        rows = [{"key": "Author", "value": (d.get("author") or {}).get("login", "")},
                {"key": "Opened", "value": f"{relative(d.get('createdAt'))} ago"},
                {"key": "Updated", "value": f"{relative(d.get('updatedAt'))} ago"}]
        if pr:
            decision = (d.get("reviewDecision") or "").replace("_", " ").lower()
            if decision:
                chips.append({"type": "chip", "label": decision,
                              "tone": "success" if decision == "approved" else "warning"})
            checks = checks_summary(d.get("statusCheckRollup"))
            if checks:
                chips.append({"type": "chip", "label": f"checks {checks[0]}", "tone": checks[1]})
            rows += [
                {"key": "Branch", "value": f"{d.get('headRefName')} → {d.get('baseRefName')}"},
                {"key": "Changes", "value": f"+{d.get('additions', 0)} −{d.get('deletions', 0)} in {d.get('changedFiles', 0)} files"},
            ]
            if d.get("mergeable") and d.get("mergeable") != "UNKNOWN":
                rows.append({"key": "Mergeable", "value": d["mergeable"].lower()})
        assignees = ", ".join(a.get("login", "") for a in d.get("assignees") or [])
        if assignees:
            rows.append({"key": "Assignees", "value": assignees})
        for label in (d.get("labels") or [])[:6]:
            chips.append({"type": "chip", "label": label.get("name", "")})

        kind = "pull request" if pr else "issue"
        mine = (d.get("author") or {}).get("login") == self.viewer()
        if state == "open" and self.confirm_close == url:
            close = [button("Confirm close", "close_item", "danger", "x", url=url),
                     button("Cancel", "cancel_close", "compact")]
        elif state == "open":
            close = [button("Close", "ask_close", "compact", "x", url=url)]
        elif state == "closed":
            close = [button("Reopen", "reopen_item", "compact", "refresh", url=url)]
        else:
            close = []
        actions = hstack(
            button("Open on GitHub", "host.open_url", "primary", "link", url=url),
            button("Ask Jcode", "host.start_chat", "secondary", "sparkles", prompt=self.agent_prompt(d, pr)),
            button("Copy link", "host.copy", "compact", text=url),
            *( [button("Approve", "approve", "compact", "check", url=url)] if pr and state == "open" and not mine else [] ),
            *close,
            gap="sm",
        )
        body = d.get("body") or "_No description provided._"
        comments = d.get("comments") or []
        comment_nodes = []
        for comment in comments[-10:]:
            who = (comment.get("author") or {}).get("login", "")
            comment_nodes.append({"type": "card", "title": f"{who} · {relative(comment.get('createdAt'))} ago",
                                  "children": [{"type": "markdown", "text": comment.get("body") or ""}]})
        children = [
            hstack(back, gap="sm"),
            text(d.get("title", ""), "heading"),
            text(f"{self.repo_of(url)}#{d.get('number')} · {kind}", "caption", "dim"),
            hstack(*chips),
            actions,
            {"type": "key_value", "rows": rows},
            {"type": "divider"},
            {"type": "markdown", "text": body[:60000]},
        ]
        if comments:
            extra = f" (latest 10 of {len(comments)})" if len(comments) > 10 else ""
            children += [{"type": "divider"}, text(f"Comments{extra}", "title"), *comment_nodes]
        children += [
            {"type": "input", "bind": "comment", "placeholder": "Leave a comment (Markdown)", "multiline": True},
            hstack(button("Comment", "comment", "primary", url=url), gap="sm"),
        ]
        return {"type": "stack", "gap": "md", "children": children}

    @staticmethod
    def repo_of(url: str) -> str:
        parts = url.split("github.com/", 1)[-1].split("/")
        return "/".join(parts[:2])

    @staticmethod
    def agent_prompt(d: dict, pr: bool) -> str:
        url = d.get("url", "")
        if pr:
            return (f"Review GitHub pull request {url} (\"{d.get('title', '')}\"). Use `gh pr view` and "
                    "`gh pr diff` to read it, check the reasoning and tests, and summarize any problems "
                    "with concrete suggestions. Do not post anything to GitHub without asking me.")
        return (f"Look into GitHub issue {url} (\"{d.get('title', '')}\"). Use `gh issue view` to read it and "
                "its comments, find the relevant code in this workspace, and propose a fix. Do not post "
                "anything to GitHub without asking me.")

    # -- background work ------------------------------------------------------

    def spawn(self, fn, *args) -> None:
        threading.Thread(target=fn, args=args, daemon=True).start()

    def refresh_section(self, section: str) -> None:
        with self.lock:
            if section in self.loading:
                return
            self.loading.add(section)
            repo = self.state.get("repo", "").strip()
            query = self.state.get("query", "").strip()
            kind = self.state.get("kind", "prs")
        self.mount()

        def run() -> None:
            try:
                if section == "search":
                    items = free_search(kind if kind in ("prs", "issues") else "prs", query, repo)
                else:
                    items = section_search(section, "", repo)
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

    def refresh_all(self) -> None:
        for section in SECTIONS:
            self.refresh_section(section)

    def open_detail(self, url: str) -> None:
        with self.lock:
            self.detail_url = url
            self.detail = None
            self.detail_error = None
            self.confirm_close = None
            self.state["comment"] = ""
        self.mount()
        self.load_detail()

    def load_detail(self) -> None:
        url = self.detail_url
        if not url:
            return

        def run() -> None:
            try:
                self.viewer()
                detail = fetch_detail(url)
                with self.lock:
                    if self.detail_url == url:
                        self.detail, self.detail_error = detail, None
            except Exception as error:  # noqa: BLE001
                with self.lock:
                    if self.detail_url == url:
                        self.detail_error = str(error)
            self.mount()

        self.spawn(run)

    def write_action(self, label: str, args: list[str], done: str) -> None:
        """Run a mutating gh command, then reload the detail view."""
        instance = self.instance
        send({"type": "busy", "instance": instance, "busy": True})

        def run() -> None:
            try:
                gh(*args)
                send({"type": "toast", "instance": instance, "text": done, "tone": "success"})
                with self.lock:
                    self.state["comment"] = "" if label == "comment" else self.state.get("comment", "")
                self.load_detail()
                with self.lock:
                    self.fetched_at.clear()
            except Exception as error:  # noqa: BLE001
                send({"type": "toast", "instance": instance, "text": f"{label} failed: {error}", "tone": "danger"})
            finally:
                send({"type": "busy", "instance": instance, "busy": False})

        self.spawn(run)

    # -- host messages --------------------------------------------------------

    def handle(self, message: dict) -> None:
        kind = message.get("type")
        if kind == "launch":
            with self.lock:
                self.instance = message["instance"]
                self.detail_url = None
            self.mount()
            stale = not self.fetched_at or time.time() - min(self.fetched_at.values()) > 60
            if stale:
                self.refresh_all()
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
                self.detail_url = None
        elif kind == "rejected":
            log(f"host rejected a message: {message.get('reason')}")

    def on_action(self, action: dict) -> None:
        name = action.get("action")
        args = action.get("args") or {}
        url = args.get("url", "")
        if name == "open" and url:
            self.open_detail(url)
        elif name == "back":
            with self.lock:
                self.detail_url = None
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
            self.refresh_all()
            if self.state.get("tab") == "search":
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
        elif name == "comment" and url:
            body = (self.state.get("comment") or "").strip()
            if not body:
                send({"type": "toast", "instance": self.instance, "text": "Write a comment first", "tone": "warning"})
                return
            sub = "pr" if is_pr_url(url) else "issue"
            self.write_action("comment", [sub, "comment", url, "--body", body], "Comment posted")
        elif name == "approve" and url:
            self.write_action("approve", ["pr", "review", url, "--approve"], "Approved")
        elif name == "ask_close" and url:
            with self.lock:
                self.confirm_close = url
            self.mount()
        elif name == "cancel_close":
            with self.lock:
                self.confirm_close = None
            self.mount()
        elif name == "close_item" and url and self.confirm_close == url:
            with self.lock:
                self.confirm_close = None
            sub = "pr" if is_pr_url(url) else "issue"
            self.write_action("close", [sub, "close", url], "Closed")
        elif name == "reopen_item" and url:
            sub = "pr" if is_pr_url(url) else "issue"
            self.write_action("reopen", [sub, "reopen", url], "Reopened")

    def tick(self) -> None:
        """Periodic refresh while the panel is open."""
        while True:
            time.sleep(REFRESH_SECONDS)
            with self.lock:
                open_inbox = self.instance and not self.detail_url
            if open_inbox:
                self.refresh_all()


MANIFEST = {
    "schema": SCHEMA,
    "id": APPLET_ID,
    "title": "GitHub",
    "icon": "pull-request",
    "description": "Your GitHub pull requests, review requests, and issues",
    "launchers": [
        {"trigger": "sidebar", "placement": {"kind": "panel", "open": "split"}},
        {"trigger": "command", "label": "GitHub: issues and pull requests",
         "placement": {"kind": "panel", "open": "split"}},
    ],
    "capabilities": ["open_url", "clipboard", "start_chat"],
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
