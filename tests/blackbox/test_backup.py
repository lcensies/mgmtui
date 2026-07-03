"""Blackbox tests for `mgmt backup` / `mgmt restore`.

These drive the real binary against a *fake* `rclone` (a shell script that mirrors a `name:path`
remote to a local directory), so the full snapshot -> upload -> list -> restore cycle is exercised
without a cloud account or a real rclone install.
"""

import os
import stat

FAKE_RCLONE = r"""#!/usr/bin/env bash
set -euo pipefail
root="${FAKE_REMOTE_ROOT:?}"
map() { case "$1" in [a-zA-Z0-9_-]*:*) echo "$root/${1#*:}";; *) echo "$1";; esac; }
cmd="${1:-}"; shift || true
case "$cmd" in
  version) echo "rclone v9.99-fake" ;;
  config)  echo "[${2:-x}]"; echo "type = crypt" ;;
  copyto)  src="$(map "$1")"; dst="$(map "$2")"; mkdir -p "$(dirname "$dst")"; cp "$src" "$dst" ;;
  deletefile) rm -f "$(map "$1")" ;;
  lsjson)
    dir=""; for a in "$@"; do case "$a" in --*) ;; *) dir="$a";; esac; done
    d="$(map "$dir")"
    python3 - "$d" <<'PY'
import json,os,sys
d=sys.argv[1]; out=[]
if os.path.isdir(d):
    for n in sorted(os.listdir(d)):
        p=os.path.join(d,n)
        if os.path.isfile(p):
            out.append({"Path":n,"Name":n,"Size":os.path.getsize(p),"IsDir":False,"ModTime":"2026-07-03T12:00:00Z"})
print(json.dumps(out))
PY
    ;;
  *) echo "fake rclone: unknown cmd $cmd" >&2; exit 2 ;;
esac
"""


def _setup(env, tmp_path):
    """Install a fake rclone + a backup config; return (remote_dir, config_dir)."""
    binp = tmp_path / "bin" / "rclone"
    binp.parent.mkdir(parents=True, exist_ok=True)
    binp.write_text(FAKE_RCLONE)
    binp.chmod(binp.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)

    remote = tmp_path / "remote"
    cfg_dir = tmp_path / "cfg" / "mgmt"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        "backup:\n"
        f"  remote: 'fakecrypt:backups'\n"
        f"  rclone_binary: '{binp}'\n"
        "  keep_last: 2\n"
    )
    env["FAKE_REMOTE_ROOT"] = str(remote)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "cfg")
    return remote, cfg_dir


def test_backup_run_uploads_archive_and_sidecar(cli, env, tmp_path):
    remote, _ = _setup(env, tmp_path)
    cli("add", "Buy milk")

    r = cli("backup", "run")
    assert r.returncode == 0, r.stderr
    assert "backed up" in r.stdout

    files = sorted(os.listdir(remote / "backups"))
    assert any(f.endswith(".tar.zst") for f in files)
    assert any(f.endswith(".manifest.json") for f in files)


def test_backup_list_and_verify(cli, env, tmp_path):
    _setup(env, tmp_path)
    cli("add", "Task one")
    cli("backup", "run")

    lst = cli("backup", "list")
    assert lst.returncode == 0, lst.stderr
    assert ".tar.zst" in lst.stdout

    ver = cli("backup", "verify", "--deep")
    assert ver.returncode == 0, ver.stderr
    assert ver.stdout.startswith("ok") or "ok " in ver.stdout


def test_restore_to_extracts_snapshot(cli, env, tmp_path):
    _setup(env, tmp_path)
    cli("add", "Restore me")
    cli("backup", "run")

    dest = tmp_path / "inspect"
    r = cli("restore", "latest", "--to", str(dest))
    assert r.returncode == 0, r.stderr
    tasks = list((dest / "data" / "tasks").glob("*.md"))
    assert len(tasks) == 1
    assert "Restore me" in tasks[0].read_text()


def test_restore_in_place_requires_yes(cli, env, tmp_path):
    _setup(env, tmp_path)
    cli("add", "Original")
    cli("backup", "run")

    # Without --yes, an in-place restore must refuse and leave the vault untouched.
    r = cli("restore", "latest")
    assert r.returncode != 0
    assert "--yes" in (r.stderr + r.stdout)


def test_backup_disabled_without_config(cli, env, tmp_path):
    # Config present but no backup section -> friendly "disabled" error.
    cfg_dir = tmp_path / "cfg" / "mgmt"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text("statuses: []\n")
    env["XDG_CONFIG_HOME"] = str(tmp_path / "cfg")

    r = cli("backup", "run")
    assert r.returncode != 0
    assert "disabled" in (r.stderr + r.stdout)
