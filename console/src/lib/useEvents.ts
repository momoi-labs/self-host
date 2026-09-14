import { useCallback, useEffect, useState } from "react";
import { api } from "./api.js";
import { normalizeEvent } from "./platformEvents.js";
import type { PlatformEvent, PlatformEventResponse } from "./platformEvents.js";

export function useEvents() {
  const [events, setEvents] = useState<PlatformEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const reload = useCallback(() => setRevision(value => value + 1), []);

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const response = await api("/events", { signal: controller.signal });
        if (!response.ok) throw new Error("Could not load event history.");
        const next: PlatformEventResponse[] = await response.json();
        if (!controller.signal.aborted) { setEvents(next.map(normalizeEvent)); setError(null); }
      } catch {
        if (!controller.signal.aborted) setError("Could not load event history. The displayed events may be out of date.");
      } finally {
        if (!controller.signal.aborted) {
          setLoading(false);
          timer = setTimeout(() => void poll(), 5000);
        }
      }
    };
    void poll();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [revision]);
  return { events, loading, error, reload };
}
