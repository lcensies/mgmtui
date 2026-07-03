# mgmt dev tasks. Build/test run offline; `fetch` needs the networked pane.
default: run

run:
    cargo run -p mgmt-cli

build:
    cargo build --offline --workspace

# Install the `mgmt` binary to ~/.cargo/bin (ensure it is on PATH).
install:
    cargo install --offline --path crates/mgmt-cli

test:
    cargo test --offline --workspace

# Blackbox end-to-end tests (drive the real binary + TUI via a pty).
test-bb:
    python3 -m venv tests/blackbox/.venv
    tests/blackbox/.venv/bin/pip install -q -r tests/blackbox/requirements.txt
    tests/blackbox/.venv/bin/python -m pytest tests/blackbox -q

# Verify mgmt-tui never owns the terminal (embeddability invariant).
check-embed:
    @! grep -rnE 'enable_raw_mode|EnterAlternateScreen|event::read|event::poll' crates/mgmt-tui/src | grep -vE ':[0-9]+://' \
        && echo 'PASS: mgmt-tui is terminal-agnostic'

add task:
    cargo run -p mgmt-cli -- add "{{task}}"

sync target="":
    cargo run -p mgmt-cli -- sync {{target}}
