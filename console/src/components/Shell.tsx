import type { MouseEvent, ReactNode } from "react";
import { ConsoleShell, Dot } from "@momoi-labs/kiso-react";

import { logout } from "../lib/api.js";
import { useTheme } from "../lib/theme.js";
import type { FailingWorkload } from "../lib/types.js";
import { Icon } from "./Icon.js";
import { BrandSymbol, Lockup } from "./Logo.js";

/** Where a sidebar entry points, and how it knows it is the current one. */
export type Destination = {
  href: string;
  active: boolean;
  onClick?: () => void;
};

/**
 * The console's frame: the sidebar, the breadcrumb, the health status and
 * the account menu.
 */
export function Shell({
  crumb,
  parent,
  dnsSuffix,
  healthy,
  degraded = false,
  failing = [],
  onOpenFailing,
  version,
  overview,
  databases = { href: "/console/#databases", active: false },
  customImages = { href: "/console/#custom-images", active: false },
  routes = { href: "/console/#routes", active: false },
  dns = { href: "/console/#dns", active: false },
  events = { href: "/console/#events", active: false },
  host,
  settings = { href: "/console/#settings", active: false },
  children,
}: {
  crumb: string;
  /** The list the current screen belongs to, linked between Console and it. */
  parent?: Destination & { label: string };
  dnsSuffix: string | null;
  healthy: boolean;
  /** The daemon answers but a workload is down; the line names it and opens it. */
  degraded?: boolean;
  failing?: FailingWorkload[];
  onOpenFailing?: (id: string) => void;
  version: string | null;
  overview: Destination;
  databases?: Destination;
  customImages?: Destination;
  routes?: Destination;
  events?: Destination;
  dns?: Destination;
  /** Absent while the Operator has allowed nothing on the Host. */
  host?: Destination;
  settings?: Destination;
  children: ReactNode;
}) {
  const [theme, setTheme] = useTheme();

  const versionLabel = version ? <span className="muted mono">v{version}</span> : null;

  return (
    <ConsoleShell
      brandMark={<BrandSymbol />}
      brand={
        <span className="console-brand">
          <Lockup />
          {dnsSuffix ? <span className="console-dns t-metadata muted truncate">{dnsSuffix}</span> : null}
        </span>
      }
      navigation={[
        {
          destinations: [
            {
              ...overview,
              label: "Overview",
              leading: <Icon name="chart" />,
              trailing: degraded ? <><Dot variant="warning" pulse /><span className="offscreen">Degraded</span></> : undefined,
            },
            { ...databases, label: "Databases", leading: <Icon name="database" /> },
            { ...routes, label: "Routes", leading: <Icon name="route" /> },
            { ...dns, label: "DNS", leading: <Icon name="globe" /> },
            { ...events, label: "Events", leading: <Icon name="history" /> },
            ...(host ? [{ ...host, label: "Host", leading: <Icon name="screen" /> }] : []),
            { ...customImages, label: "Custom images", leading: <Icon name="box" /> },
          ],
        },
        { destinations: [{ ...settings, label: "Settings", leading: <Icon name="gear" /> }] },
      ]}
      breadcrumb={[
        { label: dnsSuffix ?? "Console", href: "/console/" },
        ...(parent
          ? [{
              label: parent.label,
              href: parent.href,
              onClick: (event: MouseEvent<HTMLAnchorElement>) => {
                if (!parent.onClick || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                event.preventDefault();
                parent.onClick();
              },
            }]
          : []),
        { label: crumb },
      ]}
      status={degraded ? (
        <button
          type="button"
          className="health-link"
          title={failing.length ? `The daemon answers, but ${failedLabel(failing).toLowerCase()}. Open it.` : "The daemon answers, but some workloads are down."}
          disabled={!failing.length || !onOpenFailing}
          onClick={() => { if (failing[0]) onOpenFailing?.(failing[0].id); }}
        >
          <Dot variant="warning" pulse />
          <span>{failedLabel(failing)}</span>
          {versionLabel}
        </button>
      ) : (
        <span className="health-link" title="The daemon answering on this Host">
          <Dot variant={healthy ? "success" : "neutral"} />
          <span>{healthy ? "Healthy" : "Checking health…"}</span>
          {versionLabel}
        </span>
      )}
      account={{
        identity: "Admin",
        theme,
        onThemeChange: setTheme,
        onSignOut: logout,
      }}
    >
      {children}
    </ConsoleShell>
  );
}

/** "1 database failed", "2 applications and 1 database failed". */
function failedLabel(failing: FailingWorkload[]): string {
  const applications = failing.filter((workload) => workload.kind === "application").length;
  const databases = failing.filter((workload) => workload.kind === "database").length;
  const parts = [
    applications ? `${applications} ${applications === 1 ? "application" : "applications"}` : null,
    databases ? `${databases} ${databases === 1 ? "database" : "databases"}` : null,
  ].filter(Boolean);
  return parts.length ? `${parts.join(" and ")} failed` : "Degraded";
}
