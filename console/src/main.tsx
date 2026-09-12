import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App.js";
import { Toasts } from "@momoi-labs/kiso-react";
import { api, logout, requireKey } from "./lib/api.js";
import "./console.css";

/*
 * A key in session storage is not proof it still works: the Platform may have
 * been reset since. Ask once, and send a rejected key back to the login page
 * rather than rendering a console that cannot load anything.
 */
async function start() {
  // PROTOTYPE (#107): the variants need no Host, so skip the login gate.
  const prototype = import.meta.env.DEV && location.search.includes("variant=");
  if (prototype) sessionStorage.setItem("api_key", "prototype");
  if (!prototype && !requireKey()) return;
  if (!prototype) try {
    const res = await api("/health");
    if (!res.ok) {
      logout();
      return;
    }
  } catch {
    // The Host is unreachable, not the key's fault. Render, and let each
    // request report its own failure.
  }

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <Toasts>
        <App />
      </Toasts>
    </StrictMode>,
  );
}

void start();
