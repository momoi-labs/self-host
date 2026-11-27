import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { Badge } from "@momoi-labs/kiso-react";

import { OperationOutcomes } from "./components/OperationOutcomes.js";
import { Shell } from "./components/Shell.js";
import { useMetrics } from "./lib/useMetrics.js";
import { useEnvironments } from "./lib/useEnvironments.js";
import { settingsQuery } from "./lib/queries.js";
import { failedCounts } from "./lib/overview.js";
import { useSettingsFailures } from "./lib/settingsSections.js";
import { usePlatform } from "./lib/usePlatform.js";
import { useView } from "./lib/useView.js";
import { Dns } from "./views/Dns.js";
import { Routes } from "./views/Routes.js";
import { ManagedPostgres } from "./views/ManagedPostgres.js";
import { Databases } from "./views/Databases.js";
import { NewDatabase } from "./views/NewDatabase.js";
import { Apps } from "./views/Apps.js";
import { Machines } from "./views/Machines.js";
import { AppDetail } from "./views/AppDetail.js";
import { NewApp } from "./views/NewApp.js";
import { Overview } from "./views/Overview.js";
import { CustomImages } from "./views/CustomImages.js";
import { EventsPage } from "./views/Events.js";
import { Environments } from "./views/Environments.js";
import { Settings } from "./views/Settings.js";
import { Host } from "./views/Host.js";

