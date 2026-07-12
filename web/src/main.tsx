import { render } from "preact";
import { LocationProvider, Router, Route } from "preact-iso";
import { registerSW } from "virtual:pwa-register";
import "./styles.css";
import { App } from "./app";
import { Calendar } from "./pages/Calendar";
import { Board } from "./pages/Board";
import { Tasks } from "./pages/Tasks";
import { Focus } from "./pages/Focus";
import { Invite } from "./pages/Invite";

function Root() {
  return (
    <LocationProvider>
      <App>
        <Router>
          <Route path="/" component={Calendar} />
          <Route path="/board" component={Board} />
          <Route path="/tasks" component={Tasks} />
          <Route path="/focus" component={Focus} />
          <Route path="/invite" component={Invite} />
        </Router>
      </App>
    </LocationProvider>
  );
}

render(<Root />, document.getElementById("app")!);

// Auto-update the PWA: without explicit update checks a service-worker-cached shell can stay
// stale for a long time (only re-checked on hard navigations), so poll hourly and whenever the
// tab regains visibility. registerType is autoUpdate, so a found update activates on its own.
const updateSW = registerSW({
  immediate: true,
  onRegisteredSW(_url, reg) {
    if (!reg) return;
    setInterval(() => void reg.update(), 60 * 60 * 1000);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") void reg.update();
    });
  },
  onNeedRefresh() {
    void updateSW(true);
  },
});
