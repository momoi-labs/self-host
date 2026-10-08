import { useEffect } from "react";
import {
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  PaneGrid,
} from "@momoi-labs/kiso-react";

import { GitConnectionsSettings } from "../components/GitConnections.js";
import { useStoredLayout } from "../lib/useStoredLayout.js";
import { ApiKeys } from "./settings/ApiKeys.js";
import { DnsSetup, YourHost } from "./settings/DnsSetup.js";
import { ApplicationDefaults, AuditHistory } from "./settings/General.js";

export type SettingsSection = "general" | "api-keys" | "dns-setup" | "git-connections";

export const settingsSections: readonly SettingsSection[] = ["general", "api-keys", "dns-setup", "git-connections"];

export function isSettingsSection(value: string): value is SettingsSection {
  return (settingsSections as readonly string[]).includes(value);
}

/** The pane a section's link scrolls to. */
const paneFor: Record<SettingsSection, string> = {
  general: "audit-history",
  "api-keys": "api-keys",
  "dns-setup": "dns-setup",
  "git-connections": "git-connections",
};

/**
 * Everything the Operator configures about the Platform itself, one pane per
 * group, packed so a short pane does not wait for the tallest one beside it.
 * The Operator can move and resize the panes, and this browser keeps the
 * arrangement. The URL fragment names a section, so a link scrolls to its
 * pane.
 */
export function Settings({ section }: { section: SettingsSection }) {
  const [layout, saveLayout] = useStoredLayout("settings");
  useEffect(() => {
    if (section === "general") return;
    const card = document.querySelector<HTMLElement>(`[data-pane-id="${paneFor[section]}"]`);
    const grid = card?.parentElement;
    if (!card || !grid) return;
    // The cards load their data after this runs and the page grows under the
    // scroll, so the card is brought back each time the grid grows, until the
    // page settles or the Operator scrolls on their own.
    const observer = new ResizeObserver(() => card.scrollIntoView({ block: "start" }));
    observer.observe(grid);
    const stop = () => observer.disconnect();
    const timer = window.setTimeout(stop, 2000);
    const events = ["wheel", "touchstart", "keydown"] as const;
    for (const event of events) window.addEventListener(event, stop, { once: true });
    return () => {
      stop();
      window.clearTimeout(timer);
      for (const event of events) window.removeEventListener(event, stop);
    };
  }, [section]);

  return (
    <div className="stack">
      <PageHeader>
        <PageHeaderTitle>Settings</PageHeaderTitle>
        <PageHeaderDescription>How the Platform runs, who can reach it, and how devices find it.</PageHeaderDescription>
      </PageHeader>
      <PaneGrid aria-label="Settings" flow="masonry" defaultLayout={layout} onLayoutChange={saveLayout}>
        <AuditHistory id="audit-history" size={6} />
        <ApplicationDefaults id="applications" size={6} />
        <YourHost id="your-host" size={6} />
        <ApiKeys id="api-keys" size={12} />
        <GitConnectionsSettings id="git-connections" size={12} />
        <DnsSetup id="dns-setup" size={12} />
      </PaneGrid>
    </div>
  );
}
