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
  Separator,
  TerminalIcon,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { logout } from "../lib/api.js";
import { statusTone } from "../lib/status.js";
import { useTheme } from "../lib/theme.js";
import type { App } from "../lib/types.js";
import { Icon } from "./Icon.js";

/** Where a sidebar entry points, and how it knows it is the current one. */
export type Destination = { href: string; active: boolean; onClick?: () => void };

/**
 * The console's frame: the sidebar, the breadcrumb and the health indicator.
 *
 * The DNS setup and API keys pages are separate documents rather than views of
 * the application, so the frame has to be a component both can mount. What
 * differs between them is only how a destination is reached — the application
 * changes a view in place, the settings pages follow a link.
 */
export function Shell({
  crumb,
  dnsSuffix,
  apps,
  healthy,
  overview,
  deploy,
  application,
  children,
}: {
  crumb: string;
  dnsSuffix: string;
  apps: App[];
  healthy: boolean;
  overview: Destination;
  deploy: Destination;
  application: (app: App) => Destination;
  children: ReactNode;
}) {
  const [theme, setTheme] = useTheme();

  const follow = (destination: Destination) => (event: React.MouseEvent) => {
    if (!destination.onClick) return;
    event.preventDefault();
    destination.onClick();
  };

  return (
    <ApplicationShell
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
        <Button variant="primary" size="sm" className="btn-block" asChild>
          <a href={deploy.href} onClick={follow(deploy)}>
            <Icon name="plus" />
            Deploy application
          </a>
        </Button>
      }
      navigation={[
        {
          destinations: [{ ...overview, label: "Overview", leading: <Icon name="chart" /> }],
        },
        {
          label: "Applications",
          empty: <p className="nav-item muted">None yet</p>,
          destinations: apps.map((candidate) => {
            const tone = statusTone(candidate.status);
            return {
              ...application(candidate),
              label: candidate.name,
              leading: <Dot variant={tone} className={tone === "neutral" ? "subtle" : undefined} />,
            };
          }),
        },
      ]}
      footer={
        <>
          <a
            className="nav-item"
            href="/console/setup.html"
            aria-current={crumb === "DNS setup" ? "page" : undefined}
          >
            <Icon name="globe" />
            DNS setup
          </a>
          <a
            className="nav-item"
            href="/console/api-keys.html"
            aria-current={crumb === "API keys" ? "page" : undefined}
          >
            <Icon name="key" />
            API keys
          </a>
          <ThemeSelector theme={theme} onChange={setTheme} />
          <Separator />
          <button type="button" className="nav-item" onClick={logout}>
            <Icon name="lock" />
            Lock
          </button>
        </>
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
            <span className="health-link" title="The daemon answering on this Host">
              <Dot variant={healthy ? "success" : "neutral"} />
              <span className="muted">{healthy ? "Healthy" : "Checking health…"}</span>
            </span>
          </span>
        </>
      }
    >
      {children}
    </ApplicationShell>
  );
}
