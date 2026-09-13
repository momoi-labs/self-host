import { Meter } from "@momoi-labs/kiso-react";

import { formatBytes } from "../lib/format.js";
import type { Totals } from "../lib/useMetrics.js";

/** CPU and memory as fractions of the available capacity (ADR-0020). */
export function Meters({ totals }: { totals: Totals }) {
  return (
    <div className="meters">
      <Meter
        label="CPU"
        value={Number.isFinite(totals.cpuCapacity) && totals.cpuCapacity > 0
          ? (totals.cpu / totals.cpuCapacity) * 100 : null}
        valueText={`${totals.cpu.toFixed(2)}%`}
      />
      <Meter
        label="Memory"
        value={totals.memoryLimit > 0 ? (totals.memory / totals.memoryLimit) * 100 : null}
        valueText={`${formatBytes(totals.memory)} / ${formatBytes(totals.memoryLimit)}`}
      />
    </div>
  );
}
