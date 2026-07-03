#!/usr/bin/env bash
# Seed a fresh, self-contained demo dataset via the `mgmt` CLI.
#
# Everything is created from scratch (no host data is read). All dates are computed RELATIVE to
# the day the container runs, so the calendar's "today" / "this week" / "next 7 days" always
# line up regardless of when you record. Data lands in $XDG_DATA_HOME/mgmt (events as an .ics
# vdir, tasks/projects as markdown), which the TUI then reads. Projects auto-register the first
# time a task references them, and are coloured by config.yaml.
set -euo pipefail

# --- date helpers (GNU date) -------------------------------------------------------------
D() { date -d "$1" +%F; }                  # YYYY-MM-DD for a relative expression
TODAY=$(date +%F)
DOW=$(date +%u)                            # 1=Mon .. 7=Sun
MON=$(date -d "-$((DOW - 1)) days" +%F)    # Monday of the current week
after() { date -d "$MON +$1 days" +%F; }   # a date N days after this week's Monday

WEEKDAYS="FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"

echo "  data root : ${XDG_DATA_HOME}/mgmt"
echo "  today     : ${TODAY}  (week of ${MON})"

# --- calendar events ---------------------------------------------------------------------
# Recurring daily stand-up across the work week.
mgmt event add "Daily stand-up" --calendar work \
  --start "$MON 09:30" --duration 15 --rrule "$WEEKDAYS" \
  --location "Zoom" --project work >/dev/null

# Today's meetings.
mgmt event add "1:1 with Dana"    --calendar work --start "$TODAY 11:00" --duration 30 \
  --location "Zoom"      --project work >/dev/null
mgmt event add "Lunch with Priya" --calendar home --start "$TODAY 13:00" --duration 60 \
  --location "Cafe Loup" --project home >/dev/null
mgmt event add "Design sync"      --calendar work --start "$TODAY 15:00" --duration 60 \
  --location "Studio B"  --project work >/dev/null

# Recurring gym sessions (Tue/Thu evenings) and weekly sprint planning.
mgmt event add "Gym session" --calendar home \
  --start "$(after 1) 18:00" --duration 60 \
  --rrule "FREQ=WEEKLY;BYDAY=TU,TH" --project health >/dev/null
mgmt event add "Sprint planning" --calendar work \
  --start "$(after 7) 10:00" --duration 60 \
  --rrule "FREQ=WEEKLY;BYDAY=MO" --location "War room" --project work >/dev/null

# An all-day team offsite next week and a one-off dentist appointment.
mgmt event add "Team offsite" --calendar work --start "$(after 9)" --all-day --project work >/dev/null
mgmt event add "Dentist" --calendar home --start "$(D '+6 days') 14:00" --duration 45 \
  --location "Bright Smile Clinic" --project home >/dev/null

# --- tasks (spread across statuses / projects / priorities / due dates) -------------------
# Todo
mgmt task add "Write Q3 roadmap"             --project work    --priority high   --due "$(D '+3 days') 17:00" >/dev/null
mgmt task add "Review pull requests"         --project work    --priority medium --due "$TODAY 17:00"         >/dev/null
mgmt task add "Buy groceries"                --project home    --priority low    --due "$TODAY 19:00"         >/dev/null
mgmt task add "Draft blog: local-first apps" --project writing --priority medium --due "$(D '+5 days') 12:00" >/dev/null
mgmt task add "Renew gym membership"         --project health                    --due "$(D '+6 days') 09:00" >/dev/null
# Inbox (no project)
mgmt task add "Call the dentist back"                                            --due "$(D '+2 days') 10:00" >/dev/null
mgmt task add "Plan weekend hike"                                                                             >/dev/null
# Doing
mgmt task add "Implement CalDAV sync"        --project work    --priority high   --status doing              >/dev/null
mgmt task add "Refactor storage layer"       --project work    --priority medium --status doing              >/dev/null
# Blocked
mgmt task add "Ship v0.2 release"            --project work    --priority high   --status blocked            >/dev/null
# Done
mgmt task add "Set up CI pipeline"           --project work                      --status done               >/dev/null
mgmt task add "Morning run"                  --project health                    --status done               >/dev/null
mgmt task add "Read DDIA, chapter 4"         --project writing                   --status done               >/dev/null

echo "  seeded $(mgmt task list 2>/dev/null | wc -l) tasks and $(mgmt event list 2>/dev/null | wc -l) event series"
