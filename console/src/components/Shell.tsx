import type { ReactNode } from "react";
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
  NavigationItem,
  NavigationLink,
  NavigationList,
  TerminalIcon,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { logout } from "../lib/api.js";
import { statusTone } from "../lib/status.js";
import { useTheme } from "../lib/theme.js";
import type { App, Environment } from "../lib/types.js";
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
  dnsSuffix,
  apps,
  environments,
  healthy,
  version,
  overview,
  deploy,
  newMachine = { href: "/console/#new-environment", active: false },
  customImages = { href: "/console/#custom-images", active: false },
  dns = { href: "/console/#dns", active: false },
  events = { href: "/console/#events", active: false },
  settings = { href: "/console/#settings", active: false },
  application,
  virtualMachine,
  children,
}: {
  crumb: string;
  dnsSuffix: string;
  apps: App[];
  environments: Environment[];
  healthy: boolean;
  version: string | null;
  overview: Destination;
  deploy: Destination;
  newMachine?: Destination;
  customImages?: Destination;
  events?: Destination;
  dns?: Destination;
  settings?: Destination;
  application: (app: App) => Destination;
  virtualMachine: (machine: Environment) => Destination;
  children: ReactNode;
}) {
  const [theme, setTheme] = useTheme();

  const navigate = (destination: Destination) => {
    if (destination.onClick) destination.onClick();
    else window.location.assign(destination.href);
  };

  return (
    <ApplicationShell
      className="console-shell"
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
        <CreateResource onDeploy={() => navigate(deploy)} onCreateMachine={() => navigate(newMachine)} />
      }
      navigation={[
        {
          destinations: [
            { ...overview, label: "Overview", leading: <Icon name="chart" /> },
            { ...dns, label: "DNS", leading: <Icon name="globe" /> },
            { ...events, label: "Events", leading: <Icon name="history" /> },
            {
              ...customImages,
              label: "Custom images",
              leading: <Icon name="box" />,
            },
          ],
        },
        {
          destinations: [],
          empty: (
            <>
              {apps.length > 0 && (
                <ResourceLinks label="Applications" items={apps.map((app) => ({
                  ...application(app), name: app.name, status: app.status,
                }))} />
              )}
              {environments.length > 0 && (
                <ResourceLinks label="Virtual machines" items={environments.map((machine) => ({
                  ...virtualMachine(machine), name: machine.config.name, status: machine.state,
                }))} />
              )}
            </>
          ),
        },
      ]}
      footer={
        <div className="sidebar-controls">
          <Button
            variant="ghost"
            size="sm"
            aria-current={settings.active ? "page" : undefined}
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
              <BreadcrumbItem>
                <BreadcrumbPage>{crumb}</BreadcrumbPage>
              </BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>
          <span className="grow" />
          <span className="row t-label">
            <span
              className="health-link"
              title="The daemon answering on this Host"
            >
              <Dot variant={healthy ? "success" : "neutral"} />
              <span className="muted">
                {healthy ? "Healthy" : "Checking health…"}
              </span>
              {version ? (
                <>
                  <span className="muted" aria-hidden="true">·</span>
                  <span className="muted mono">v{version}</span>
                </>
              ) : null}
            </span>
          </span>
        </>
      }
    >
      {children}
    </ApplicationShell>
  );
}

/** Resource groups disappear entirely until they have something to navigate to. */
function ResourceLinks({ label, items }: {
  label: string;
  items: (Destination & { name: string; status: string })[];
}) {
  return (
    <section className="resource-navigation" aria-label={label}>
      <div className="resource-navigation-heading">
        <span>{label}</span><span>{items.length}</span>
      </div>
      <NavigationList>
        {items.map((item) => {
          const tone = statusTone(item.status);
          return (
            <NavigationItem key={item.href}>
              <NavigationLink href={item.href} active={item.active} title={`${item.name}: ${item.status}`}
                onClick={(event) => {
                  if (!item.onClick) return;
                  event.preventDefault();
                  item.onClick();
                }}>
                <Dot
                  variant={tone}
                  className={tone === "neutral" ? "subtle" : undefined}
                  role="img"
                  aria-label={item.status}
                  aria-hidden={false}
                />
                <span className="grow truncate">{item.name}</span>
              </NavigationLink>
            </NavigationItem>
          );
        })}
      </NavigationList>
    </section>
  );
}
