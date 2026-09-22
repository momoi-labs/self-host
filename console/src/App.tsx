import { useEffect } from "react";

import { OperationOutcomes } from "./components/OperationOutcomes.js";
import { Shell } from "./components/Shell.js";
import { useMetrics } from "./lib/useMetrics.js";
import { useEnvironments } from "./lib/useEnvironments.js";
import { usePlatform } from "./lib/usePlatform.js";
import { useView } from "./lib/useView.js";
import { Dns } from "./views/Dns.js";
import { AppDetail } from "./views/AppDetail.js";
import { NewApp } from "./views/NewApp.js";
import { Overview } from "./views/Overview.js";
import { CustomImages } from "./views/CustomImages.js";
import { EventsPage } from "./views/Events.js";
import { Environments } from "./views/Environments.js";
import { Settings } from "./views/Settings.js";

export function App() {
  const { apps, dnsSuffix, healthy, version, ready, reload } = usePlatform();
  const { environments } = useEnvironments();
  const metrics = useMetrics();
  const [view, go] = useView();

  const app =
    view.view === "app"
      ? apps.find((candidate) => candidate.id === view.id)
      : undefined;

  // A view whose subject is gone — an Application removed here or in another
  // tab — falls back to the Overview rather than rendering nothing.
  useEffect(() => {
    if (!ready) return;
    if (view.view === "app" && !app) go({ view: "overview", id: null });
  }, [ready, view, app, go]);

  const crumb =
    view.view === "dns"
      ? "DNS"
      : view.view === "events"
      ? "Events"
      : view.view === "app"
      ? (app?.name ?? "Application")
      : view.view === "new"
        ? "New application"
        : view.view === "custom-images"
          ? "Custom images"
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
                  : "Overview";

  return (
    <Shell
      crumb={crumb}
      dnsSuffix={dnsSuffix}
      apps={apps}
      environments={environments}
      healthy={healthy}
      version={version}
      overview={{
        href: "/console/",
        active: view.view === "overview",
        onClick: () => go({ view: "overview", id: null }),
      }}
      dns={{ href: "/console/#dns", active: view.view === "dns", onClick: () => go({ view: "dns", id: null }) }}
      events={{
        href: "/console/#events",
        active: view.view === "events",
        onClick: () => go({ view: "events", id: null }),
      }}
      deploy={{
        href: "/console/#new",
        active: view.view === "new",
        onClick: () => go({ view: "new", id: null }),
      }}
      newMachine={{
        href: "/console/#new-environment",
        active: view.view === "environment-new",
        onClick: () => go({ view: "environment-new", id: null }),
      }}
      virtualMachine={(machine) => ({
        href: `/console/#environment-${machine.id}`,
        active: view.view === "environments" && view.id === machine.id,
        onClick: () => go({ view: "environments", id: machine.id }),
      })}
      customImages={{
        href: "/console/#custom-images",
        active: view.view === "custom-images" || view.view === "custom-image",
        onClick: () => go({ view: "custom-images", id: null }),
      }}
      settings={{
        href: "/console/#settings",
        active: view.view === "settings",
        onClick: () => go({ view: "settings", id: "general" }),
      }}
      application={(candidate) => ({
        href: `/console/#app-${candidate.id}`,
        active: view.view === "app" && view.id === candidate.id,
        onClick: () => go({ view: "app", id: candidate.id }),
      })}
    >
      {/* The id is a hook for console.css: the detail panel sizes itself
          differently from a scrolling page. */}
      <section className="page" id="detail" aria-live="polite">
        {view.view === "dns" ? (
          <Dns applications={apps} machines={environments}
            onOpenApplication={id => go({ view: "app", id })}
            onOpenVirtualMachine={id => go({ view: "environments", id })} />
        ) : view.view === "events" ? (
          <EventsPage onOpenSubject={(subject) => {
            if (subject.kind === "api-key") { go({ view: "settings", id: "api-keys" }); return; }
            if (subject.kind === "settings") { go({ view: "settings", id: "general" }); return; }
            go({ view: subject.kind === "virtual-machine" ? "environments" : subject.kind === "custom-image" ? "custom-image" : "app", id: subject.id });
          }} />
        ) : view.view === "settings" ? (
          <Settings tab={view.id} onTab={(tab) => go({ view: "settings", id: tab })} />
        ) : view.view === "environment-new" || (view.view === "environments" && view.id) ? (
          <Environments
            metrics={metrics}
            mode={view.view === "environment-new" ? "new" : "detail"}
            selected={view.id}
            onOpen={(id) =>
              go(id ? { view: "environments", id } : { view: "overview", id: null })
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
            onCancel={() => go({ view: "overview", id: null })}
            onCreated={(created) =>
              go(
                created
                  ? { view: "app", id: created.id }
                  : { view: "overview", id: null },
              )
            }
          />
        ) : app ? (
          <AppDetail
            // A record that moved rebuilds the form: the fields describe the
            // Application, and the Application changed underneath them.
            key={`${app.id}|${signature(app)}`}
            app={app}
            dnsSuffix={dnsSuffix}
            metrics={metrics}
            reload={reload}
            onRemoved={() => go({ view: "overview", id: null })}
          />
        ) : (
          <Overview
            apps={apps}
            environments={environments}
            dnsSuffix={dnsSuffix}
            metrics={metrics}
            onOpenApp={(id) => go({ view: "app", id })}
            onOpenEnvironment={(id) => go({ view: "environments", id })}
            onDeploy={() => go({ view: "new", id: null })}
            onNewMachine={() => go({ view: "environment-new", id: null })}
          />
        )}
      </section>
      {/* A run is watched on the Overview's table or the machine's own
          screen; the toast only says how it ended, wherever the Operator
          went meanwhile. */}
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
