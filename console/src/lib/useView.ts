import { useCallback, useEffect, useState } from "react";

import { isSettingsTab, type SettingsTab } from "../views/Settings.js";

export type View =
  | { view: "overview"; id: null }
  | { view: "events"; id: null }
  | { view: "dns"; id: null }
  | { view: "app"; id: string }
  | { view: "new"; id: null }
  | { view: "custom-image"; id: string | null }
  | { view: "custom-images"; id: null }
  | { view: "environments"; id: string }
  | { view: "environment-new"; id: null }
  | { view: "settings"; id: SettingsTab };

const overview: View = { view: "overview", id: null };

/**
 * A view named in the URL fragment, so a link into the console opens on it.
 * Settings carries its tab in the fragment too: `#settings/api-keys`.
 */
function fromHash(): View | null {
  const hash = location.hash;
  if (hash === "#dns") return { view: "dns", id: null };
  if (hash === "#settings") return { view: "settings", id: "general" };
  if (hash.startsWith("#settings/") && isSettingsTab(hash.slice(10)))
    return { view: "settings", id: hash.slice(10) as SettingsTab };
  if (hash === "#events") return { view: "events", id: null };
  if (hash === "#new") return { view: "new", id: null };
  if (hash === "#custom-images") return { view: "custom-images", id: null };
  if (hash === "#new-custom-image") return { view: "custom-image", id: null };
  if (hash.startsWith("#custom-image-"))
    return { view: "custom-image", id: hash.slice(14) };
  if (hash === "#new-environment") return { view: "environment-new", id: null };
  if (hash.startsWith("#environment-"))
    return { view: "environments", id: hash.slice(13) };
  if (hash.startsWith("#app-")) return { view: "app", id: hash.slice(5) };
  return null;
}

function fromHistory(): View {
  const state = history.state as View | null;
  if (state?.view === "dns") return { view: "dns", id: null };
  if (state?.view === "events") return { view: "events", id: null };
  if (state?.view === "app") return state;
  if (state?.view === "new") return { view: "new", id: null };
  if (state?.view === "custom-images")
    return { view: "custom-images", id: null };
  if (state?.view === "custom-image") return state;
  if (state?.view === "environments") return state;
  if (state?.view === "environment-new") return state;
  if (state?.view === "settings") return state;
  return (
    fromHash() ??
    (new URLSearchParams(location.search).get("demo") === "environments"
      ? { view: "environment-new", id: null }
      : overview)
  );
}

/** The fragment a view is addressed by, for the views that keep one. */
function hashFor(view: View): string | undefined {
  if (view.view === "dns") return "#dns";
  if (view.view === "settings") return view.id === "general" ? "#settings" : `#settings/${view.id}`;
  return undefined;
}

/**
 * The views live in history state, so Back returns to the list instead of
 * leaving the console. A view the record no longer supports — an Application
 * that was removed in another tab — falls back to the Overview.
 */
export function useView(): [View, (next: View) => void] {
  const [current, setCurrent] = useState<View>(fromHistory);

  useEffect(() => {
    // A first load carries no state. Seed it with the view that is actually on
    // screen — a link into `#app-<id>` opens on that Application, and Back has
    // to return to it rather than to the Overview it never showed.
    if (!history.state) history.replaceState(current, "");
    const onPop = () => setCurrent(fromHistory());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
    // Seeding is a first-load concern; `current` afterwards is `go`'s business.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const go = useCallback((next: View) => {
    const state = history.state as View | null;
    if (!state) history.replaceState(next, "");
    else if (state.view !== next.view || state.id !== next.id) {
      const url = hashFor(next) ?? (hashFor(state) ? location.pathname + location.search : undefined);
      history.pushState(next, "", url);
    }
    setCurrent(next);
  }, []);

  return [current, go];
}
