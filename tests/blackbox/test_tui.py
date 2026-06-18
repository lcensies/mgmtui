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
