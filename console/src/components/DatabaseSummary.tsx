import { Diagram, DiagramColumn, DiagramEdge, DiagramNode, GridPane, KV, KVKey, KVValue, PaneGrid } from "@momoi-labs/kiso-react";
import { HardDrive } from "lucide-react";

import { formatBytes } from "../lib/format.js";
import type { App, DatabaseDetail } from "../lib/types.js";
import { useStoredLayout } from "../lib/useStoredLayout.js";
import { diagramLook, nodeOf, opens, statusOf } from "./AppSummary.js";

/**
 * A database's Summary: the Applications connected to it drawn into it, its
 * volume out of it, then the image it runs and how much its volume holds.
 */
export function DatabaseSummary({ app, apps, database, onOpenApp, onOpenConnections }: {
  app: App;
  apps: App[];
  /** Null until the detail first loads. */
  database: DatabaseDetail | null;
  onOpenApp: (id: string) => void;
  onOpenConnections: () => void;
}) {
  const [layout, saveLayout] = useStoredLayout("summary:database");
  const running = app.status === "running";
  const connections = database?.connections.filter((connection) => connection.status !== "revoked") ?? [];
  const size = database?.volume_bytes ?? null;

  return (
    <div className="stack">
      <Diagram label={`${app.name}: the Applications connected to it and the volume it stores data in`} {...diagramLook}>
        <DiagramColumn>
          {connections.length ? connections.map((connection) => {
            const consumer = apps.find((one) => one.id === connection.consumer_application_id);
            const node = consumer ? nodeOf(consumer) : null;
            return (
              <DiagramNode key={connection.id} id={connection.id} kind={node?.kind ?? "service"} icon={node?.icon} label={node?.label ?? "Application"}
                title={consumer?.name ?? "Removed application"} status={consumer ? statusOf(consumer.status) : undefined}
                {...(consumer ? opens(() => onOpenApp(consumer.id)) : {})}>
                {connection.status === "ready" ? undefined : connection.status}
              </DiagramNode>
            );
          }) : (
            <DiagramNode id="none" kind="service" label="Applications" title="None connected" borderStyle="dash" {...opens(onOpenConnections)}>
              {running ? "Connect one" : "Connect one once it runs"}
            </DiagramNode>
          )}
        </DiagramColumn>
        <DiagramColumn>
          <DiagramNode id="database" kind="database" label={`PostgreSQL ${database?.major ?? app.managed_postgres?.major ?? ""}`} title={app.name}
            accent status={statusOf(app.status)}>
            {running ? "Private network only" : "Not running"}
          </DiagramNode>
        </DiagramColumn>
        <DiagramColumn>
          <DiagramNode id="volume" icon={HardDrive} label="Volume" title={size === null ? "Size unknown" : formatBytes(size)}>
            {database?.volume ?? app.managed_postgres?.volume}
          </DiagramNode>
        </DiagramColumn>
        {connections.length ? connections.map((connection) => (
          <DiagramEdge key={connection.id} from={connection.id} to="database" label={connection.variable}
            line={connection.status === "ready" ? undefined : "dashed"} />
        )) : <DiagramEdge from="none" to="database" line="dashed" />}
        <DiagramEdge from="database" to="volume" />
      </Diagram>

      <PaneGrid aria-label={`${app.name} summary`} defaultLayout={layout} onLayoutChange={saveLayout}>
        <GridPane id="what-runs" title="What runs" size={6}>
          <KV>
            <KVKey>Image</KVKey><KVValue>{app.image}</KVValue>
            <KVKey>Volume size</KVKey><KVValue>{size === null ? <span className="muted">Unknown</span> : formatBytes(size)}</KVValue>
          </KV>
        </GridPane>
      </PaneGrid>
    </div>
  );
}
