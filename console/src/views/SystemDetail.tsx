import {
  Card,
  KV,
  KVKey,
  KVValue,
  Pane,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Split,
  Splitter,
} from "@momoi-labs/kiso-react";

import { Logs } from "../components/Logs.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { statusTone } from "../lib/status.js";
import type { SystemContainer } from "../lib/types.js";

/**
 * The Platform Infra detail is the read-only twin of the Application detail:
 * the same split, with the edit form replaced by a definition list. No Remove,
 * no Save — the absence of the actions is what says read-only.
 */
export function SystemDetail({ container }: { container: SystemContainer }) {
  const restarts =
    container.restarts === null || container.restarts === undefined
      ? "—"
      : String(container.restarts);

  return (
    <>
      <div className="between">
        <PageHeader>
          <PageHeaderTitle>{container.role}</PageHeaderTitle>
          <PageHeaderDescription>
            <span className="mono">{container.name}</span>
          </PageHeaderDescription>
        </PageHeader>
        <StatusBadge tone={statusTone(container.status)}>{container.status}</StatusBadge>
      </div>

      <Card className="detail-panel">
        <Split>
          <Pane>
            <p className="t-caps">Configuration</p>
            <KV>
              <KVKey>Role</KVKey>
              <KVValue>{container.role}</KVValue>
              <KVKey>Container</KVKey>
              <KVValue>{container.name}</KVValue>
              <KVKey>Image</KVKey>
              <KVValue>{container.image}</KVValue>
              <KVKey>Restarts</KVKey>
              <KVValue>{restarts}</KVValue>
            </KV>
          </Pane>
          <Splitter
            defaultSize={42}
            min={25}
            max={70}
            aria-label="Resize the configuration and the logs"
          />
          <Pane className="pane-logs">
            <p className="t-caps">Logs from {container.name}</p>
            <Logs
              label="Logs"
              url={`/system/${encodeURIComponent(container.role)}/logs`}
            />
          </Pane>
        </Split>
      </Card>
    </>
  );
}
