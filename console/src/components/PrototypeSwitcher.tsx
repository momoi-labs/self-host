import { useEffect, useState } from "react";

/**
 * PROTOTYPE. A floating bar that cycles a `?variant=` URL search param so
 * several throwaway renderings of one route can be compared in place. Never
 * ships: it only mounts when the param is present, and it dies with the
 * prototype branch.
 */
export function PrototypeSwitcher({ variants, current, note }: {
  variants: readonly { key: string; name: string }[];
  current: string;
  note?: string;
}) {
  const index = Math.max(0, variants.findIndex(one => one.key === current));
  const go = (step: number) => {
    const next = variants[(index + step + variants.length) % variants.length]!;
    const url = new URL(location.href);
    url.searchParams.set("variant", next.key);
    history.replaceState(history.state, "", url);
    dispatchEvent(new Event("prototype-variant"));
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && (target.closest("input, textarea, select, [contenteditable], [role=combobox]"))) return;
      if (event.key === "ArrowLeft") go(-1);
      if (event.key === "ArrowRight") go(1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return <div className="proto-switcher" role="toolbar" aria-label="Prototype variants">
    <button type="button" onClick={() => go(-1)} aria-label="Previous variant">←</button>
    <span className="proto-switcher-label">
      <strong>{variants[index]?.key}</strong> {variants[index]?.name}
      <small>{index + 1}/{variants.length}{note ? ` · ${note}` : ""}</small>
    </span>
    <button type="button" onClick={() => go(1)} aria-label="Next variant">→</button>
  </div>;
}

/** The current `?variant=`, re-read whenever the switcher changes it. */
export function useVariant(fallback: string): string {
  const read = () => new URLSearchParams(location.search).get("variant") || fallback;
  const [value, set] = useState(read);
  useEffect(() => {
    const on = () => set(read());
    addEventListener("prototype-variant", on);
    addEventListener("popstate", on);
    return () => { removeEventListener("prototype-variant", on); removeEventListener("popstate", on); };
  }, []);
  return value;
}
