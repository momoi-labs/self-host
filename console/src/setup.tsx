import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { DeviceSetup } from "./device-setup.js";
import "./console.css";

/*
 * Over plain HTTP at /setup this is the page a device sees before it trusts
 * the CA or resolves the DNS Suffix. The Operator's own DNS setup lives in the
 * console's Settings now, so the old /console/setup.html address goes there.
 */
if (window.location.protocol === "http:" && window.location.pathname === "/setup") {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <DeviceSetup />
    </StrictMode>,
  );
} else {
  window.location.replace("/console/#settings/dns-setup");
}
