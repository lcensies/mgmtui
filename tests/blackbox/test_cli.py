"""Blackbox tests for the non-interactive `mgmt` subcommands."""

import glob
import os


def test_add_creates_markdown_task(cli, data_dir):
    r = cli("add", "Buy oat milk", "--project", "home")
    assert r.returncode == 0, r.stderr
    assert "added task" in r.stdout

    files = glob.glob(str(data_dir / "tasks" / "*.md"))
    assert len(files) == 1
    body = open(files[0]).read()
    assert "title: Buy oat milk" in body
    assert "project: home" in body
    assert "status: todo" in body


def test_import_then_export_roundtrips_event(cli, data_dir, tmp_path):
    ics = tmp_path / "ev.ics"
    ics.write_text(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"
        "BEGIN:VEVENT\r\nUID:meeting-1\r\n"
        "DTSTART:20260618T140000Z\r\nDTEND:20260618T150000Z\r\n"
        "SUMMARY:Design review\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    r = cli("import", str(ics), "--calendar", "work")
    assert r.returncode == 0, r.stderr
    assert "imported 1 event" in r.stdout

    out = cli("export").stdout
    assert "SUMMARY:Design review" in out
    assert "UID:meeting-1" in out
    # mgmt-private sync props must never leak into exported (server-bound) ics
    assert "X-MGMT" not in out


def test_export_filters_by_calendar(cli, data_dir, tmp_path):
    for cal, uid in [("work", "w1"), ("home", "h1")]:
        ics = tmp_path / f"{uid}.ics"
        ics.write_text(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n"
            f"UID:{uid}\r\nDTSTART:20260618T140000Z\r\nDTEND:20260618T150000Z\r\n"
            f"SUMMARY:{cal} event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
        cli("import", str(ics), "--calendar", cal)

    out = cli("export", "--calendar", "work").stdout
    assert "UID:w1" in out
    assert "UID:h1" not in out


def test_sync_without_config_is_friendly(cli):
    r = cli("sync")
    assert r.returncode == 0
    assert "no collections configured" in r.stdout


def test_add_rejects_empty_title(cli):
    r = cli("add", "   ")
    assert r.returncode != 0
