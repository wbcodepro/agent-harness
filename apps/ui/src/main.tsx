import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createLocalTransport, readToken } from "./transport";
import "./styles/global.css";

// The web build talks to the hame web server that served it. The desktop
// and mobile apps choose a transport the same way (local, or remote
// through the relay) before rendering.
const transport = createLocalTransport(readToken());

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App transport={transport} />
  </StrictMode>,
);
