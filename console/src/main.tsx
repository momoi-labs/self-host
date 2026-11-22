import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";

import { App } from "./App.js";
import { Toasts } from "@momoi-labs/kiso-react";
import { api } from "./lib/api.js";
import { queryClient } from "./lib/queryClient.js";
import "./console.css";

// The Host's screen opens in a window of its own, without the console around
// it, and noVNC is only fetched there.
const HostScreen = lazy(() => import("./components/HostScreen.js").then((module) => ({ default: module.HostScreen })));

async function start() {
  try {
    const res = await api("/whoami");
    if (res.status === 401 || res.status === 403) {
      window.location.replace("/console");
      return;
    }
  } catch {
    // The Host is unreachable, not the key's fault. Render, and let each
    // request report its own failure.
  }

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <Toasts>
          {location.hash === "#host-screen" ? <Suspense fallback={null}><HostScreen /></Suspense> : <App />}
        </Toasts>
      </QueryClientProvider>
    </StrictMode>,
  );
}

void start();
