import type { MouseEvent } from "react";
import { Sparkline, StatusBadge, StepBar, TableCell } from "@momoi-labs/kiso-react";

import { NOUNS, TITLES, barSteps, phase, position } from "../lib/runSteps.js";
import type { AppSample, Environment } from "../lib/types.js";

export function plainClick(event: MouseEvent): boolean {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

export function Activity({ operation }: { operation: NonNullable<Environment["operation"]> }) {
  const failed = operation.status === "failed";
  const said = failed
    ? `${NOUNS[operation.action] ?? operation.action} failed`
    : phase(operation.action, operation.step);
  const place = position(operation.action, operation.step);
  return (
    <span
      className="activity-cell"
      title={operation.step ? `${said}: ${operation.step}` : said}
    >
      <StatusBadge tone={failed ? "danger" : "success"} pulse={!failed}>
        {said}
      </StatusBadge>
      <StepBar
        label={TITLES[operation.action] ?? operation.action}
        steps={barSteps(operation.action, operation.step, failed)}
      />
      {place ? (
        <span className="mono muted t-label">
          {place.at}/{place.of}
        </span>
      ) : null}
    </span>
  );
}

export function Usage({
  samples,
  unmeasured,
  value,
  render,
  className,
}: {
  samples: AppSample[] | null;
  unmeasured?: boolean;
  value: (sample: AppSample) => number;
  render: (sample: AppSample) => string;
  className?: string;
}) {
  if (unmeasured) {
    return (
      <TableCell className={className ? `muted ${className}` : "muted"} title="This Host has no resource controls for native processes">Not measured</TableCell>
    );
  }
  if (!samples) {
    return <TableCell className={className ? `muted ${className}` : "muted"}>—</TableCell>;
  }
  return (
    <TableCell className={className}>
      <span className="usage-inline">
        <span className="usage-value">{render(samples[samples.length - 1])}</span>
        <Sparkline values={samples.map(value)} height={16} />
      </span>
    </TableCell>
  );
}