export function App() {
  const { apps, dnsSuffix, domain, healthy, degraded, failing, version, ready, reload } = usePlatform();
  const { environments, ready: environmentsReady } = useEnvironments();
  const metrics = useMetrics();
  const [view, go] = useView();
  // The Host is in the menu only while the Operator allows something on it.
  const settings = useQuery(settingsQuery).data;
  const manageHost = !!(settings?.remoteScreen.effective || settings?.hostTerminal.effective);
  const failures = useSettingsFailures();
  const settingsFailing = failures.general + failures.access + failures.network + failures.backups;

  const app =
    view.view === "app"
      ? apps.find((candidate) => candidate.id === view.id)
      : undefined;

  useEffect(() => {
    if (!ready) return;
    if (view.view === "app" && !app) go({ view: "overview", id: null }, { replace: true });
    if (app?.runtime?.kind === "external") go({ view: "routes", id: null }, { replace: true });
  }, [ready, view, app, go]);

  const crumb =
    view.view === "dns"
      ? "DNS"
      : view.view === "apps"
      ? "Apps"
      : view.view === "machines"
      ? "Machines"
      : view.view === "routes"
      ? "Routes"
      : view.view === "databases"
      ? "Databases"
      : view.view === "database-new"
      ? "New database"
      : view.view === "events"
      ? "Events"
      : view.view === "app"
      ? (app?.name ?? "Application")
      : view.view === "new"
        ? "New application"
        : view.view === "custom-images"
          ? "Images"
          : view.view === "custom-image"
            ? view.id
              ? "Custom image"
              : "New image"
            : view.view === "environments"
              ? (environments.find((one) => one.id === view.id)?.config.name ??
                "Virtual machine")
              : view.view === "environment-new"
                ? "New virtual machine"
                : view.view === "settings"
                  ? "Settings"
                  : view.view === "host"
                    ? "Host"
                    : "Overview";

  const parent =
    view.view === "database-new" || app?.managed_postgres
      ? { label: "Databases", href: "/console/#databases", active: false, onClick: () => go({ view: "databases", id: null }) }
      : view.view === "new" || (view.view === "app" && app)
      ? { label: "Apps", href: "/console/#apps", active: false, onClick: () => go({ view: "apps", id: null }) }
      : view.view === "environment-new" || view.view === "environments"
      ? { label: "Machines", href: "/console/#machines", active: false, onClick: () => go({ view: "machines", id: null }) }
      : view.view === "custom-image"
      ? { label: "Images", href: "/console/#custom-images", active: false, onClick: () => go({ view: "custom-images", id: null }) }
      : undefined;

  return (
    <Shell
      crumb={crumb}
      parent={parent}
      dnsSuffix={domain}
      healthy={healthy}
      degraded={degraded}
      failing={failing}
      onOpenFailing={(id) => go({ view: "app", id })}
      version={version}
      overview={{
        href: "/console/",
        active: view.view === "overview",
        onClick: () => go({ view: "overview", id: null }),
      }}
      apps={{
        href: "/console/#apps",
        active: view.view === "apps" || view.view === "new" || (view.view === "app" && !!app && !app.managed_postgres),
        onClick: () => go({ view: "apps", id: null }),
      }}
      machines={{
        href: "/console/#machines",
        active: view.view === "machines" || view.view === "environments" || view.view === "environment-new",
        onClick: () => go({ view: "machines", id: null }),
      }}
      failed={failedCounts(apps, environments)}
      dns={{ href: "/console/#dns", active: view.view === "dns", onClick: () => go({ view: "dns", id: null }) }}
      routes={{ href: "/console/#routes", active: view.view === "routes", onClick: () => go({ view: "routes", id: null }) }}
      events={{
        href: "/console/#events",
        active: view.view === "events",
        onClick: () => go({ view: "events", id: null }),
      }}
      databases={{
        href: "/console/#databases",
        active: view.view === "databases" || view.view === "database-new" || !!app?.managed_postgres,
        onClick: () => go({ view: "databases", id: null }),
      }}
      customImages={{
        href: "/console/#custom-images",
        active: view.view === "custom-images" || view.view === "custom-image",
        onClick: () => go({ view: "custom-images", id: null }),
      }}
      host={manageHost ? { href: "/console/#host", active: view.view === "host", onClick: () => go({ view: "host", id: null }) } : undefined}
      settings={{
        href: "/console/#settings",
        active: view.view === "settings",
        onClick: () => go({ view: "settings", id: "general" }),
        trailing: settingsFailing ? <Badge variant="danger">{settingsFailing}<span className="offscreen"> failing</span></Badge> : undefined,
      }}
    >
      {view.view === "dns" ? (
        <Dns applications={apps} machines={environments}
          onOpenApplication={id => go({ view: "app", id })}
          onOpenVirtualMachine={id => go({ view: "environments", id })} />
      ) : view.view === "events" ? (
        <EventsPage onOpenSubject={(subject) => {
          if (subject.kind === "api-key") { go({ view: "settings", id: "access/api-keys" }); return; }
          if (subject.kind === "settings") { go({ view: "settings", id: "general" }); return; }
          if (subject.kind === "platform") { go({ view: "settings", id: "backups/scheduled-tasks" }); return; }
          go({ view: subject.kind === "virtual-machine" ? "environments" : subject.kind === "custom-image" ? "custom-image" : "app", id: subject.id });
        }} />
      ) : view.view === "routes" ? (
        <Routes apps={apps} ready={ready} dnsSuffix={dnsSuffix} reload={reload} onOpenApp={(id) => go({ view: "app", id })} />
      ) : view.view === "apps" ? (
        <Apps apps={apps} ready={ready} metrics={metrics} onDeploy={() => go({ view: "new", id: null })} onOpen={(id) => go({ view: "app", id })} />
      ) : view.view === "machines" ? (
        <Machines machines={environments} ready={environmentsReady} metrics={metrics}
          onCreate={() => go({ view: "environment-new", id: null })} onOpen={(id) => go({ view: "environments", id })} />
      ) : view.view === "databases" ? (
        <Databases apps={apps} ready={ready} reload={reload} onCreate={() => go({ view: "database-new", id: null })} onOpen={(id) => go({ view: "app", id })} />
      ) : view.view === "database-new" ? (
        <NewDatabase reload={reload} onCancel={() => go({ view: "databases", id: null })} onCreated={(id) => go({ view: "app", id })} />
      ) : view.view === "settings" ? (
        <Settings id={view.id} onOpen={(id) => go({ view: "settings", id })} />
      ) : view.view === "host" ? (
        <Host onOpenSettings={() => go({ view: "settings", id: "access/host" })} />
      ) : view.view === "environment-new" || (view.view === "environments" && view.id) ? (
        <Environments
          metrics={metrics}
          mode={view.view === "environment-new" ? "new" : "detail"}
          selected={view.id}
          onOpen={(id) =>
            go(id ? { view: "environments", id } : { view: "machines", id: null })
          }
        />
      ) : view.view === "custom-images" || view.view === "custom-image" ? (
        <CustomImages
          listing={view.view === "custom-images"}
          selected={view.id}
          onOpen={(id) => go({ view: "custom-image", id })}
          onList={() => go({ view: "custom-images", id: null })}
        />
      ) : view.view === "new" ? (
        <NewApp
          dnsSuffix={dnsSuffix}
          reload={reload}
          onCancel={() => go({ view: "apps", id: null })}
          onCreated={(created) =>
            go(
              created
                ? { view: "app", id: created.id }
                : { view: "apps", id: null },
            )
          }
        />
      ) : app?.managed_postgres ? (
        <ManagedPostgres key={app.id} app={app} apps={apps} metrics={metrics} reload={reload} onRemoved={() => go({ view: "databases", id: null })} onOpenApp={(id) => go({ view: "app", id })} />
      ) : app && app.runtime?.kind !== "external" ? (
        <AppDetail
          key={app.id}
          app={app}
          apps={apps}
          formRevision={signature(app)}
          dnsSuffix={dnsSuffix}
          metrics={metrics}
          reload={reload}
          onRemoved={() => go({ view: "apps", id: null })}
          onOpenApp={(id) => go({ view: "app", id })}
        />
      ) : (
        <Overview
          ready={ready && environmentsReady}
          domain={domain}
          apps={apps}
          environments={environments}
          metrics={metrics}
          onOpenApp={(id) => go({ view: "app", id })}
          onOpenEnvironment={(id) => go({ view: "environments", id })}
          onDeploy={() => go({ view: "new", id: null })}
          onNewDatabase={() => go({ view: "database-new", id: null })}
          onNewMachine={() => go({ view: "environment-new", id: null })}
          onOpenEvents={() => go({ view: "events", id: null })}
        />
      )}
      <OperationOutcomes environments={environments} />
    </Shell>
  );
}

/** What the detail is showing. A poll that changes none of it changes nothing. */
function signature(app: import("./lib/types.js").App): string {
  return [
    app.status,
    app.name,
    app.image,
    JSON.stringify(app.runtime ?? null),
    JSON.stringify(app.publication ?? null),
    JSON.stringify(app.network_policy ?? null),
    JSON.stringify(app.git ?? null),
    JSON.stringify(app.git_build ?? null),
    app.hostname,
    (app.aliases ?? []).join(","),
    JSON.stringify(app.last_error ?? null),
    app.compose ?? "",
    app.web_service ?? "",
    app.web_port ?? "",
    (app.services ?? [])
      .map((service) => `${service.service}:${service.state}`)
      .join(","),
  ].join("|");
}
