import { useCallback, useEffect, useState } from "react";
import { api } from "./api.js";
import type { PlatformEvent } from "./platformEvents.js";

/** The whole history, newest first, as the API answers it. */
export async function fetchEvents(): Promise<PlatformEvent[]> {
  const response = await api("/events");
  if (!response.ok) throw new Error("Could not load event history.");
  return (await response.json()) as PlatformEvent[];
}

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
        const next: PlatformEvent[] = await response.json();
        if (!controller.signal.aborted) { setEvents(next); setError(null); }
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
