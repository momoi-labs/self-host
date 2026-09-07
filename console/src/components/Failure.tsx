import { Alert, AlertContent, AlertTitle, Button } from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { Causes } from "./Causes.js";
import { Icon } from "./Icon.js";

/**
 * The single place an error gets rendered. Title says what failed, the causes
 * say why, and the action is the way out — the same shape every time.
 */
export function Failure({
  failure,
  actionLabel,
  onAction,
}: {
  failure: unknown;
  actionLabel?: string;
  onAction?: () => void;
}) {
  const report = asReport(failure);
  return (
    <Alert variant="error">
      <Icon name="alert" size="md" />
      <AlertContent>
        <AlertTitle>{report.error}</AlertTitle>
        <Causes report={report} />
      </AlertContent>
      {actionLabel ? (
        <Button size="sm" onClick={onAction}>
          {actionLabel}
        </Button>
      ) : null}
    </Alert>
  );
}
