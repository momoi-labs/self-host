import { useState, type ReactNode } from "react";
import {
  ApplicationShell,
  BrandMark,
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
  Button,
  Dot,
  TerminalIcon,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { logout } from "../lib/api.js";
import { statusTone } from "../lib/status.js";
import { useTheme } from "../lib/theme.js";
import type { App, Environment, FailingWorkload } from "../lib/types.js";
import { CreateResource } from "./CreateResource.js";
import { Icon } from "./Icon.js";

/** Where a sidebar entry points, and how it knows it is the current one. */
export type Destination = {
  href: string;
  active: boolean;
  onClick?: () => void;
};

/**
 * The console's frame: the sidebar, the breadcrumb and the health indicator.
 */
export function Shell({
  crumb,
  parent,
  dnsSuffix,
  apps,
  environments,
  healthy,
  degraded = false,
  failing = [],
  onOpenFailing,
  version,
  overview,
  deploy,
  newMachine = { href: "/console/#new-virtual-machine", active: false },
  databases = { href: "/console/#databases", active: false },
  newDatabase = { href: "/console/#new-database", active: false },
  customImages = { href: "/console/#custom-images", active: false },
  routes = { href: "/console/#routes", active: false },
  dns = { href: "/console/#dns", active: false },
  events = { href: "/console/#events", active: false },
  host,
  settings = { href: "/console/#settings", active: false },
  application,
  virtualMachine,
  children,
}: {
  crumb: string;
  /** The list the current screen belongs to, linked between Console and it. */
  parent?: Destination & { label: string };
  dnsSuffix: string;
  apps: App[];
  environments: Environment[];
  healthy: boolean;
  /** The daemon answers but a workload is down; the line names it and opens it. */
  degraded?: boolean;
  failing?: FailingWorkload[];
  onOpenFailing?: (id: string) => void;
  version: string | null;
  overview: Destination;
  deploy: Destination;
  newMachine?: Destination;
  databases?: Destination;
  newDatabase?: Destination;
  customImages?: Destination;
  routes?: Destination;
  events?: Destination;
  dns?: Destination;
  /** Absent while the Operator has allowed nothing on the Host. */
  host?: Destination;
  settings?: Destination;
  application: (app: App) => Destination;
  virtualMachine: (machine: Environment) => Destination;
  children: ReactNode;
}) {
  const [theme, setTheme] = useTheme();
  // Below 1024px the sidebar is a drawer; any way out of it closes it,
  // including the create menu, which opens in a portal of its own.
  const [drawerOpen, setDrawerOpen] = useState(false);
  const applications = apps.filter((app) => !app.managed_postgres);

  const navigate = (destination: Destination) => {
    setDrawerOpen(false);
    if (destination.onClick) destination.onClick();
    else window.location.assign(destination.href);
  };

  return (
    <ApplicationShell
      className="console-shell"
      phoneNavigation="bottom"
      drawerOpen={drawerOpen}
      onDrawerOpenChange={setDrawerOpen}
      brand={
        <div className="brand">
          <BrandMark>
            <TerminalIcon />
          </BrandMark>
          <div className="grow truncate">
            <div className="t-label">self-host</div>
            <div className="t-metadata muted mono">{dnsSuffix}</div>
          </div>
        </div>
      }
      primaryAction={
        <CreateResource onDeploy={() => navigate(deploy)} onCreateMachine={() => navigate(newMachine)} onCreateDatabase={() => navigate(newDatabase)} />
      }
      navigation={[
        {
          destinations: [
            { ...overview, label: "Overview", leading: <Icon name="chart" /> },
            { ...databases, label: "Databases", leading: <Icon name="database" /> },
            { ...routes, label: "Routes", leading: <Icon name="route" /> },
            { ...dns, label: "DNS", leading: <Icon name="globe" /> },
            { ...events, label: "Events", leading: <Icon name="history" /> },
            ...(host ? [{ ...host, label: "Host", leading: <Icon name="screen" /> }] : []),
            {
              ...customImages,
              label: "Custom images",
              leading: <Icon name="box" />,
            },
          ],
        },
        ...(applications.length ? [{
          label: "Applications",
          destinations: applications.map((app) => ({
            ...application(app), label: app.name, leading: <StatusDot status={app.status} />,
          })),
        }] : []),
        ...(environments.length ? [{
          label: "Virtual machines",
          destinations: environments.map((machine) => ({
            ...virtualMachine(machine), label: machine.config.name, leading: <StatusDot status={machine.state} />,
          })),
        }] : []),
      ]}
      footer={
        <div className="sidebar-controls">
          <Button
            variant="ghost"
            size="sm"
            aria-current={settings.active ? "page" : undefined}
            // Tells the drawer and the More sheet to close, as a link would.
            data-shell-navigate=""
            onClick={() => navigate(settings)}
          >
            <Icon name="gear" />
            Settings
          </Button>
          <ThemeSelector theme={theme} onChange={setTheme} />
          <Button variant="ghost" size="sm" className="btn-icon" aria-label="Lock" title="Lock" onClick={logout}>
            <Icon name="lock" />
          </Button>
        </div>
      }
      header={
        <>
          <Breadcrumb>
            <BreadcrumbList>
              <BreadcrumbItem>
                <BreadcrumbLink href="/console/">Console</BreadcrumbLink>
              </BreadcrumbItem>
              <BreadcrumbSeparator />
              {parent ? (
                <>
                  <BreadcrumbItem>
                    <BreadcrumbLink
                      href={parent.href}
                      onClick={(event) => {
                        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                        event.preventDefault();
                        navigate(parent);
                      }}
                    >
                      {parent.label}
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator />
                </>
              ) : null}
              <BreadcrumbItem>
                <BreadcrumbPage>{crumb}</BreadcrumbPage>
              </BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>
          <span className="grow" />
          <span className="row t-label">
            {degraded ? (
              <button
                type="button"
                className="health-link"
                title={failing.length ? `The daemon answers, but ${failedLabel(failing).toLowerCase()}. Open it.` : "The daemon answers, but some workloads are down."}
                disabled={!failing.length || !onOpenFailing}
                onClick={() => { if (failing[0]) onOpenFailing?.(failing[0].id); }}
              >
                <Dot variant="warning" pulse />
                <span className="health-word">{failedLabel(failing)}</span>
                {version ? (
                  <>
                    <span className="muted health-sep" aria-hidden="true">·</span>
                    <span className="muted mono">v{version}</span>
                  </>
                ) : null}
              </button>
            ) : (
              <span
                className="health-link"
                title="The daemon answering on this Host"
              >
                <Dot variant={healthy ? "success" : "neutral"} />
                <span className="muted health-word">
                  {healthy ? "Healthy" : "Checking health…"}
                </span>
                {version ? (
                  <>
                    <span className="muted health-sep" aria-hidden="true">·</span>
                    <span className="muted mono">v{version}</span>
                  </>
                ) : null}
              </span>
            )}
          </span>
        </>
      }
    >
      {children}
    </ApplicationShell>
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

/** A resource's state, read out by screen readers since the color says it to everyone else. */
function StatusDot({ status }: { status: string }) {
  const tone = statusTone(status);
  return <Dot variant={tone} className={tone === "neutral" ? "subtle" : undefined} role="img" aria-label={status} aria-hidden={false} />;
}
