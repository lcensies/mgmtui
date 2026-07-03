import { render } from "preact";
import { LocationProvider, Router, Route } from "preact-iso";
import "./styles.css";
import { App } from "./app";
import { Calendar } from "./pages/Calendar";
import { Board } from "./pages/Board";
import { Tasks } from "./pages/Tasks";
import { Focus } from "./pages/Focus";

function Root() {
  return (
    <LocationProvider>
      <App>
        <Router>
          <Route path="/" component={Calendar} />
          <Route path="/board" component={Board} />
          <Route path="/tasks" component={Tasks} />
          <Route path="/focus" component={Focus} />
        </Router>
      </App>
    </LocationProvider>
  );
}

render(<Root />, document.getElementById("app")!);
