import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createLocalTransport, readToken } from "./transport";
import { desktop, startDesktop } from "./desktop";
import "./styles/global.css";

// The web build talks to the hame web server that served it. The desktop
// app serves it from its own in-process server and hands over the token
// itself, never in the address. The mobile apps choose a transport the same
// way (local, or remote through the relay) before rendering.
const transport = createLocalTransport(desktop?.token ?? readToken());
startDesktop();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App transport={transport} />
  </StrictMode>,
);
