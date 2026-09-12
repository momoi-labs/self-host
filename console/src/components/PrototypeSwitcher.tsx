// PROTOTYPE. Floating bar that cycles ?variant= on the current route.
// Rendered only in dev builds.

import { useEffect, useState } from "react";

export function readVariant(): string | null {
  return new URLSearchParams(location.search).get("variant");
}

function writeVariant(key: string) {
  const url = new URL(location.href);
  url.searchParams.set("variant", key);
  // history.state carries the console's view; keep it so Back still works.
  history.replaceState(history.state, "", url);
}

export function PrototypeSwitcher({ variants, current, onChange, state }: {
  variants: { key: string; name: string }[];
  current: string;
  onChange: (key: string) => void;
  state?: unknown;
}) {
  const [showState, setShowState] = useState(false);
  if (import.meta.env.PROD) return null;
  const index = Math.max(0, variants.findIndex((variant) => variant.key === current));

  function go(offset: number) {
    const next = variants[(index + offset + variants.length) % variants.length]!.key;
    writeVariant(next);
    onChange(next);
  }

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable]")) return;
      if (event.key === "ArrowLeft") go(-1);
      if (event.key === "ArrowRight") go(1);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="proto-bar" role="toolbar" aria-label="Prototype variants">
      {showState ? <pre className="proto-state">{JSON.stringify(state, null, 2)}</pre> : null}
      <div className="proto-bar-row">
        <button type="button" onClick={() => go(-1)} aria-label="Previous variant">←</button>
        <span className="proto-bar-label">
          <strong>{variants[index]!.key}</strong> {variants[index]!.name}
          <span className="proto-bar-count">{index + 1}/{variants.length}</span>
        </span>
        <button type="button" onClick={() => go(1)} aria-label="Next variant">→</button>
        <button type="button" className="proto-bar-state" onClick={() => setShowState((value) => !value)}>
          {showState ? "hide state" : "state"}
        </button>
      </div>
    </div>
  );
}
