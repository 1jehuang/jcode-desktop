# Applet catalog

Status: API, website pages and a reference client implemented. The in-Desktop
catalog browser and the `jcode applet` CLI subcommand are follow-ups.

The catalog lets anyone with a jcode account publish an applet, and lets users
find one, see exactly what it may do, and install a checksummed copy. It lives
at <https://jcode.sh/applets> and <https://api.jcode.sh/v1/applets>.

| Piece | Location |
| --- | --- |
| API (Cloudflare Worker `subscription-api`, D1, R2) | `solosystems-backend/workers/api/src/applets.js`, `src/applet_package.js` |
| Schema | `solosystems-backend/workers/api/migrations/2026-09-25-applet-catalog.sql` |
| Ops and endpoint reference | `solosystems-backend/workers/api/APPLET_CATALOG.md` |
| Website pages | `jcode-website/public/applets.html`, `public/applets/`, `functions/applets/[[path]].js` |
| Reference client (check, pack, publish, install, search) | `scripts/applet-catalog.py` in this repo |

Local applet basics (protocol, capabilities, limits) are in
[applet-api.md](applet-api.md).

## Package format

A package is a gzip-compressed tar of the applet directory with `applet.json`
at its root. The catalog stores the exact bytes you upload, immutable, keyed
by id, version and sha256.

`applet.json` is a superset of the local format Desktop already reads
(`{id, command, env, autostart}`), so an installed package runs unchanged.

```json
{
  "id": "alice.weather",
  "version": "1.0.0",
  "title": "Weather",
  "description": "Current conditions in a sidebar card.",
  "license": "MIT",
  "author": "Alice",
  "homepage": "https://example.com/weather",
  "repository": "https://github.com/alice/weather",
  "command": ["python3", "provider.py"],
  "env": {},
  "autostart": true,
  "requires": {
    "bins": ["python3"],
    "platforms": ["linux", "macos"],
    "desktop": ">=0.3.3"
  },
  "capabilities": ["open_url"]
}
```

| Field | Rule |
| --- | --- |
| `id` | `<publisher>.<name>`. Publisher is your handle. Both parts are lowercase letters, digits and inner hyphens (handle up to 32, name up to 40). The catalog shows it as `publisher/name` |
| `version` | Semver `MAJOR.MINOR.PATCH[-prerelease]`, no leading zeros. Immutable once published |
| `title`, `description` | 1 to 60 and 10 to 500 characters |
| `license` | SPDX identifier |
| `author`, `homepage`, `repository` | Optional. Links must be `https` |
| `command` | Non-empty array. The program must be a file in the package (`./run.sh`) or listed in `requires.bins` |
| `env` | Up to 32 string values. May not set `PATH`, `HOME`, `SHELL`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `LD_*`, `DYLD_*` or `JCODE_*` |
| `requires.bins` | Programs that must be on `PATH` (for example `python3`, `gh`). Shown on the listing and checked at install |
| `requires.platforms` | Subset of `linux`, `macos`, `windows`. Default all |
| `requires.desktop` | Minimum Desktop version, `>=X.Y.Z` |
| `capabilities` | The capabilities the provider's `register` manifest will declare. Shown with risk labels before install |

The provider must `register` with `manifest.id` equal to `JCODE_APPLET_ID`,
which Desktop sets from `applet.json`. Read it from the environment rather than
hard-coding it, as the GitHub applet does.

Archive rules: regular files and directories only (no symlinks, hard links or
devices), no absolute or `..` paths, no duplicates, at most 500 files, 25 MiB
unpacked and 5 MiB compressed (2 MiB for new publishers). `README.md` at the
root, if present, becomes the listing's readme (first 64 KiB).

`scripts/applet-catalog.py pack <dir>` produces a deterministic archive
(sorted entries, zeroed owners and timestamps, normalized modes), so the same
source always has the same sha256.

## Capability risk

Every listing states that the applet runs a local program with the user's
permissions. Capabilities limit what it can ask Desktop to do. They are not a
sandbox, and the page says so.

| Capability | Risk | Why |
| --- | --- | --- |
| `open_url`, `notifications` | low | Visible, user-initiated effects |
| `clipboard`, `remote_images`, `start_chat` | medium | Data leaves the applet, or a new chat starts with its text |
| `send_prompt`, `read_files`, `html` | high | Can steer an existing agent into running tools, show local files, or render web content |

The applet's risk level is its highest capability. `GET /v1/applets/risk`
returns the table so Desktop and the website never drift.

## Publishing

Publisher identity is the existing jcode account, authenticated with the
`jck_live_` API key that `jcode login` stores. No new auth.

1. `jcode login`
2. Claim a handle once per account: `POST /v1/applets/publishers {"handle": "alice"}`
3. `python3 scripts/applet-catalog.py publish path/to/applet`

The first-party namespace `jcode` is reserved and assigned to the maintainer
account by an admin, which also marks it verified. The GitHub applet
(`applets/github`) is the first entry, published as `jcode.github`. Its
catalog `applet.json` adds `version`, `title`, `description`, `license`,
`requires: {bins: [python3, gh]}` and `capabilities: [open_url, clipboard,
start_chat]` to the local manifest.

