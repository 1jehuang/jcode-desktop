"""Transcript cards for GitHub MCP tool calls.

The GitHub MCP server (github/github-mcp-server) returns JSON text. This module
turns a finished `mcp__github__*` call into a readable card: a pull request or
issue with its state, a list of results, CI checks, or a confirmation for a
write. Rendering uses only the call's own output, so no extra network calls.
Unknown tools and output that does not parse fall back to a short summary.
"""
from __future__ import annotations

import json
import os

SERVER = os.environ.get("JCODE_GITHUB_MCP_SERVER", "github")
LIST_MAX = 15

READ_TOOLS = (
    "pull_request_read",
    "issue_read",
    "list_pull_requests",
    "list_issues",
    "search_issues",
    "search_pull_requests",
)
WRITE_TOOLS = (
    "create_pull_request",
    "update_pull_request",
    "merge_pull_request",
    "issue_write",
    "add_issue_comment",
)
TOOLS = READ_TOOLS + WRITE_TOOLS

VERBS = {
    "pull_request_read": "Reading pull request",
    "issue_read": "Reading issue",
    "list_pull_requests": "Listing pull requests",
    "list_issues": "Listing issues",
    "search_issues": "Searching issues",
    "search_pull_requests": "Searching pull requests",
    "create_pull_request": "Opening pull request",
    "update_pull_request": "Updating pull request",
    "merge_pull_request": "Merging pull request",
    "issue_write": "Writing issue",
    "add_issue_comment": "Commenting",
}

STATE_TONE = {"open": "success", "merged": "accent", "closed": "danger", "draft": "dim"}


def claims() -> list[dict]:
    return [{"tool": f"mcp__{SERVER}__{name}"} for name in TOOLS] + [{"tool": "mcp_call"}]


def resolve(tool: str, raw_input) -> tuple[str, dict] | None:
    """(github tool, arguments) for a claimed call, or None when it isn't ours."""
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


# -- helpers --------------------------------------------------------------------


def _text(value: str, style: str = "body", tone: str = "default", max_lines: int | None = None) -> dict:
    node = {"type": "text", "text": value, "style": style, "tone": tone}
    if max_lines:
        node["max_lines"] = max_lines
    return node


def _row(*children: dict, gap: str = "xs") -> dict:
    return {"type": "stack", "direction": "horizontal", "gap": gap, "align": "center", "children": list(children)}


def _chip(label: str, tone: str = "default") -> dict:
    return {"type": "chip", "label": label, "tone": tone}


def _open(url: str, label: str = "Open on GitHub") -> dict:
    return {"type": "button", "label": label, "variant": "secondary", "icon": "link",
            "on_press": {"action": "host.open_url", "args": {"url": url}}}


def _login(user) -> str:
    return (user or {}).get("login", "") if isinstance(user, dict) else ""


def _repo(args: dict, url: str = "") -> str:
    if args.get("owner") and args.get("repo"):
        return f"{args['owner']}/{args['repo']}"
    if "github.com/" in url:
        return "/".join(url.split("github.com/", 1)[1].split("/")[:2])
    return ""


def _is_pr(item: dict) -> bool:
    url = item.get("html_url") or item.get("url") or ""
    return "/pull/" in url or "pull_request" in item or "head" in item


def _state(item: dict) -> str:
    if item.get("merged") or item.get("merged_at"):
        return "merged"
    if item.get("draft") and (item.get("state") or "").lower() == "open":
        return "draft"
    return (item.get("state") or "").lower()


def _labels(item: dict) -> list[str]:
    out = []
    for label in item.get("labels") or []:
        name = label.get("name") if isinstance(label, dict) else label
        if name:
            out.append(str(name))
    return out


def _parse(output: str):
    try:
        return json.loads(output)
    except (json.JSONDecodeError, TypeError):
        return None


def _url_of(value) -> str | None:
    if isinstance(value, dict):
        for key in ("html_url", "url"):
            url = value.get(key)
            if isinstance(url, str) and url.startswith("https://github.com/"):
                return url
    return None


# -- views ----------------------------------------------------------------------


