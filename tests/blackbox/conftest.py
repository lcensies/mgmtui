"""Shared fixtures for the mgmt blackbox tests.

These drive the real `mgmt` binary end-to-end: the CLI tests run it as a subprocess, and the
TUI tests spawn it in a pseudo-terminal and assert on the rendered screen (parsed with pyte).
"""

import os
import shutil
import subprocess
import time
from pathlib import Path

import pexpect
import pyte
import pytest

REPO = Path(__file__).resolve().parents[2]
BIN = REPO / "target" / "debug" / "mgmt"

ROWS, COLS = 40, 120


@pytest.fixture(scope="session")
def mgmt_bin():
    """Build the binary once per session and return its path."""
    subprocess.run(
        ["cargo", "build", "--offline", "-p", "mgmt-cli"],
        cwd=REPO,
        check=True,
    )
    assert BIN.exists(), f"binary not found at {BIN}"
    return str(BIN)


@pytest.fixture
def env(tmp_path):
    """A clean environment with an isolated HOME so no real config is read."""
    e = dict(os.environ)
    home = tmp_path / "home"
    home.mkdir()
    e["HOME"] = str(home)
    e.pop("XDG_CONFIG_HOME", None)
    e.pop("XDG_DATA_HOME", None)
    e["TERM"] = "xterm-256color"
    return e


@pytest.fixture
def data_dir(tmp_path):
    d = tmp_path / "data"
    d.mkdir()
    return d


def run_cli(mgmt_bin, env, data_dir, *args, **kwargs):
    """Invoke a non-TUI mgmt subcommand and return the CompletedProcess."""
    return subprocess.run(
        [mgmt_bin, "--data-dir", str(data_dir), *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
        **kwargs,
    )


class Tui:
    """A pseudo-terminal wrapper around the mgmt TUI, with a pyte screen for assertions."""

    def __init__(self, mgmt_bin, env, data_dir):
        self.screen = pyte.Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.child = pexpect.spawn(
            mgmt_bin,
            ["--data-dir", str(data_dir), "tui"],
            env=env,
            dimensions=(ROWS, COLS),
            encoding=None,
            timeout=10,
        )
        self.pump(0.5)

    def pump(self, duration=0.3):
        """Drain the pty for `duration` seconds, feeding bytes into the screen."""
        end = time.time() + duration
        while time.time() < end:
            try:
                data = self.child.read_nonblocking(8192, timeout=0.1)
                self.stream.feed(data)
            except pexpect.TIMEOUT:
                continue
            except pexpect.EOF:
                break

    def text(self):
        return "\n".join(self.screen.display)

    def send(self, keys):
        self.child.send(keys)
        self.pump(0.35)

    def wait_for(self, substr, timeout=6):
        end = time.time() + timeout
        while time.time() < end:
            if substr in self.text():
                return True
            self.pump(0.25)
        return False

    def close(self):
        try:
            self.child.send("q")
            self.pump(0.3)
            self.child.close(force=True)
        except Exception:
            pass


@pytest.fixture
def tui(mgmt_bin, env, data_dir):
    t = Tui(mgmt_bin, env, data_dir)
    yield t
    t.close()


@pytest.fixture
def make_tui(mgmt_bin, env, data_dir):
    """Factory that launches the TUI on demand (so tests can seed data first)."""
    created = []

    def _make():
        t = Tui(mgmt_bin, env, data_dir)
        created.append(t)
        return t

    yield _make
    for t in created:
        t.close()


# Expose helpers to test modules.
@pytest.fixture
def cli(mgmt_bin, env, data_dir):
    def _run(*args, **kwargs):
        return run_cli(mgmt_bin, env, data_dir, *args, **kwargs)

    return _run
