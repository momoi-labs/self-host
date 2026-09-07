import { useCallback, useEffect, useState } from "react";

export type View =
  | { view: "overview"; id: null }
  | { view: "system"; id: string }
  | { view: "app"; id: string }
  | { view: "new"; id: null };

const overview: View = { view: "overview", id: null };

function fromHistory(): View {
  const state = history.state as View | null;
  if (state?.view === "app" || state?.view === "system") return state;
  if (state?.view === "new") return { view: "new", id: null };
  return overview;
}

/**
 * The four views live in history state, so Back returns to the list instead of
 * leaving the console. A view the record no longer supports — an Application
 * that was removed in another tab — falls back to the Overview.
 */
export function useView(): [View, (next: View) => void] {
  const [current, setCurrent] = useState<View>(fromHistory);

  useEffect(() => {
    if (!history.state) history.replaceState(overview, "");
    const onPop = () => setCurrent(fromHistory());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const go = useCallback((next: View) => {
    const state = history.state as View | null;
    if (!state) history.replaceState(next, "");
    else if (state.view !== next.view || state.id !== next.id) history.pushState(next, "");
    setCurrent(next);
  }, []);

  return [current, go];
}
