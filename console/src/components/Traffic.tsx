import { Chart, type ChartSeries } from "@momoi-labs/kiso-react";

import type { PlatformSample } from "../lib/types.js";

const series: ChartSeries[] = [
  { key: "proxy", label: "Proxy", slot: 1 },
  { key: "dns", label: "DNS", slot: 2 },
];

/** Proxy and DNS count events over the same interval and share one scale (ADR-0020). */
export function Traffic({
  platform,
  intervalSeconds,
}: {
  platform: PlatformSample[];
  intervalSeconds: number;
}) {
  const perMinute = (count: number) => Math.round((count * 60) / Math.max(1, intervalSeconds));
  const data = platform.map((sample) => ({
    timestamp: sample.at * 1000,
    values: { proxy: perMinute(sample.proxy_requests), dns: perMinute(sample.dns_queries) },
  }));
  const latest = platform[platform.length - 1];

  return (
    <div className="traffic">
      <Chart
        label="Platform traffic"
        data={data}
        series={series}
        layout="compact"
        height={112}
        legend="inline"
        min={0}
        max={Math.max(1, ...data.flatMap((sample) => [sample.values.proxy, sample.values.dns]))}
        formatTime={(timestamp) => new Date(timestamp).toLocaleTimeString()}
        formatValue={(value) => `${value}/min`}
      />
      {latest?.proxy_errors ? (
        <p className="traffic-errors">{perMinute(latest.proxy_errors)} errors/min</p>
      ) : null}
    </div>
  );
}
