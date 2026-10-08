import { useCallback, useState } from "react";
import type { PaneGridLayout } from "@momoi-labs/kiso-react";

/**
 * A PaneGrid layout the Operator arranged, kept in this browser under one key
 * per screen. A layout that cannot be read starts from the panes' defaults;
 * one that cannot be written lasts for this visit.
 */
export function useStoredLayout(key: string): [PaneGridLayout | undefined, (layout: PaneGridLayout) => void] {
  const storageKey = `self-host:layout:${key}`;
  const [layout] = useState<PaneGridLayout | undefined>(() => {
    try {
      const saved = localStorage.getItem(storageKey);
      return saved ? (JSON.parse(saved) as PaneGridLayout) : undefined;
    } catch {
      return undefined;
    }
  });
  const save = useCallback((next: PaneGridLayout) => {
    try {
      localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      // Storage full or blocked: the arrangement holds until the page reloads.
    }
  }, [storageKey]);
  return [layout, save];
}
