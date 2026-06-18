"""Blackbox TUI tests: spawn the real `mgmt` TUI in a pty and assert on the rendered screen."""

import datetime


def test_launch_shows_calendar_view(make_tui):
    t = make_tui()
    assert t.wait_for("Calendar"), t.text()
    assert "Mo Tu We Th Fr Sa Su" in t.text()
    assert "mgmt" in t.text()


def test_tabs_cycle_through_all_views(make_tui):
    t = make_tui()
    assert t.wait_for("Mo Tu We"), "calendar should be first"
    t.send("\t")
    assert t.wait_for("Todo ("), "second tab is the board"
    t.send("\t")
    assert t.wait_for("Tasks"), "third tab is the task list"
    t.send("\t")
    assert t.wait_for("FOCUS"), "fourth tab is the pomodoro timer"


def test_quick_add_task_appears_in_list(make_tui):
    t = make_tui()
    t.wait_for("Calendar")
    t.send("\t")  # Board
    t.send("\t")  # Tasks
    assert t.wait_for("Tasks")
    t.send("a")
    assert t.wait_for("New task"), "input prompt should open"
    for ch in "write the report":
        t.send(ch)
    t.send("\r")
    assert t.wait_for("write the report"), t.text()


def test_board_move_card_changes_columns(cli, make_tui):
    cli("add", "Ship release")
    t = make_tui()
    t.wait_for("Calendar")
    t.send("\t")  # Board
    assert t.wait_for("Ship release"), t.text()
    assert "Todo (1)" in t.text()
    t.send("L")  # move card to next column (Doing)
    assert t.wait_for("Doing (1)"), t.text()
    assert "Todo (0)" in t.text()


def test_calendar_shows_event_and_reschedules(cli, make_tui, tmp_path):
    today = datetime.date.today().strftime("%Y%m%d")
    ics = tmp_path / "today.ics"
    ics.write_text(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n"
        f"UID:today-1\r\nDTSTART:{today}T120000Z\r\nDTEND:{today}T130000Z\r\n"
        "SUMMARY:Strategy sync\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    cli("import", str(ics), "--calendar", "work")

    t = make_tui()
    assert t.wait_for("Strategy sync"), t.text()
    assert "12:00" in t.text()
    t.send("J")  # reschedule +15m
    assert t.wait_for("12:15"), t.text()


def test_help_overlay_opens(make_tui):
    t = make_tui()
    t.wait_for("Calendar")
    t.send("?")
    assert t.wait_for("Help"), t.text()
    assert "undo" in t.text()


def test_focus_timer_starts(make_tui):
    t = make_tui()
    t.wait_for("Calendar")
    t.send("\t")
    t.send("\t")
    t.send("\t")
    assert t.wait_for("25:00"), t.text()
    assert "paused" in t.text()
    t.send(" ")
    assert t.wait_for("running"), t.text()


def test_quit_exits_process(make_tui):
    t = make_tui()
    t.wait_for("Calendar")
    t.child.send("q")
    t.pump(0.6)
    assert not t.child.isalive(), "process should exit on q"


def _seed_event_today(cli, tmp_path, summary="Standup"):
    today = datetime.date.today().strftime("%Y%m%d")
    ics = tmp_path / "t.ics"
    ics.write_text(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n"
        f"UID:seed-1\r\nDTSTART:{today}T090000Z\r\nDTEND:{today}T100000Z\r\n"
        f"SUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    cli("import", str(ics), "--calendar", "work")


def test_create_event_with_times_via_form(make_tui):
    t = make_tui()
    t.wait_for("Mo Tu We")
    t.send("a")  # calendar 'a' opens the event form
    assert t.wait_for("New event"), t.text()
    for ch in "Lunch":
        t.send(ch)
    t.send("\r")  # -> Start (default 09:00)
    t.send("\r")  # -> End (default 10:00)
    t.send("\r")  # submit
    assert t.wait_for("Lunch"), t.text()
    assert "09:00" in t.text()


def test_calendar_view_cycles_week_and_day(cli, make_tui, tmp_path):
    _seed_event_today(cli, tmp_path)
    t = make_tui()
    assert t.wait_for("Standup")
    t.send("v")  # week
    assert t.wait_for("Mon"), t.text()  # week shows weekday column headers
    t.send("v")  # day
    assert t.wait_for("09:00-10:00"), t.text()  # day view shows start-end range


def test_agenda_focus_and_reschedule(cli, make_tui, tmp_path):
    _seed_event_today(cli, tmp_path)
    t = make_tui()
    assert t.wait_for("09:00")
    t.send("\r")  # focus agenda
    assert t.wait_for("[agenda]"), t.text()
    t.send("J")  # reschedule +15m
    assert t.wait_for("09:15"), t.text()


def test_project_picker_assigns_project(cli, make_tui):
    cli("add", "alpha", "--project", "home")
    cli("add", "beta")
    t = make_tui()
    t.wait_for("Calendar")
    t.send("\t")  # Board
    t.send("\t")  # Tasks
    assert t.wait_for("beta")
    t.send("p")  # open project picker
    assert t.wait_for("Project"), t.text()
    t.send("j")  # move to "home"
    t.send("\r")  # assign
    assert t.wait_for("#home"), t.text()


def test_edit_in_external_editor_reloads(cli, env, make_tui, tmp_path):
    cli("add", "editable")
    # a non-interactive fake editor that rewrites the task title. Set both EDITOR and VISUAL
    # so the test is robust regardless of which the host prefers or what the shell inherited.
    fake = tmp_path / "fakeeditor.sh"
    fake.write_text("#!/bin/sh\nsed -i 's/^title: .*/title: EDITED/' \"$1\"\n")
    fake.chmod(0o755)
    env["EDITOR"] = str(fake)
    env["VISUAL"] = str(fake)

    t = make_tui()
    t.wait_for("Calendar")
    t.send("\t")  # Board
    t.send("\t")  # Tasks
    assert t.wait_for("editable")
    t.send("e")  # open in $EDITOR (suspends TUI, runs fake editor, reloads)
    assert t.wait_for("EDITED"), t.text()

