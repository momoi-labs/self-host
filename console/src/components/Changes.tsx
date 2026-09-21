import { Fragment } from "react";

export type Change = { setting: string; from: string; to: string };

/**
 * What a change to the settings reads like, before it is applied and in the
 * event that recorded it: a git-style diff, one removed and one added line
 * per setting, so the Operator sees the same thing in both places.
 */
export function Changes({ changes }: { changes: Change[] }) {
  return (
    <pre className="changes-diff" aria-label="Changes">
      {changes.map((change) => (
        <Fragment key={change.setting}>
          <span className="changes-diff-removed">- {change.setting}: {change.from}</span>
          {"\n"}
          <span className="changes-diff-added">+ {change.setting}: {change.to}</span>
          {"\n"}
        </Fragment>
      ))}
    </pre>
  );
}
