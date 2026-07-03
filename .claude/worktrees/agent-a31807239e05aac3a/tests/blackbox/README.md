# Blackbox tests

End-to-end tests that drive the real `mgmt` binary — no internal APIs.

- `test_cli.py` — runs `mgmt add/import/export/sync` as subprocesses and checks output + files.
- `test_tui.py` — spawns the TUI in a pseudo-terminal (`pexpect`), parses the rendered screen
  with a VT emulator (`pyte`), sends keystrokes, and asserts on what's drawn: tab navigation,
  quick-add, kanban card moves, calendar event display + reschedule, help overlay, pomodoro
  start, and clean quit.

## Run

```bash
python3 -m venv tests/blackbox/.venv
tests/blackbox/.venv/bin/pip install -r tests/blackbox/requirements.txt
tests/blackbox/.venv/bin/python -m pytest tests/blackbox -q
```

The session fixture builds `mgmt-cli` (`--offline`) before the tests run. Each test gets an
isolated `HOME` and `--data-dir`, so nothing touches real config or data.
