import { Diagram, DiagramColumn, DiagramEdge, DiagramNode, GridPane, KV, KVKey, KVValue, PaneGrid } from "@momoi-labs/kiso-react";
import { HardDrive } from "lucide-react";

import { formatBytes } from "../lib/format.js";
import type { App, DatabaseDetail } from "../lib/types.js";
import { useStoredLayout } from "../lib/useStoredLayout.js";
import { diagramLook, Fact, opens, statusOf } from "./AppSummary.js";
import { GroupCard } from "./GroupCard.js";

/**
 * A database's Summary: the Applications connected to it, each with the
 * variable its URL arrives in, drawn into it, its volume out of it, then the
 * image it runs and how much its volume holds.
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
      <Diagram className="app-diagram" label={`${app.name}: the Applications connected to it and the volume it stores data in`} {...diagramLook}>
        <DiagramColumn>
          {connections.length ? (
            <DiagramNode id="consumers" title="" className="group-node">
              <GroupCard title="Applications" sections={[{
                key: "connected", title: "Connected", icon: "box", onAdd: onOpenConnections,
                lines: connections.map((connection) => {
                  const consumer = apps.find((one) => one.id === connection.consumer_application_id);
                  return {
                    key: connection.id, label: consumer?.name ?? "Removed application", env: connection.variable,
                    to: connection.status === "ready" ? undefined : connection.status,
                    hint: `${consumer?.name ?? "The Application"} reads ${app.name}'s connection URL from the environment variable ${connection.variable}.`,
                    onOpen: consumer ? () => onOpenApp(consumer.id) : undefined,
                  };
                }),
              }]} />
            </DiagramNode>
          ) : (
            <DiagramNode id="consumers" kind="service" label="Applications" title="None connected" borderStyle="dash" {...opens(onOpenConnections)}>
              {running ? "Connect one" : "Connect one once it runs"}
            </DiagramNode>
          )}
        </DiagramColumn>
        <DiagramColumn>
          <DiagramNode id="database" className="facts-node" kind="database" label={`PostgreSQL ${database?.major ?? app.managed_postgres?.major ?? ""}`} title={app.name}
            accent status={statusOf(app.status)}>
            <span className="group-section is-flat">
              <Fact icon="box" label="Image" value={app.image} />
              <Fact icon="plug" label="Port" value=":5432" />
              <Fact icon="lock" label="Network" value={running ? "Private only" : undefined} empty="Not running" />
            </span>
          </DiagramNode>
        </DiagramColumn>
        <DiagramColumn>
          <DiagramNode id="volume" className="facts-node" icon={HardDrive} label="Volume" title={size === null ? "Size unknown" : formatBytes(size)}>
            <span className="group-section is-flat">
              <Fact icon="database" label="Name" value={database?.volume ?? app.managed_postgres?.volume} empty="Unknown" />
            </span>
          </DiagramNode>
        </DiagramColumn>
        <DiagramEdge from="consumers" to="database" line={connections.length ? undefined : "dashed"} />
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
