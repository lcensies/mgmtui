# Encrypted backups (`mgmt backup`)

`mgmt backup` snapshots the whole vault to an [rclone](https://rclone.org) remote. Each snapshot is
a single `tar.zst` of the data root (tasks, calendars, projects, trash, state) plus, optionally,
your config — with an embedded `manifest.json` listing every file and its SHA-256. A small sidecar
manifest is uploaded alongside so `list`/`verify` never has to download an archive.

Encryption is **rclone's** job: point the remote at an `rclone crypt` wrapper and mgmt never sees
the passphrase. This keeps backups provider-agnostic — Backblaze B2, S3, Google Drive, a WebDAV
box, or a local path all work identically.

## One-time setup

1. Install rclone and create a backend + a crypt wrapper over it:

   ```bash
   rclone config
   # 1) create a remote for your provider, e.g. `b2:` (or s3/drive/webdav/…)
   # 2) create a second remote of type `crypt` wrapping it, e.g. `crypt-b2:` -> `b2:mgmt-bucket`
   #    rclone stores the crypt passphrase in rclone.conf; protect it with RCLONE_CONFIG_PASS if desired.
   ```

2. Add a `backup:` section to `~/.config/mgmt/config.yaml`:

   ```yaml
   backup:
     remote: "crypt-b2:mgmt-backups"   # rclone remote:path (required)
     keep_last: 14                     # keep at most N recent snapshots
     keep_days: 180                    # also drop snapshots older than this (0 = no age cap)
     include_config: true              # bundle config.yaml (+ web-auth.yaml)
   ```

   `mgmt backup` warns if `remote` is not an rclone `crypt` remote, since the archive would then be
   uploaded unencrypted.

## Commands

```bash
mgmt backup run [--dry-run]     # snapshot -> upload -> prune per retention policy
mgmt backup list                # snapshots on the remote, newest first
mgmt backup prune [--dry-run]   # apply retention without taking a new snapshot
mgmt backup verify [NAME] [--deep]
                                # no NAME: check every snapshot has its sidecar
                                # NAME:    download + full SHA-256 verify that snapshot
                                # --deep:  download + verify all of them
mgmt restore latest [--yes]     # restore the newest snapshot in place (see safety below)
mgmt restore NAME --to DIR      # extract a snapshot to DIR for inspection (no swap)
```

`NAME` accepts a full filename, a unique prefix, or `latest`.

## Retention

A snapshot is kept only if it is among the newest `keep_last` **and** younger than `keep_days`
(when set). Both bounds apply, so the remote file count stays bounded even with frequent backups.
The newest snapshot is always kept.

## Restore safety

An in-place restore never untars over the live vault:

1. the archive is downloaded and its manifest SHA-256s are verified;
2. its `data/` tree is extracted to a sibling staging dir on the same filesystem;
3. the current data root is renamed aside to `<data_root>.pre-restore-<timestamp>`;
4. the staged tree is renamed into place.

The pre-restore copy is **never deleted automatically** — remove it yourself once you're satisfied.
In-place restore requires `--yes` and refuses to run while a backup holds the lock. Use `--to DIR`
to extract a snapshot elsewhere without touching the vault.

## Scheduling

Install the bundled systemd *user* units for a daily backup:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/systemd/mgmt-backup.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now mgmt-backup.timer
```

Alternatively, a `post-sync` hook can back up after every CalDAV sync — put `exec mgmt backup run`
in `~/.config/mgmt/hooks/post-sync` (chmod +x).
