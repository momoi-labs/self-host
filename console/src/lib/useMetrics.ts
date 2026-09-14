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

/** One Virtual machine's series, oldest first, or `null` before its first
 * sample lands. A machine that is off is measured by nobody. */
export function machineSeriesFor(metrics: Metrics | null, id: string): AppSample[] | null {
  const series = metrics?.machines.find((candidate) => candidate.id === id);
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

/** Everything running on this Host, summed from each Application's and each
 * Virtual machine's newest sample. A machine is a workload on this Host like
 * any other, so the Host meter counts it. `null` until the first tick. */
export function hostTotals(metrics: Metrics | null): Totals | null {
  if (!metrics) return null;
  const latest = [...metrics.applications, ...metrics.machines]
    .map((series) => series.samples[series.samples.length - 1])
    .filter((sample): sample is AppSample => Boolean(sample));
  return latest.length ? totalsOf(metrics, latest) : null;
}

/** One Application's newest reading, against the same Host ceilings. */
export function appTotals(metrics: Metrics | null, id: string): Totals | null {
  const samples = seriesFor(metrics, id);
  return metrics && samples ? totalsOf(metrics, [samples[samples.length - 1]]) : null;
}

/** One Virtual machine's newest reading. Its memory ceiling is its own, not
 * the Host's: the hypervisor reserved it at boot. */
export function machineTotals(metrics: Metrics | null, id: string): Totals | null {
  const samples = machineSeriesFor(metrics, id);
  return metrics && samples ? totalsOf(metrics, [samples[samples.length - 1]]) : null;
}

/** One machine's network series, for its chart. */
export function machineNetworkSeries(metrics: Metrics | null, id: string): Pick<AppSample, "at" | "rx_bytes" | "tx_bytes">[] {
  const samples = machineSeriesFor(metrics, id) ?? [];
  return samples.map(({ at, rx_bytes, tx_bytes }) => ({ at, rx_bytes, tx_bytes }));
}

/** Sum only readings from the same collection tick, preserving their
 * timestamps. A machine is a workload on this Host like any other, so the
 * Host's line counts it. */
export function hostNetworkSeries(metrics: Metrics | null): Pick<AppSample, "at" | "rx_bytes" | "tx_bytes">[] {
  const ticks = new Map<number, Pick<AppSample, "at" | "rx_bytes" | "tx_bytes">>();
  for (const workload of [...(metrics?.applications ?? []), ...(metrics?.machines ?? [])]) {
    for (const sample of workload.samples) {
      const total = ticks.get(sample.at) ?? { at: sample.at, rx_bytes: 0, tx_bytes: 0 };
      total.rx_bytes += sample.rx_bytes;
      total.tx_bytes += sample.tx_bytes;
      ticks.set(sample.at, total);
    }
  }
  return [...ticks.values()].sort((a, b) => a.at - b.at);
}
