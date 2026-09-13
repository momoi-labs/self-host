import { Chart, type ChartSeries } from "@momoi-labs/kiso-react";
import { formatBytes } from "../lib/format.js";
import type { AppSample } from "../lib/types.js";

const series: ChartSeries[] = [
  { key: "download", label: "Received", slot: 1 },
  { key: "upload", label: "Sent", slot: 2 },
];

export function NetworkChart({ samples }: {
  samples: Pick<AppSample, "at" | "rx_bytes" | "tx_bytes">[];
}) {
  const peak = Math.max(1, ...samples.flatMap((sample) => [sample.rx_bytes, sample.tx_bytes]));
  // Round symmetric bounds so the shared axis includes zero at its center.
  const step = 10 ** Math.floor(Math.log10(peak));
  const ceiling = Math.ceil(peak / step) * step;
  return (
    <div className="network-chart">
      <Chart
        label="Network traffic"
        data={samples.map((sample) => ({
          timestamp: sample.at * 1000,
          values: { download: sample.rx_bytes, upload: -sample.tx_bytes },
        }))}
        series={series}
        layout="compact"
        height={112}
        legend="inline"
        min={-ceiling}
        max={ceiling}
        formatTime={(timestamp) => new Date(timestamp).toLocaleTimeString()}
        // Keep the byte value and unit together so the top tick cannot wrap and clip.
        formatValue={(value) => formatBytes(Math.abs(value)).replace(" ", "\u00a0")}
      />
    </div>
  );
}
