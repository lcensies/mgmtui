# mgmt — dockerized demo recorder

Builds `mgmt` from source and records a single terminal **video** (plus per-scene PNG stills)
showing the whole workflow — CLI capture, calendar, kanban board, task smart-lists, and the
pomodoro focus timer — in a fully isolated container. No data from your machine is read; the
demo dataset is generated from scratch with dates relative to the day you run it.

## Run it

```bash
./demo/run.sh            # builds the image (first time) and records
./demo/run.sh --rebuild  # force a clean rebuild
```

Artifacts land in `demo/out/`:

| File | What |
|------|------|
| `mgmt-demo.mp4` | the demo video (primary) |
| `mgmt-demo.webm` | same, web-friendly |
| `mgmt-demo.gif` | same, for embedding in markdown |
| `NN-*.png` | a still from each scene (CLI, calendar, board, tasks, focus, …) |

`run.sh` uses `docker`; it works unchanged with `podman` (`DOCKER=podman ./demo/run.sh`).

## How it works

- **`Dockerfile`** — two stages on Debian *bookworm* (matched glibc):
  1. `rust:slim-bookworm` builds the release `mgmt` binary.
  2. `debian:bookworm-slim` installs the headless recording stack — **VHS** (drives the
     terminal), **ttyd** (the PTY web terminal), **chromium** (renders frames), **ffmpeg**
     (encodes) — and copies the binary in.
- **`seed.sh`** — creates the demo events/tasks/projects via the `mgmt` CLI into a private
  `$XDG_DATA_HOME`, with all dates computed relative to "today".
- **`config.yaml`** — the demo's statuses (kanban columns), project colours, theme, and smart
  lists. Copied to `$XDG_CONFIG_HOME/mgmt/config.yaml`.
- **`demo.tape`** — the [VHS](https://github.com/charmbracelet/vhs) storyboard (keystrokes +
  timing) that drives the CLI and the TUI and declares the outputs.
- **`entrypoint.sh`** — seeds, runs `vhs demo.tape`, copies `out/` to the mounted volume.

## Tweaking

Edit `demo.tape` (keys/timing/scenes) or `seed.sh` (dataset) and re-run `./demo/run.sh` — the
Rust build stage is cached, so iterating on the storyboard is fast. Recording size/quality:
change `Set Width/Height/FontSize/Framerate` at the top of `demo.tape`.
