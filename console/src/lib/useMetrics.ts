import { useEffect, useState } from "react";

import { getJson } from "./api.js";
import type { AppSample, ContainerSeries, Metrics } from "./types.js";

/** What the collector ticks at until the daemon has said otherwise. */
const ASSUMED_INTERVAL_SECONDS = 10;

/**
 * What the daemon collected about itself and its Applications, polled at
 * half the collector's interval so a fresh sample is never missed. The
 * interval is the daemon's to set, so the first answer sets the cadence.
 */
export function useMetrics(): Metrics | null {
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const interval = metrics?.interval_seconds ?? ASSUMED_INTERVAL_SECONDS;

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      const next = await getJson<Metrics>("/metrics");
      if (!cancelled && next) setMetrics(next);
    };
    void load();
    const timer = window.setInterval(() => void load(), (interval * 1000) / 2);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [interval]);

  return metrics;
}

/** The series of one Application, oldest first, or `null` before the first
 * sample lands (up to a minute after the Application starts). */
export function seriesFor(metrics: Metrics | null, id: string): AppSample[] | null {
  const series = metrics?.applications.find((candidate) => candidate.id === id);
  return series && series.samples.length > 0 ? series.samples : null;
}

/** The same window per container, busiest first, so the service eating the
 * Host is the one at the top rather than the one named first. */
export function containersFor(metrics: Metrics | null, id: string): ContainerSeries[] {
  const series = metrics?.applications.find((candidate) => candidate.id === id);
  return [...(series?.containers ?? [])]
    .filter((container) => container.samples.length > 0)
    .sort((a, b) => memoryOf(b) - memoryOf(a));
}

function memoryOf(series: ContainerSeries): number {
  return series.samples[series.samples.length - 1].memory_bytes;
}

/** The latest reading of one scope, with the Host's ceilings alongside it.
 * `cpu_percent` counts a fully used core as 100, so the Host's share is it
 * over `cpuCapacity`. */
export type Totals = {
  cpu: number;
  cpuCapacity: number;
  memory: number;
  memoryLimit: number;
  rx: number;
  tx: number;
};

function totalsOf(metrics: Metrics, samples: AppSample[]): Totals {
  return {
    cpu: samples.reduce((sum, sample) => sum + sample.cpu_percent, 0),
    cpuCapacity: Math.max(1, metrics.host_cpus || 1) * 100,
    memory: samples.reduce((sum, sample) => sum + sample.memory_bytes, 0),
    // A ceiling is not additive: the daemon already reports the largest one,
    // and across Applications the largest is the Host's own memory.
    memoryLimit: samples.reduce((most, sample) => Math.max(most, sample.memory_limit_bytes), 0),
    rx: samples.reduce((sum, sample) => sum + sample.rx_bytes, 0),
    tx: samples.reduce((sum, sample) => sum + sample.tx_bytes, 0),
  };
}

/** Everything running on this Host, summed from each Application's newest
 * sample. `null` until the collector's first tick. */
export function hostTotals(metrics: Metrics | null): Totals | null {
  if (!metrics) return null;
  const latest = metrics.applications
    .map((series) => series.samples[series.samples.length - 1])
    .filter((sample): sample is AppSample => Boolean(sample));
  return latest.length ? totalsOf(metrics, latest) : null;
}

/** One Application's newest reading, against the same Host ceilings. */
export function appTotals(metrics: Metrics | null, id: string): Totals | null {
  const samples = seriesFor(metrics, id);
  return metrics && samples ? totalsOf(metrics, [samples[samples.length - 1]]) : null;
}

/** Every Application's network series, summed tick by tick, for the Host
 * meter's one line. Series can differ in length after a deploy, so they are
 * aligned on the newest sample and truncated to the shortest. */
export function hostNetworkSeries(metrics: Metrics | null): number[] {
  const series = (metrics?.applications ?? [])
    .map((app) => app.samples)
    .filter((samples) => samples.length > 0);
  if (!series.length) return [];
  const length = Math.min(...series.map((samples) => samples.length));
  return Array.from({ length }, (_, index) =>
    series.reduce((sum, samples) => {
      const sample = samples[samples.length - length + index];
      return sum + sample.rx_bytes + sample.tx_bytes;
    }, 0),
  );
}
