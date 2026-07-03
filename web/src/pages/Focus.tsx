import { useEffect, useState } from "preact/hooks";
import { api, type PomodoroWire } from "../api";

function clock(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${pad(m)}:${pad(sec)}`;
}

export function Focus() {
  const [wire, setWire] = useState<PomodoroWire>({});
  const [nowSec, setNowSec] = useState(() => Date.now() / 1000);

  // Poll the server for state changes; animate locally every second.
  useEffect(() => {
    let alive = true;
    const load = () => api.status().then((w) => alive && setWire(w)).catch(() => {});
    load();
    const poll = setInterval(load, 15000);
    const tick = setInterval(() => setNowSec(Date.now() / 1000), 1000);
    return () => {
      alive = false;
      clearInterval(poll);
      clearInterval(tick);
    };
  }, []);

  async function act(action: string, engine?: string) {
    setWire(await api.focus(action, engine));
  }

  const p = wire.pomodoro;
  let display = "—";
  let phase = "idle";
  if (p) {
    phase = p.phase + (p.running ? "" : " · paused");
    if (p.running && p.ends_at !== undefined) display = clock(p.ends_at - nowSec);
    else if (p.running && p.count_from !== undefined) display = clock(nowSec - p.count_from);
    else if (p.remaining !== undefined) display = clock(p.remaining);
    else if (p.elapsed !== undefined) display = clock(p.elapsed);
  }

  return (
    <div class="timer">
      <div class="phase">{phase}</div>
      <div class="clock">{display}</div>
      {wire.next_event && (
        <div class="muted">Next: {wire.next_event.summary}</div>
      )}
      <div class="controls">
        {!p ? (
          <>
            <button class="primary" onClick={() => act("start")}>
              Start pomodoro
            </button>
            <button onClick={() => act("start", "flowtime")}>Flowtime</button>
          </>
        ) : (
          <>
            <button onClick={() => act("toggle")}>{p.running ? "Pause" : "Resume"}</button>
            <button onClick={() => act("skip")}>Skip</button>
            <button onClick={() => act("stop")}>Stop</button>
          </>
        )}
      </div>
    </div>
  );
}
