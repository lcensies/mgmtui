# Multi-user vaults & native sync

`mgmt web` is multi-tenant: each user owns an isolated vault, and other devices keep in sync with a
user over a simple, persistent **pairing**. This doc covers the server (admin) side, the client
(pairing) side, and how bidirectional sync converges.

## Layout & migration

The server's data root uses a per-user layout:

```
<data_root>/
  users/
    admin/            # the admin's own vault (migrated from a legacy single vault on first start)
      tasks/ calendars/ projects/ .trash/ .state/
    <id>/ …           # each admin-created user
  .state/web-sessions.json   # global session store
```

On first `mgmt web` start a legacy single vault (`<data_root>/{tasks,calendars,projects}`) is moved
under `users/admin/` automatically. The local CLI/TUI/daemon then follow transparently
(`local_vault_root` resolves to `users/admin` once `users/` exists), so one machine running both the
web server and the TUI shares the admin vault.

## Authentication

The web **UI** is a single admin login (password + optional TOTP), handled by `axum-login` +
`tower-sessions` (sessions are persisted so a restart doesn't sign the phone out). Additional
**users** are isolated vaults reached over `/api/sync` with scoped **bearer tokens** — they have no
web-login password. CSRF is covered by the `SameSite=Lax` session cookie.

Provision the admin three ways:

- `MGMT_WEB_PASSWORD` (or a pre-computed `MGMT_WEB_PASSWORD_HASH`) in the environment, or
- `mgmt web setpass` (reads `MGMT_WEB_PASSWORD` or stdin), or
- **first-run setup**: start `mgmt web` with no password and open the UI — it serves only a
  "create admin" screen until claimed (a passwordless non-loopback server no longer refuses to
  start; it locks to setup instead). On loopback with no password the server runs open (dev).

## Managing users (admin)

In the PWA: **Settings → Users** — create a user, copy its **Sync URL**, or delete it. Or via the
API (admin session / admin bearer token only):

```
GET    /api/admin/users                       # list (never exposes token hashes)
POST   /api/admin/users            {id,name}   # create + its isolated vault
DELETE /api/admin/users/:id                    # remove (vault files are left on disk)
POST   /api/admin/users/:id/tokens {name?}     # mint a scoped sync token (shown once)
DELETE /api/admin/users/:id/tokens/:name       # revoke
POST   /api/admin/users/:id/pair-url {name?}   # → { url: "mgmt://pair/…", token }
```

The pair URL is `mgmt://pair/<base64url({host,token,user})>`; `host` comes from
`web.public_origin` in `config.yaml` (set it, or the endpoint returns 400).

## Pairing a device (client)

A **pairing** is a durable link that keeps a local vault in sync with a remote user, bidirectionally,
on an interval. Import the URL:

```
mgmt pair import "mgmt://pair/…"     # clone the remote vault + save the pairing
mgmt pair list
mgmt pair remove <name>
```

Import does a full initial clone, then writes `~/.config/mgmt/sync-pairings.yaml`:

```yaml
pairings:
  - name: laptop
    remote: "https://mgmt.example.com/api/sync"
    token: "…"
    poll: true          # THIS node polls the remote (see below)
    interval_secs: 60
    tasks: true
    calendars: [default]
```

### Who polls whom

Sync is client-driven: exactly one side runs the poll loop, the other is the passive `mgmt web`
server. The `poll` flag decides which:

- `poll: true` (default on import) — this node polls the remote's `/api/sync` every
  `interval_secs`. Typical: a laptop/PC polls the web server that mobile edits land in.
- `poll: false` — this node is passive; it must run `mgmt web` so the remote polls *it*. Use
  `mgmt pair import --no-poll` to flip.

The polling node runs the loop inside `mgmt daemon` (a third cadence alongside reminders and the
status bar); `mgmt sync` also runs every pairing once on demand.

## How sync converges (3-way, bidirectional)

Each pass compares three states — the persisted **base** snapshot (last sync), the current **local**
vault, and the **remote** listing — so it detects edits and deletes on *either* side instead of
blindly overwriting:

| local vs base | remote vs base | action |
|---|---|---|
| unchanged | unchanged | nothing |
| changed | unchanged | push update |
| unchanged | changed | pull |
| absent (deleted) | unchanged | delete on remote |
| unchanged | absent (deleted) | delete locally |
| changed | changed | **conflict → newer `modified` wins** (remote-wins on ties) |

Local edits are detected by hashing the clean-serialized item; conflicts are resolved by the
`modified` timestamp stamped on every mutation (`LAST-MODIFIED` for events, a `modified` frontmatter
field for tasks). The base snapshot lives at `<vault>/.state/sync/<pairing>/<collection>.json`; the
first pass (empty base) is the initial clone.

## Verify

```
cargo test --workspace                                   # 3-way matrix, migration, auth, admin isolation
tests/blackbox/.venv/bin/python -m pytest tests/blackbox/test_usersync.py -q   # bidirectional e2e
```