def item_view(item: dict, args: dict) -> tuple[str, list[dict]]:
    """A single pull request or issue."""
    url = item.get("html_url") or ""
    pr = _is_pr(item)
    state = _state(item)
    title = f"{_repo(args, url)}#{item.get('number')}".lstrip("#")
    chips = [_chip(state, STATE_TONE.get(state, "default"))] if state else []
    chips += [_chip(name) for name in _labels(item)[:5]]
    rows = []
    author = _login(item.get("user"))
    if author:
        rows.append({"key": "Author", "value": author})
    if pr:
        head, base = (item.get("head") or {}).get("ref"), (item.get("base") or {}).get("ref")
        if head and base:
            rows.append({"key": "Branch", "value": f"{head} → {base}"})
        if "additions" in item:
            rows.append({"key": "Changes", "value": f"+{item.get('additions', 0)} −{item.get('deletions', 0)} in {item.get('changed_files', 0)} files"})
        mergeable = (item.get("mergeable_state") or "").replace("_", " ")
        if mergeable and mergeable != "unknown" and state == "open":
            rows.append({"key": "Merge state", "value": mergeable})
    assignees = ", ".join(filter(None, (_login(a) for a in item.get("assignees") or [])))
    if assignees:
        rows.append({"key": "Assignees", "value": assignees})
    children = [_text(item.get("title") or "(untitled)", "heading")]
    if chips:
        children.append(_row(*chips))
    if rows:
        children.append({"type": "key_value", "rows": rows})
    body = (item.get("body") or "").strip()
    if body:
        children.append({"type": "scroll", "max_height": 220,
                         "children": [{"type": "markdown", "text": body[:20000]}]})
    if url:
        children.append(_row(_open(url), gap="sm"))
    return title, children


def list_view(items: list, args: dict, total: int | None, noun: str) -> tuple[str, list[dict]]:
    rows = []
    for item in items[:LIST_MAX]:
        if not isinstance(item, dict):
            continue
        url = item.get("html_url") or item.get("url") or ""
        pr = _is_pr(item)
        state = _state(item)
        subtitle = f"{_repo(args, url)}#{item.get('number')}"
        author = _login(item.get("user"))
        if author:
            subtitle += f" · {author}"
        node = {
            "type": "list_item",
            "key": url or str(item.get("number")),
            "title": item.get("title") or "(untitled)",
            "subtitle": subtitle,
            "leading": {"type": "icon", "name": "pull-request" if pr else "issue",
                        "tone": STATE_TONE.get(state, "default")},
            "badges": [b for b in ([state] if state and state != "open" else []) + _labels(item)[:2] if b],
        }
        if url.startswith("https://github.com/"):
            node["on_press"] = {"action": "host.open_url", "args": {"url": url}}
        rows.append(node)
    count = total if isinstance(total, int) else len(items)
    title = _repo(args) or "GitHub"
    children = [_row(_chip(f"{count} {noun}{'' if count == 1 else 's'}", "dim"))]
    if rows:
        children.append({"type": "scroll", "max_height": 360, "children": [{"type": "list", "children": rows}]})
    else:
        children.append({"type": "empty", "title": f"No {noun}s"})
    if count > len(rows) and rows:
        children.append(_text(f"Showing {len(rows)} of {count}", "caption", "dim"))
    return title, children


def checks_view(data: dict, args: dict) -> tuple[str, list[dict]]:
    runs = data.get("check_runs") or []
    passed = failed = pending = 0
    rows = []
    for run in runs:
        conclusion = (run.get("conclusion") or "").lower()
        if run.get("status") != "completed":
            pending += 1
            tone, label = "warning", "running"
        elif conclusion in ("success", "neutral", "skipped"):
            passed += 1
            tone, label = "success", conclusion
        else:
            failed += 1
            tone, label = "danger", conclusion or "failed"
        rows.append({"type": "list_item", "key": str(run.get("id") or run.get("name")),
                     "title": run.get("name") or "check", "badges": [label],
                     "leading": {"type": "icon", "name": "check" if tone == "success" else "x" if tone == "danger" else "clock",
                                 "tone": tone},
                     **({"on_press": {"action": "host.open_url", "args": {"url": run["html_url"]}}}
                        if str(run.get("html_url") or "").startswith("https://github.com/") else {})})
    chips = []
    if failed:
        chips.append(_chip(f"{failed} failing", "danger"))
    if pending:
        chips.append(_chip(f"{pending} running", "warning"))
    if passed:
        chips.append(_chip(f"{passed} passing", "success"))
    title = f"{_repo(args)}#{args.get('pullNumber', '')} checks"
    children = [_row(*chips)] if chips else []
    children.append({"type": "list", "children": rows} if rows else {"type": "empty", "title": "No checks"})
    return title, children


