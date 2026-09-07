import { useEffect, useState } from "react";
import {
  AppShell,
  AppShellMain,
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
  BrandMark,
  Button,
  Dot,
  Header,
  Navigation,
  NavigationGroup,
  NavigationItem,
  NavigationLink,
  NavigationList,
  Separator,
  Sidebar,
  SidebarBody,
  SidebarFooter,
  SidebarHeader,
  TerminalIcon,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { Icon } from "./components/Icon.js";
import { logout } from "./lib/api.js";
import { statusTone } from "./lib/status.js";
import { useTheme } from "./lib/theme.js";
import { usePlatform } from "./lib/usePlatform.js";
import { useView } from "./lib/useView.js";
import { AppDetail } from "./views/AppDetail.js";
import { NewApp } from "./views/NewApp.js";
import { Overview } from "./views/Overview.js";
import { SystemDetail } from "./views/SystemDetail.js";

export function App() {
  const { apps, system, dnsSuffix, healthy, ready, reload } = usePlatform();
  const [view, go] = useView();
  const [theme, setTheme] = useTheme();
  const [query, setQuery] = useState("");
  const [showPlatform, setShowPlatform] = useState(false);

  const app = view.view === "app" ? apps.find((candidate) => candidate.id === view.id) : undefined;
  const container =
    view.view === "system" ? system.find((candidate) => candidate.role === view.id) : undefined;

  // A view whose subject is gone — an Application removed here or in another
  // tab — falls back to the Overview rather than rendering nothing.
  useEffect(() => {
    if (!ready) return;
    if (view.view === "app" && !app) go({ view: "overview", id: null });
    if (view.view === "system" && !container) go({ view: "overview", id: null });
  }, [ready, view, app, container, go]);

  const crumb =
    view.view === "app"
      ? (app?.name ?? "Application")
      : view.view === "system"
        ? "Platform"
        : view.view === "new"
          ? "New application"
          : "Overview";

  return (
    <AppShell>
      <Sidebar>
        <SidebarHeader>
          <div className="brand">
            <BrandMark>
              <TerminalIcon />
            </BrandMark>
            <div className="grow truncate">
              <div className="t-label">self-host</div>
              <div className="t-metadata muted mono">{dnsSuffix}</div>
            </div>
          </div>
          <Button
            variant="primary"
            size="sm"
            className="btn-block"
            onClick={() => go({ view: "new", id: null })}
          >
            <Icon name="plus" />
            Deploy application
          </Button>
        </SidebarHeader>

        <SidebarBody>
          <Navigation>
            <NavigationGroup>
              <NavigationList>
                <NavigationItem>
                  <NavigationLink
                    href="#overview"
                    active={view.view === "overview"}
                    onClick={(event) => {
                      event.preventDefault();
                      go({ view: "overview", id: null });
                    }}
                  >
                    <Icon name="chart" />
                    Overview
                  </NavigationLink>
                </NavigationItem>
              </NavigationList>
            </NavigationGroup>

            <NavigationGroup label="Applications">
              {apps.length === 0 ? (
                <p className="nav-item muted">None yet</p>
              ) : (
                <NavigationList>
                  {apps.map((candidate) => (
                    <NavigationItem key={candidate.id}>
                      <NavigationLink
                        href={`#app-${candidate.id}`}
                        active={view.view === "app" && view.id === candidate.id}
                        onClick={(event) => {
                          event.preventDefault();
                          go({ view: "app", id: candidate.id });
                        }}
                      >
                        <Dot
                          variant={statusTone(candidate.status)}
                          className={statusTone(candidate.status) === "neutral" ? "subtle" : undefined}
                        />
                        <span className="grow truncate">{candidate.name}</span>
                      </NavigationLink>
                    </NavigationItem>
                  ))}
                </NavigationList>
              )}
            </NavigationGroup>
          </Navigation>
        </SidebarBody>

        <SidebarFooter>
          <a className="nav-item" href="/console/setup.html">
            <Icon name="globe" />
            DNS setup
          </a>
          <a className="nav-item" href="/console/api-keys.html">
            <Icon name="key" />
            API keys
          </a>
          <ThemeSelector theme={theme} onChange={setTheme} />
          <Separator />
          <button type="button" className="nav-item" onClick={logout}>
            <Icon name="lock" />
            Lock
          </button>
        </SidebarFooter>
      </Sidebar>

      <AppShellMain>
        <Header>
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
            {/* The health indicator doubles as the way to show platform services. */}
            <button
              type="button"
              className="health-link"
              title="Platform Infra"
              onClick={() => {
                setShowPlatform(true);
                setQuery("");
                go({ view: "overview", id: null });
              }}
            >
              <Dot variant={healthy ? "success" : "neutral"} />
              <span className="muted">{healthy ? "Healthy" : "Checking health…"}</span>
            </button>
          </span>
        </Header>

        {/* The id is a hook for console.css: the detail panel sizes itself
            differently from a scrolling page. */}
        <section className="page" id="detail" aria-live="polite">
          {view.view === "new" ? (
            <NewApp
              dnsSuffix={dnsSuffix}
              reload={reload}
              onCancel={() => go({ view: "overview", id: null })}
              onCreated={(created) =>
                go(created ? { view: "app", id: created.id } : { view: "overview", id: null })
              }
            />
          ) : app ? (
            <AppDetail
              // A record that moved rebuilds the form: the fields describe the
              // Application, and the Application changed underneath them.
              key={`${app.id}|${signature(app)}`}
              app={app}
              dnsSuffix={dnsSuffix}
              reload={reload}
              onRemoved={() => go({ view: "overview", id: null })}
            />
          ) : container ? (
            <SystemDetail container={container} />
          ) : (
            <Overview
              apps={apps}
              system={system}
              dnsSuffix={dnsSuffix}
              query={query}
              onQuery={setQuery}
              showPlatform={showPlatform}
              onShowPlatform={setShowPlatform}
              onOpenApp={(id) => go({ view: "app", id })}
              onOpenSystem={(role) => go({ view: "system", id: role })}
              onDeploy={() => go({ view: "new", id: null })}
            />
          )}
        </section>
      </AppShellMain>
    </AppShell>
  );
}

/** What the detail is showing. A poll that changes none of it changes nothing. */
function signature(app: import("./lib/types.js").App): string {
  return [
    app.status,
    app.name,
    app.image,
    app.hostname,
    (app.aliases ?? []).join(","),
    JSON.stringify(app.last_error ?? null),
    app.compose ?? "",
    app.web_service ?? "",
    app.web_port ?? "",
    (app.services ?? []).map((service) => `${service.service}:${service.state}`).join(","),
  ].join("|");
}