Yank a bad version with `POST /v1/applets/<publisher>/<name>/versions/<v>/yank`.
Yanked versions stop resolving as latest, while exact-version installs keep
working so nobody's pinned setup breaks.

## Anti-abuse

| Threat | Control |
| --- | --- |
| Spam publishing | Per-account limits (10/hour, 50/day), per-IP limit (20/hour), 5 new applets per account per day |
| Throwaway accounts | New publishers (first 14 days, unless verified) get 3/hour, 10/day, 2 MiB packages and at most 3 applets |
| Impersonation | Reserved handles (`jcode`, `solosystems`, `admin`, `official`, `github`, ...), reserved prefixes, and handles confusable with a reserved or verified handle are refused |
| Typosquatting | Names within edit distance 1 (2 for names of 8+ characters), or equal after folding lookalikes (`rn`/`m`, `0`/`o`, `1`/`l`, separators), of a popular name go to review. Popular means 25+ installs, a verified publisher, or a seed list (github, gmail, slack, ...). Reusing an established name under another handle also goes to review |
| Risky first steps | A new publisher's applet asking for a high-risk capability goes to review. An unverified applet adding a high-risk capability in an update has that version held |
| Malicious archives | Tar validation above. Gzip bombs stop while inflating |
| Bad actors after listing | One report per reporter per applet (keyed hash of account or /24 IPv4, /48 IPv6 network). Anonymous reports weigh 1, signed-in 2. At 6 the applet is hidden pending review (15 for verified publishers, half for malware reports) |
| Moderation | Admin-token endpoints to approve, hide, restore, remove (applet or version), dismiss reports, suspend or verify publishers and assign reserved handles. Every action is logged |
| Inflated installs | See telemetry below |

Pending, hidden and removed applets are never listed or downloadable. Pending
and hidden ones keep a stub page that explains the state.

## Usage telemetry

Two anonymous events: `install` (sent once by the installer) and `active` (at
most once per UTC day while the applet runs).

```json
POST /v1/applets/telemetry
{"applet": "jcode/github", "version": "0.1.0", "event": "active", "install_id": "<random uuid for this install>"}
```

- The client generates a random id per installed applet and keeps it in
  `~/.jcode/applets/.install-ids.json`. It is not derived from the machine or
  account and is different for each applet.
- The server stores only `HMAC(secret, applet + install_id)`. The raw id is never
  stored, and hashes cannot be joined across applets.
- Installs dedupe on that hash. Only 3 new installs per applet per network per
  day count toward the public number. The rest are recorded as uncounted, so
  rotating ids cannot inflate it.
- 120 events per IP per hour. Unknown applets, duplicates and throttled installs
  all get the same `202` reply.
- `weekly_active` is distinct counted installs with an `active` event in the
  last 7 days, recomputed by the Worker's cron. Rows older than 30 days are
  deleted.
- `JCODE_NO_TELEMETRY` or `DO_NOT_TRACK` disables all of it.

The reference installer sends `install`. Desktop sending `active` is part of
the follow-up below.

## `jcode applet install` (design sketch)

The CLI subcommand should behave like `scripts/applet-catalog.py install`:

```text
$ jcode applet install jcode/github
jcode/github 0.1.0 · verified publisher · 1,284 installs
Your pull requests, review requests and issues in a Desktop panel.
Permissions:
  runs a local program with your user permissions   high
  clipboard     Copy text to your clipboard         medium
  open_url      Open links in your browser          low
  start_chat    Start a new chat with a prompt      medium
Needs: python3, gh (found)
Install? [y/N]
```

1. `GET /v1/applets/<id>/resolve[?version=]` returns the version, sha256,
   package URL, manifest and risk.
2. Show the permissions. Refuse on an unsupported platform and warn about
   missing `requires.bins`.
3. Download, verify sha256, validate the archive with the same rules as the
   server, extract into a staging directory, then atomically replace
   `~/.jcode/applets/<publisher.name>/`. Write `.catalog.json`
   (`{id, version, sha256}`) so updates and uninstalls know the origin.
4. Send the `install` event unless telemetry is off.
5. Related: `jcode applet update [id]` (resolve latest, and ask again if
   capabilities grew), `jcode applet remove <id>`, `jcode applet list`,
   `jcode applet publish <dir>`, `jcode applet yank <id>@<v>`.

Installing does not grant capabilities. Desktop's existing consent prompt
still asks on first run. Capabilities an update adds after that decision stay
denied: `AppletHost::pending_consent` does not prompt again today, although its
comment says it should. Safe, but the applet silently loses the feature.

## Follow-ups

- Desktop catalog browser: an applet panel (it can itself be an applet using
  `open_url`) listing `/v1/applets`, with a detail view showing risk labels and
  an Install pill that runs the install flow above.
- Desktop sends the daily `active` event for catalog-installed applets
  (those with `.catalog.json`), respecting the telemetry opt-out.
- Update notifications when a newer non-yanked version exists, and a hard
  warning when an installed version is removed by moderation.
- Re-prompt for capabilities an update adds (fix `pending_consent`, which
  currently returns nothing once any decision exists).
- `jcode applet` subcommands in the CLI, replacing the reference script.
- Publisher self-service on the website (claim handle, see review state) using
  the browser session instead of curl.
- Optional signed packages (Sigstore or minisign) on top of the sha256 pin.
