# mgmt dev tasks. Build/test run offline; `fetch` needs the networked pane.
default: run

run:
    cargo run -p mgmt-cli

build:
    cargo build --offline --workspace

test:
    cargo test --offline --workspace

# Verify mgmt-tui never owns the terminal (embeddability invariant).
check-embed:
    @! grep -rnE 'enable_raw_mode|EnterAlternateScreen|event::read|event::poll' crates/mgmt-tui/src | grep -vE ':[0-9]+://' \
        && echo 'PASS: mgmt-tui is terminal-agnostic'

add task:
    cargo run -p mgmt-cli -- add "{{task}}"

sync target="":
    cargo run -p mgmt-cli -- sync {{target}}
