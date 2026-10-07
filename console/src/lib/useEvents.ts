import { useCallback } from "react";
import { useQuery } from "@tanstack/react-query";

import { api } from "./api.js";
import type { PlatformEvent } from "./platformEvents.js";

/** The whole history, newest first, as the API answers it. */
export async function fetchEvents(signal?: AbortSignal): Promise<PlatformEvent[]> {
  const response = await api("/events", { signal });
  if (!response.ok) throw new Error("Could not load event history.");
  return (await response.json()) as PlatformEvent[];
}

const noEvents: PlatformEvent[] = [];

export function useEvents() {
  const { data, isPending, isError, refetch } = useQuery({
    queryKey: ["events"],
    queryFn: ({ signal }) => fetchEvents(signal),
    refetchInterval: 5000,
  });
  const reload = useCallback(() => void refetch(), [refetch]);
  return {
    events: data ?? noEvents,
    loading: isPending,
    error: isError ? "Could not load event history. The displayed events may be out of date." : null,
    reload,
  };
}
