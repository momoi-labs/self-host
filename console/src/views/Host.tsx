import { lazy, Suspense } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Button, Card, EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateIcon, EmptyStateTitle,
  Lifecycle, PageHeader, PageHeaderDescription, PageHeaderTitle, Skeleton, StatusBadge, TerminalIcon,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { hostQuery } from "../lib/queries.js";

// xterm is only needed once the Operator opens this screen, so its chunk
// arrives then.
const Terminal = lazy(() => import("../components/Terminal.js").then((module) => ({ default: module.Terminal })));

/**
 * The machine the Platform runs on: a shell on it, and its screen, which opens
 * in a window of its own so the shell stays here. Each is off until the
 * Operator allows it in Settings.
 */
export function Host({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { data: host } = useQuery(hostQuery);
  const screen = host?.screen.supported && host.screen.allowed ? host.screen : null;

  function openScreen() {
    window.open("/console/#host-screen", "self-host-host-screen", "popup,width=1440,height=900");
  }

  return (
    <>
      <div className="between">
        <PageHeader>
          <PageHeaderTitle>Host</PageHeaderTitle>
          <PageHeaderDescription>The machine the Platform runs on.</PageHeaderDescription>
        </PageHeader>
        {screen ? (
          <Lifecycle
            status={
              <StatusBadge tone={screen.listening ? "success" : "neutral"}>
                {screen.listening ? "Screen Sharing on" : "Screen Sharing off"}
              </StatusBadge>
            }
            actions={<Button size="sm" onClick={openScreen}><Icon name="screen" />Open screen</Button>}
          />
        ) : null}
      </div>
      {/* One pane, so no tab bar: a lone tab is not a choice. */}
      <Card className="detail-tabs">
        {!host ? (
          <Skeleton className="terminal-loading" />
        ) : host.terminal.allowed ? (
          <Suspense fallback={<Skeleton className="terminal-loading" />}>
            <Terminal host account={host.terminal.account ?? undefined} />
          </Suspense>
        ) : (
          <div className="container-terminal">
            <EmptyState variant="first-run" className="hatch terminal-empty">
              <EmptyStateIcon><TerminalIcon /></EmptyStateIcon>
              <EmptyStateTitle>Host terminal</EmptyStateTitle>
              <EmptyStateDescription>
                Host terminal is off. Turn it on in Settings to open a shell on the Host here.
              </EmptyStateDescription>
              <EmptyStateActions>
                <Button variant="primary" onClick={onOpenSettings}>Open Settings</Button>
              </EmptyStateActions>
            </EmptyState>
          </div>
        )}
      </Card>
    </>
  );
}
