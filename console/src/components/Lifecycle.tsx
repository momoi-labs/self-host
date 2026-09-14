import type { ReactNode } from "react";

/**
 * The header row of a detail screen: what the thing is, what you can do to it,
 * and the one action you do not want to hit by accident.
 *
 * Three groups rather than one line of siblings. Status is read, so its
 * cluster carries no shadow and each badge keeps the tone surface that carries
 * its meaning. The verbs are pressed, so their cluster carries the surface and
 * the shadow each button used to carry alone, and the hairline between them is
 * what says they are alternatives rather than a sequence. The destructive
 * action sits outside both with a wider gap, so a slip on Restart cannot land
 * on Delete.
 *
 * A screen that offers no verbs — a custom image is built, not run — passes
 * none, and the middle group is dropped rather than rendered empty: a bordered
 * box with nothing in it reads as something that failed to load.
 */
export function Lifecycle({ status, actions, destructive }: {
  status: ReactNode;
  actions?: ReactNode;
  destructive?: ReactNode;
}) {
  return (
    <div className="lifecycle">
      <div className="cluster cluster-status" role="group" aria-label="Status">
        {status}
      </div>
      {actions ? (
        <div className="cluster cluster-verbs" role="group" aria-label="Lifecycle">
          {actions}
        </div>
      ) : null}
      {destructive}
    </div>
  );
}
