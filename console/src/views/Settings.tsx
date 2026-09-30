import { useEffect } from "react";
import {
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
} from "@momoi-labs/kiso-react";

import { Masonry } from "../components/Masonry.js";
import { ApiKeys } from "./settings/ApiKeys.js";
import { DnsSetup, YourHost } from "./settings/DnsSetup.js";
import { General } from "./settings/General.js";

export type SettingsSection = "general" | "api-keys" | "dns-setup";

export const settingsSections: readonly SettingsSection[] = ["general", "api-keys", "dns-setup"];

export function isSettingsSection(value: string): value is SettingsSection {
  return (settingsSections as readonly string[]).includes(value);
}

/**
 * Everything the Operator configures about the Platform itself, one card per
 * group, packed to fill the screen. The URL fragment names a section, so a
 * link scrolls to its card.
 */
export function Settings({ section }: { section: SettingsSection }) {
  useEffect(() => {
    if (section === "general") return;
    const card = document.getElementById(`settings-${section}`);
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
      <Masonry>
        <General />
        <YourHost />
        <ApiKeys />
        <DnsSetup />
      </Masonry>
    </div>
  );
}
