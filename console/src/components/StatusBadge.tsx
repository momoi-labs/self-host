import { Badge, Dot } from "@momoi-labs/kiso-react";

import type { Tone } from "../lib/status.js";

/**
 * A status badge carries its own surface, so the tone has to name a badge
 * variant and not just a text colour. Pending is neutral: it is not good news
 * yet, and it is not bad news either.
 */
export function StatusBadge({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return (
    <Badge variant={tone}>
      <Dot />
      {children}
    </Badge>
  );
}
