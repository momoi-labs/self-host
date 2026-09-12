import { useEffect, useState } from "react";

import { Shell } from "./components/Shell.js";
import { useMetrics } from "./lib/useMetrics.js";
import { usePlatform } from "./lib/usePlatform.js";
import { useView } from "./lib/useView.js";
import { AppDetail } from "./views/AppDetail.js";
import { NewApp } from "./views/NewApp.js";
import { Overview } from "./views/Overview.js";
import { DevImages } from "./views/DevImages.js";
// PROTOTYPE (#107): ?variant= swaps the image editor for the variants.
import { DevImagePrototype } from "./views/DevImages.prototype.js";
import { readVariant } from "./components/PrototypeSwitcher.js";

export function App() {
  const { apps, dnsSuffix, healthy, ready, reload } = usePlatform();
  const metrics = useMetrics();
  const [view, go] = useView();
  const [query, setQuery] = useState("");

  const app = view.view === "app" ? apps.find((candidate) => candidate.id === view.id) : undefined;

  // A view whose subject is gone — an Application removed here or in another
  // tab — falls back to the Overview rather than rendering nothing.
  useEffect(() => {
    if (!ready) return;
    if (view.view === "app" && !app) go({ view: "overview", id: null });
  }, [ready, view, app, go]);

  const crumb =
    view.view === "app"
      ? (app?.name ?? "Application")
      : view.view === "new"
        ? "New application"
        : view.view === "dev-images"
          ? "Development images"
          : view.view === "dev-image"
            ? view.id ? "Development image" : "New image"
            : "Overview";

  return (
    <Shell
      crumb={crumb}
      dnsSuffix={dnsSuffix}
      apps={apps}
      healthy={healthy}
      overview={{
        href: "/console/",
        active: view.view === "overview",
        onClick: () => go({ view: "overview", id: null }),
      }}
      deploy={{
        href: "/console/#new",
        active: view.view === "new",
        onClick: () => go({ view: "new", id: null }),
      }}
      devImages={{
        href: "/console/#dev-images",
        active: view.view === "dev-images" || view.view === "dev-image",
        onClick: () => go({ view: "dev-images", id: null }),
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
        {import.meta.env.DEV && view.view === "dev-image" && readVariant() ? (
          <DevImagePrototype />
        ) : view.view === "dev-images" || view.view === "dev-image" ? (
          <DevImages listing={view.view === "dev-images"} selected={view.id}
            onOpen={(id) => go({ view: "dev-image", id })} />
        ) : view.view === "new" ? (
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
            metrics={metrics}
            reload={reload}
            onRemoved={() => go({ view: "overview", id: null })}
          />
        ) : (
          <Overview
            apps={apps}
            dnsSuffix={dnsSuffix}
            metrics={metrics}
            query={query}
            onQuery={setQuery}
            onOpenApp={(id) => go({ view: "app", id })}
            onDeploy={() => go({ view: "new", id: null })}
          />
        )}
      </section>
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
    (app.services ?? []).map((service) => `${service.service}:${service.state}`).join(","),
  ].join("|");
}