def write_view(name: str, args: dict, data, output: str) -> tuple[str, list[dict]]:
    url = _url_of(data)
    result = data if isinstance(data, dict) else {}
    number = args.get("pullNumber") or args.get("issue_number") or result.get("number")
    done = {
        "create_pull_request": "Pull request opened",
        "update_pull_request": "Pull request updated",
        "merge_pull_request": "Pull request merged",
        "issue_write": "Issue created" if args.get("method") == "create" else "Issue updated",
        "add_issue_comment": "Comment posted",
    }[name]
    children = [_row(_chip(done, "success"))]
    heading = args.get("title") or result.get("title")
    if heading:
        children.append(_text(str(heading), "heading"))
    body = args.get("body")
    if isinstance(body, str) and body.strip():
        children.append({"type": "scroll", "max_height": 160, "children": [{"type": "markdown", "text": body[:8000]}]})
    if name == "merge_pull_request" and result.get("message"):
        children.append(_text(str(result["message"]), "caption", "dim"))
    if url:
        children.append(_row(_open(url), gap="sm"))
    title = _repo(args, url or "") + (f"#{number}" if number else "")
    return title or "GitHub", children


def render(name: str, args: dict, output: str, error: str | None, done: bool) -> dict:
    """The card for one call's current state."""
    method = str(args.get("method") or "")
    fallback_title = _repo(args) or "GitHub"
    if not done:
        return _card(fallback_title, [{"type": "progress", "label": f"{VERBS.get(name, 'Using GitHub')}…"}])
    failure = error or (output if output.startswith(("failed to", "MCP error", "Error")) else None)
    if failure:
        return _card(fallback_title, [{"type": "error", "message": failure.strip()[:600]}])
    data = _parse(output)
    try:
        if name in WRITE_TOOLS:
            title, children = write_view(name, args, data, output)
        elif name in ("pull_request_read", "issue_read") and method in ("", "get") and isinstance(data, dict):
            title, children = item_view(data, args)
        elif name == "pull_request_read" and method == "get_check_runs" and isinstance(data, dict):
            title, children = checks_view(data, args)
        elif name == "list_pull_requests" and isinstance(data, list):
            title, children = list_view(data, args, None, "pull request")
        elif name == "list_issues" and isinstance(data, dict):
            title, children = list_view(data.get("issues") or [], args, data.get("totalCount"), "issue")
        elif name in ("search_issues", "search_pull_requests") and isinstance(data, dict):
            noun = "pull request" if name == "search_pull_requests" else "issue"
            title, children = list_view(data.get("items") or [], args, data.get("total_count"), noun)
        else:
            title, children = fallback_title, [_text(_summary(name, method), "caption", "dim")]
    except Exception as exc:  # noqa: BLE001 - a bad payload keeps a plain card
        title, children = fallback_title, [_text(f"Could not show this result: {exc}", "caption", "dim")]
    return _card(title, children)


def _summary(name: str, method: str) -> str:
    what = name.replace("_", " ")
    return f"{what} ({method.replace('_', ' ')}) finished" if method else f"{what} finished"


def _card(title: str, children: list) -> dict:
    return {"type": "card", "title": title, "children": children}


class Cards:
    """Tool-call cards for one provider process."""

    def __init__(self, send, applet_id: str) -> None:
        self.send = send
        self.applet_id = applet_id
        self.revisions: dict[str, int] = {}
        self.last: dict[str, tuple[str, dict]] = {}

    def owns(self, instance: str) -> bool:
        return instance in self.last

    def tool_call(self, message: dict) -> bool:
        resolved = resolve(str(message.get("tool") or ""), message.get("input"))
        if not resolved:
            return False
        name, args = resolved
        session_id, call_id = str(message.get("session_id") or ""), str(message.get("call_id") or "")
        if not session_id or not call_id:
            return False
        view = render(name, args, str(message.get("output") or ""), message.get("error"), bool(message.get("done")))
        instance = f"{self.applet_id}#tool-{call_id}"
        self.last[instance] = (session_id, view)
        self.mount(instance, call_id)
        return True

    def mount(self, instance: str, call_id: str | None = None) -> None:
        session_id, view = self.last[instance]
        call_id = call_id or instance.split("#tool-", 1)[1]
        revision = self.revisions.get(instance, 0) + 1
        self.revisions[instance] = revision
        self.send({
            "type": "mount",
            "instance": instance,
            "placement": {"kind": "inline", "session_id": session_id,
                          "anchor": {"kind": "tool_call", "call_id": call_id}},
            "lifetime": "persistent",
            "document": {"revision": revision, "title": view.get("title", "GitHub"), "view": view, "state": {}},
        })

    def closed(self, instance: str) -> None:
        self.last.pop(instance, None)
        self.revisions.pop(instance, None)
