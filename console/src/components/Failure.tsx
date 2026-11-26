import { Alert, AlertContent, AlertDetails, AlertHelp, AlertLabel, AlertMessage, AlertTitle, Button } from "@momoi-labs/kiso-react";

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
  incident,
  help,
}: {
  failure: unknown;
  actionLabel?: string;
  onAction?: () => void;
  incident?: string;
  help?: string;
}) {
  const report = asReport(failure);
  const action = actionLabel ? (
    <Button size="sm" onClick={onAction}>
      {actionLabel}
    </Button>
  ) : null;
  if (incident) {
    return (
      <Alert variant="error" appearance="loud" tape="top" animated pulse={false} placement="inline" labels={{ show: "Show causes", hide: "Hide causes" }}>
        <AlertContent>
          <AlertLabel>Error · {incident}</AlertLabel>
          <AlertMessage>{report.error}</AlertMessage>
          {help ? <AlertHelp action={action}>{help}</AlertHelp> : action}
        </AlertContent>
        {report.caused_by.length ? (
          <AlertDetails>
            <Causes report={report} />
          </AlertDetails>
        ) : null}
      </Alert>
    );
  }
  return (
    <Alert variant="error">
      <Icon name="alert" size="md" />
      <AlertContent>
        <AlertTitle>{report.error}</AlertTitle>
        <Causes report={report} />
      </AlertContent>
      {action}
    </Alert>
  );
}
