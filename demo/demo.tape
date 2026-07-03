# =============================================================================
#  mgmt — feature demo storyboard for VHS  (https://github.com/charmbracelet/vhs)
#  Rendered headless inside Docker (ttyd + chromium + ffmpeg). Produces one video
#  (mp4 / webm / gif) plus per-scene PNG stills. Output files are written to the
#  current directory (the entrypoint cd's into /demo/out before running vhs).
# =============================================================================

Output "mgmt-demo.mp4"
Output "mgmt-demo.webm"
Output "mgmt-demo.gif"

Require mgmt

Set Shell        "bash"
Set FontFamily   "DejaVu Sans Mono"
Set FontSize     16
Set Width        1280
Set Height       800
Set Padding      18
Set Margin       16
Set MarginFill   "#181825"
Set BorderRadius 10
Set WindowBar    Colorful
Set Theme        "Catppuccin Mocha"
Set TypingSpeed  45ms
Set Framerate    24

# ---- clean, branded prompt (not recorded) ----------------------------------
Hide
Type `export PS1="mgmt \[\e[36m\]❯\[\e[0m\] "`
Enter
Type "clear"
Enter
Sleep 800ms
Show

# =============================================================================
#  ACT 1 — the scriptable CLI
# =============================================================================
Type "mgmt --help"
Enter
Sleep 2.8s
Screenshot "01-cli-help.png"

# Quick-capture a task and a meeting straight from the shell.
Type "mgmt task add 'Prepare launch deck' --project work --priority high"
Sleep 500ms
Enter
Sleep 1.3s

Type `mgmt event add "Investor call" --start "$(date -d today +%F) 16:30" --duration 30 --calendar work`
Sleep 500ms
Enter
Sleep 1.3s

Type "mgmt task list"
Enter
Sleep 3s
Screenshot "02-cli-list.png"

Hide
Type "clear"
Enter
Sleep 400ms
Show

# =============================================================================
#  ACT 2 — the TUI: Calendar
# =============================================================================
Type "mgmt"
Enter
Sleep 2.2s
Screenshot "03-calendar-month.png"

# Cycle month -> week -> day -> month.
Type "v"
Sleep 2s
Screenshot "04-calendar-week.png"
Type "v"
Sleep 2s
Screenshot "05-calendar-day.png"
Type "v"
Sleep 1.6s

# Drop into today's agenda; move the cursor and reschedule a meeting live.
Enter
Sleep 1.3s
Type "j"
Sleep 700ms
Type "j"
Sleep 800ms
Screenshot "06-agenda.png"
Type "L"
Sleep 900ms
Type "J"
Sleep 1.1s
Screenshot "07-agenda-reschedule.png"
Enter
Sleep 900ms

# Create a recurring event through the in-TUI form.
Type "a"
Sleep 1.1s
Type "Design Review"
Sleep 500ms
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 300ms
Type "Studio B"
Sleep 400ms
Tab
Sleep 300ms
Type "work"
Sleep 400ms
Tab
Sleep 400ms
Right
Sleep 500ms
Right
Sleep 700ms
Screenshot "08-event-form.png"
Tab
Sleep 300ms
Type "Quarterly design review"
Sleep 500ms
Enter
Sleep 1.6s
Screenshot "09-event-created.png"

# =============================================================================
#  ACT 3 — the TUI: Board (kanban over task status)
# =============================================================================
Tab
Sleep 1.8s
Screenshot "10-board.png"

Type "j"
Sleep 700ms
Type "L"
Sleep 1.1s
Type "L"
Sleep 1.1s
Screenshot "11-board-moved.png"
Type "H"
Sleep 1s
Type "P"
Sleep 1s
Space
Sleep 1.4s
Screenshot "12-board-done.png"

# =============================================================================
#  ACT 4 — the TUI: Tasks (smart lists + projects + search)
# =============================================================================
Tab
Sleep 1.6s
Screenshot "13-tasks.png"

Type "h"
Sleep 700ms
Type "j"
Sleep 1s
Type "j"
Sleep 1s
Screenshot "14-tasks-next7.png"
Type "j"
Sleep 800ms
Type "j"
Sleep 900ms
Type "j"
Sleep 1.1s
Screenshot "15-tasks-project.png"
Type "k"
Sleep 600ms
Type "k"
Sleep 600ms
Type "l"
Sleep 700ms
Type "s"
Sleep 1.1s
Type "/"
Sleep 500ms
Type "roadmap"
Sleep 900ms
Enter
Sleep 1.5s
Screenshot "16-tasks-search.png"

# =============================================================================
#  ACT 5 — the TUI: Focus (pomodoro / flowtime)
# =============================================================================
Tab
Sleep 1.6s
Screenshot "17-focus.png"
Space
Sleep 3.8s
Screenshot "18-focus-running.png"
Type "s"
Sleep 2s
Screenshot "19-focus-break.png"

# =============================================================================
#  ACT 6 — command palette + help, then quit
# =============================================================================
Type ":"
Sleep 900ms
Type "cal"
Sleep 1.1s
Screenshot "20-palette.png"
Enter
Sleep 1.3s

Type "?"
Sleep 3s
Screenshot "21-help.png"
Escape
Sleep 800ms

Type "q"
Sleep 1s
Type "# mgmt — local-first calendar + markdown tasks + kanban, all in the terminal"
Sleep 2.5s
