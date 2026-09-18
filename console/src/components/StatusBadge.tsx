import { Badge, Dot } from "@momoi-labs/kiso-react";

import type { Tone } from "../lib/status.js";

/**
 * A status badge carries its own surface, so the tone has to name a badge
 * variant and not just a text colour. Pending is neutral: it is not good news
 * yet, and it is not bad news either. The dot pulses when something is being
 * worked on right now, a run or a build; a machine that is merely up holds
 * still.
 */
export function StatusBadge({
  tone,
  pulse = false,
  children,
}: {
  tone: Tone;
  pulse?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Badge variant={tone}>
      <Dot pulse={pulse || undefined} />
      {children}
    </Badge>
  );
}
