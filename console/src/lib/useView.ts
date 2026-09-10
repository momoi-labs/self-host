import { useCallback, useEffect, useState } from "react";

export type View =
  | { view: "overview"; id: null }
  | { view: "app"; id: string }
  | { view: "new"; id: null }
  | { view: "dev-image"; id: string | null }
  | { view: "dev-images"; id: null };

const overview: View = { view: "overview", id: null };

/**
 * A view named in the URL fragment. The settings pages are documents of their
 * own, so the only way their sidebar can point at an Application is a link —
 * `/console/#app-<id>` — and that link has to open on it.
 */
function fromHash(): View | null {
  const hash = location.hash;
  if (hash === "#new") return { view: "new", id: null };
  if (hash === "#dev-images") return { view: "dev-images", id: null };
  if (hash === "#new-dev-image") return { view: "dev-image", id: null };
  if (hash.startsWith("#dev-image-")) return { view: "dev-image", id: hash.slice(11) };
  if (hash.startsWith("#app-")) return { view: "app", id: hash.slice(5) };
  return null;
}

function fromHistory(): View {
  const state = history.state as View | null;
  if (state?.view === "app") return state;
  if (state?.view === "new") return { view: "new", id: null };
  if (state?.view === "dev-images") return { view: "dev-images", id: null };
  if (state?.view === "dev-image") return state;
  return fromHash() ?? overview;
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
    else if (state.view !== next.view || state.id !== next.id) history.pushState(next, "");
    setCurrent(next);
  }, []);

  return [current, go];
}
