import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
  Card,
  EmptyState,
  EmptyStateActions,
  EmptyStateDescription,
  EmptyStateIcon,
  EmptyStateTitle,
  FormField,
  Label,
  LogView,
  LogViewLine,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@momoi-labs/kiso-react";

import { Dependencies } from "../components/Dependencies.js";
import { Failure } from "../components/Failure.js";
import { Icon } from "../components/Icon.js";
import { ShellEditor } from "../components/ShellEditor.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { api, asReport, failureOf } from "../lib/api.js";
import {
  devImageTemplate,
  devImageTemplates,
  type ImageDependency,
} from "../lib/devImageTemplates.js";
import { isVersion } from "../lib/dependencies.js";
import type { Report } from "../lib/types.js";

type EnvironmentConfig = {
  name: string;
  cpus: number;
  memory_gib: number;
  disk_gib: number;
  ssh_public_key: string;
  recipe: {
    name: string;
    template_id?: string | null;
    dependencies: ImageDependency[];
    build_checks: string[];
  };
  command: string;
  web_port: number;
};
type Operation = {
  action: string;
  status: string;
  step?: string | null;
  error?: Report | null;
} | null;
type Environment = {
  id: string;
  config: EnvironmentConfig;
  applied_config?: EnvironmentConfig;
  state: string;
  service_ready: boolean;
  operation: Operation;
  log: string;
  ssh_command: string;
  tunnel_command?: string;
  web_url: string;
  installed_versions?: unknown;
};

const demoConfig = (name: string): EnvironmentConfig => ({
  name,
  cpus: 4,
  memory_gib: 8,
  disk_gib: 40,
  ssh_public_key: "ssh-ed25519 AAAA... operator@host",
  recipe: {
    name: "T3 Code",
    template_id: "t3-code",
    dependencies: structuredClone(
      devImageTemplate("t3-code")?.dependencies ?? [],
    ),
    build_checks: structuredClone(
      devImageTemplate("t3-code")?.buildChecks ?? [],
    ),
  },
  command:
    "t3 serve --mode web --host 0.0.0.0 --port 3000 --base-dir /home/dev/.local/state/t3",
  web_port: 3000,
});
const demoEnvironments: Environment[] = [
  {
    id: "demo-running",
    config: demoConfig("t3-workbench"),
    state: "running",
    service_ready: true,
    operation: null,
    log: "Virtual machine booted\nSSH is ready\nT3 is responding on port 3000",
    ssh_command: "ssh operator@host -p 2222",
    tunnel_command: "ssh -N -L 3000:127.0.0.1:3000 operator@host -p 2222",
    web_url: "http://127.0.0.1:3000",
    installed_versions: {
      node: "24.8.0",
      t3: "0.4.1",
      claude: "1.2.0",
      codex: "0.31.0",
    },
  },
  {
    id: "demo-starting",
    config: demoConfig("research-box"),
    state: "running",
    service_ready: false,
    operation: {
      action: "bootstrap",
      status: "running",
      step: "Checking T3 HTTP readiness",
    },
    log: "Virtual machine booted\nInstalling tools\nChecking T3 HTTP readiness...",
    ssh_command: "ssh operator@host -p 2223",
    tunnel_command: "ssh -N -L 3001:127.0.0.1:3000 operator@host -p 2223",
    web_url: "http://127.0.0.1:3001",
    installed_versions: {},
  },
  {
    id: "demo-failed",
    config: demoConfig("broken-bootstrap"),
    state: "stopped",
    service_ready: false,
    operation: {
      action: "bootstrap",
      status: "failed",
      step: "Installing mise dependencies",
      error: {
        error: "Bootstrap failed",
        caused_by: ["npm:t3 could not be downloaded"],
      },
    },
    log: "Virtual machine created\nInstalling mise dependencies\nERROR network request failed",
    ssh_command: "ssh operator@host -p 2224",
    tunnel_command: "ssh -N -L 3002:127.0.0.1:3000 operator@host -p 2224",
    web_url: "http://127.0.0.1:3002",
    installed_versions: {},
  },
];

function freshConfig(): EnvironmentConfig {
  const template = devImageTemplate("t3-code");
  return {
    ...demoConfig(""),
    name: "",
    ssh_public_key: "",
    recipe: {
      name: template?.name ?? "T3 Code",
      template_id: template?.id,
      dependencies: structuredClone(template?.dependencies ?? []),
      build_checks: structuredClone(template?.buildChecks ?? []),
    },
  };
}
function tone(state: string) {
  return state === "running"
    ? ("success" as const)
    : state === "failed"
      ? ("danger" as const)
      : ("neutral" as const);
}
function actionBusy(environment: Environment) {
  return !!environment.operation && environment.operation.status === "running";
}
function equalConfig(a: EnvironmentConfig, b: EnvironmentConfig) {
  return JSON.stringify(a) === JSON.stringify(b);
}
function installedVersions(value: unknown): [string, string][] {
  if (!value || typeof value !== "object" || Array.isArray(value))
    return value == null ? [] : [["details", readableVersion(value)]];
  return Object.entries(value).map(([name, version]) => [
    name,
    readableVersion(version),
  ]);
}
function readableVersion(value: unknown): string {
  if (typeof value === "string" || typeof value === "number")
    return String(value);
  if (Array.isArray(value))
    return value.map(readableVersion).filter(Boolean).join(", ");
  if (value && typeof value === "object") {
    const version = (value as { version?: unknown }).version;
    if (typeof version === "string" || typeof version === "number")
      return String(version);
  }
  return JSON.stringify(value);
}
function normalizeEnvironment(value: Environment): Environment {
  return {
    ...value,
    operation: value.operation
      ? {
          ...value.operation,
          step: value.operation.step || value.operation.status,
        }
      : null,
    installed_versions: Object.fromEntries(
      installedVersions(value.installed_versions),
    ),
  };
}

export function Environments({
  mode,
  selected,
  onOpen,
  onNew,
}: {
  mode: "list" | "new" | "detail";
  selected: string | null;
  onOpen: (id: string | null) => void;
  onNew: () => void;
}) {
  const demo =
    new URLSearchParams(location.search).get("demo") === "environments";
  const [records, setRecords] = useState<Environment[]>(
    demo ? demoEnvironments : [],
  );
  const [loaded, setLoaded] = useState(demo);
  const [loadFailure, setLoadFailure] = useState<Report | null>(null);
  const [failure, setFailure] = useState<Report | null>(null);
  const [config, setConfig] = useState<EnvironmentConfig>(freshConfig);
  const [savedConfig, setSavedConfig] = useState<EnvironmentConfig | null>(
    null,
  );
  const [appliedConfig, setAppliedConfig] = useState<EnvironmentConfig | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<Environment | null>(null);
  const [confirmName, setConfirmName] = useState("");
  const requestId = useRef<string | null>(null);
  const editorInitialized = useRef<string | null>(null);
  const current = records.find((item) => item.id === selected);
  const configDirty = !!savedConfig && !equalConfig(config, savedConfig);
  const pendingApply =
    !!savedConfig &&
    !!appliedConfig &&
    !equalConfig(savedConfig, appliedConfig);

  useEffect(() => {
    if (demo) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const response = await api("/environments");
        if (!response.ok) throw await failureOf(response);
        const next = ((await response.json()) as Environment[]).map(
          normalizeEnvironment,
        );
        if (active) {
          setRecords(next);
          setLoaded(true);
          setLoadFailure(null);
        }
      } catch (cause) {
        if (active) setLoadFailure(asReport(cause));
      } finally {
        if (active) timer = setTimeout(poll, 1500);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [demo]);

  useEffect(() => {
    if (mode === "new") {
      setConfig(freshConfig());
      setSavedConfig(null);
      setAppliedConfig(null);
      setFailure(null);
      requestId.current = null;
      editorInitialized.current = null;
      return;
    }
    if (!selected) return;
    const found = records.find((item) => item.id === selected);
    if (found) {
      setConfig(structuredClone(found.config));
      setSavedConfig(structuredClone(found.config));
      setAppliedConfig(structuredClone(found.applied_config ?? found.config));
      setFailure(null);
      editorInitialized.current = selected;
    }
    if (demo) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const response = await api(
          `/environments/${encodeURIComponent(selected)}`,
        );
        if (!response.ok) throw await failureOf(response);
        const next = normalizeEnvironment(
          (await response.json()) as Environment,
        );
        if (active) {
          setRecords((items) => [
            next,
            ...items.filter((item) => item.id !== next.id),
          ]);
          if (next.applied_config)
            setAppliedConfig(structuredClone(next.applied_config));
          if (editorInitialized.current !== selected) {
            setConfig(structuredClone(next.config));
            setSavedConfig(structuredClone(next.config));
            setAppliedConfig(
              structuredClone(next.applied_config ?? next.config),
            );
            editorInitialized.current = selected;
          }
        }
      } catch (cause) {
        if (active) setFailure(asReport(cause));
      } finally {
        if (active) timer = setTimeout(poll, 1500);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
    // selected changes identify a new editor. Record polling must preserve edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, selected]);

  // A durable operation may have started in another tab or before this page
  // loaded. Keep every editor action disabled while it is running.
  useEffect(() => {
    setBusy(current?.operation?.status === "running");
  }, [current?.operation?.status]);

  function patch(change: Partial<EnvironmentConfig>) {
    setConfig((value) => ({ ...value, ...change }));
  }
  function patchRecipe(change: Partial<EnvironmentConfig["recipe"]>) {
    setConfig((value) => ({
      ...value,
      recipe: { ...value.recipe, ...change },
    }));
  }
  function chooseTemplate(id: string) {
    const template = devImageTemplate(id);
    if (!template) return;
    patchRecipe({
      name: template.name,
      template_id: id,
      dependencies: structuredClone(template.dependencies),
      build_checks: structuredClone(template.buildChecks),
    });
    patch({
      command: `t3 serve --mode web --host 0.0.0.0 --port ${template.application.web_port} --base-dir /home/dev/.local/state/t3`,
      web_port: template.application.web_port,
    });
  }
  async function submitCreate(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setFailure(null);
    if (!requestId.current)
      requestId.current =
        globalThis.crypto?.randomUUID?.() ?? `create-${Date.now()}`;
    try {
      if (demo) {
        const record: Environment = {
          id: `demo-${Date.now()}`,
          config: structuredClone(config),
          state: "creating",
          service_ready: false,
          operation: {
            action: "create",
            status: "running",
            step: "Creating virtual machine",
          },
          log: "Creating virtual machine...",
          ssh_command: "ssh operator@host -p 2225",
          tunnel_command: "ssh -N -L 3003:127.0.0.1:3000 operator@host -p 2225",
          web_url: "",
          installed_versions: {},
        };
        setRecords((items) => [record, ...items]);
        onOpen(record.id);
        setTimeout(
          () =>
            setRecords((items) =>
              items.map((item) =>
                item.id === record.id
                  ? {
                      ...item,
                      state: "running",
                      service_ready: true,
                      operation: null,
                      log: `${item.log}\nSSH is ready\nT3 is responding`,
                    }
                  : item,
              ),
            ),
          900,
        );
      } else {
        const response = await api("/environments", {
          method: "POST",
          body: JSON.stringify({ request_id: requestId.current, config }),
        });
        if (!response.ok) throw await failureOf(response);
        const record = normalizeEnvironment(
          (await response.json()) as Environment,
        );
        setRecords((items) => [
          record,
          ...items.filter((item) => item.id !== record.id),
        ]);
        onOpen(record.id);
      }
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setBusy(false);
    }
  }
  async function saveConfig() {
    if (!current || busy || actionBusy(current)) return;
    setBusy(true);
    setFailure(null);
    try {
      if (!demo) {
        const response = await api(
          `/environments/${encodeURIComponent(current.id)}`,
          { method: "PUT", body: JSON.stringify(config) },
        );
        if (!response.ok) throw await failureOf(response);
      }
      setRecords((items) =>
        items.map((item) =>
          item.id === current.id
            ? { ...item, config: structuredClone(config) }
            : item,
        ),
      );
      setSavedConfig(structuredClone(config));
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setBusy(false);
    }
  }
  async function perform(action: string, environment = current, name?: string) {
    if (action === "apply_update" && configDirty) {
      setFailure({
        error: "Save configuration before applying the update.",
        caused_by: [],
      });
      return;
    }
    if (!environment || busy || actionBusy(environment)) return;
    setBusy(true);
    setFailure(null);
    try {
      if (demo) {
        setRecords((items) =>
          items.map((item) =>
            item.id !== environment.id
              ? item
              : {
                  ...item,
                  operation: {
                    action,
                    status: "running",
                    step:
                      action === "delete"
                        ? "Removing owned disk"
                        : `Running ${action}`,
                  },
                  log: `${item.log}\nStarting ${action}...`,
                },
          ),
        );
        setTimeout(
          () =>
            setRecords((items) =>
              items.flatMap((item) =>
                item.id !== environment.id
                  ? [item]
                  : action === "delete"
                    ? []
                    : [
                        {
                          ...item,
                          state: action === "stop" ? "stopped" : "running",
                          service_ready:
                            action === "start" ||
                            action === "restart" ||
                            action === "retry" ||
                            action === "bootstrap" ||
                            action === "apply_update",
                          operation: null,
                          log: `${item.log}\n${action} complete`,
                        },
                      ],
              ),
            ),
          900,
        );
      } else {
        const response = await api(
          `/environments/${encodeURIComponent(environment.id)}/actions`,
          {
            method: "POST",
            body: JSON.stringify({
              action,
              ...(name ? { confirm_name: name } : {}),
            }),
          },
        );
        if (!response.ok) throw await failureOf(response);
        const next = normalizeEnvironment(
          (await response.json()) as Environment,
        );
        setRecords((items) =>
          action === "delete" && next.state === "deleted"
            ? items.filter((item) => item.id !== environment.id)
            : items.map((item) => (item.id === environment.id ? next : item)),
        );
      }
      if (action === "delete") {
        setConfirming(null);
        setConfirmName("");
        onOpen(null);
      }
      if (action === "apply_update")
        setAppliedConfig(structuredClone(savedConfig ?? config));
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setBusy(false);
    }
  }

  if (mode === "list")
    return (
      <>
        <PageHeader>
          <PageHeaderTitle>Virtual machines</PageHeaderTitle>
          <PageHeaderDescription>
            Ubuntu with native T3 processes, SSH access, and saved tool recipes.
          </PageHeaderDescription>
        </PageHeader>
        {demo ? (
          <p className="demo-banner" role="status">
            <strong>Demo mode:</strong> memory-only state transitions. Live API
            actions are disabled.
          </p>
        ) : null}
        {loadFailure ? <Failure failure={loadFailure} /> : null}
        {!loaded ? (
          <p className="muted">Loading virtual machines...</p>
        ) : !records.length ? (
          <Card>
            <EmptyState variant="first-run" className="hatch">
              <EmptyStateIcon>
                <Icon name="box" size="lg" />
              </EmptyStateIcon>
              <EmptyStateTitle>No virtual machines yet</EmptyStateTitle>
              <EmptyStateDescription>
                Create a virtual machine for native development tools and T3.
              </EmptyStateDescription>
              <EmptyStateActions>
                <Button variant="primary" size="sm" onClick={onNew}>
                  Create virtual machine
                </Button>
              </EmptyStateActions>
            </EmptyState>
          </Card>
        ) : (
          <Card>
            <div className="table-wrap">
              <div className="table-scroll">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Name</TableHead>
                      <TableHead>State</TableHead>
                      <TableHead>T3</TableHead>
                      <TableHead>Operation</TableHead>
                      <TableHead>Resources</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {records.map((item) => (
                      <TableRow
                        key={item.id}
                        tabIndex={0}
                        role="button"
                        onClick={() => onOpen(item.id)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            onOpen(item.id);
                          }
                        }}
                      >
                        <TableCell>{item.config.name}</TableCell>
                        <TableCell>
                          <StatusBadge tone={tone(item.state)}>
                            {item.state}
                          </StatusBadge>
                        </TableCell>
                        <TableCell>
                          <StatusBadge
                            tone={item.service_ready ? "success" : "neutral"}
                          >
                            {item.service_ready ? "Ready" : "Unavailable"}
                          </StatusBadge>
                        </TableCell>
                        <TableCell>
                          {item.operation
                            ? `${item.operation.action}: ${item.operation.step}`
                            : "Idle"}
                        </TableCell>
                        <TableCell className="mono">
                          {item.config.cpus} CPU · {item.config.memory_gib} GiB
                          · {item.config.disk_gib} GiB
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            </div>
          </Card>
        )}
        <div className="form-actions">
          <Button variant="primary" size="sm" onClick={onNew}>
            Create virtual machine
          </Button>
        </div>
      </>
    );

  if (mode === "new")
    return (
      <EnvironmentEditor
        config={config}
        failure={failure}
        busy={busy}
        onChange={patch}
        onRecipeChange={patchRecipe}
        onTemplate={chooseTemplate}
        onCancel={() => onOpen(null)}
        onSubmit={submitCreate}
      />
    );
  if (!current)
    return (
      <Card>
        <EmptyState>
          <EmptyStateTitle>Virtual machine unavailable</EmptyStateTitle>
          <EmptyStateActions>
            <Button onClick={() => onOpen(null)}>Back to virtual machines</Button>
          </EmptyStateActions>
        </EmptyState>
      </Card>
    );
  const operation = current.operation;
  const versions = installedVersions(current.installed_versions);
  return (
    <>
      <div className="environment-page">
        <PageHeader>
          <PageHeaderTitle>{current.config.name}</PageHeaderTitle>
          <PageHeaderDescription>
            Ubuntu on Incus. Machine state and T3 health are tracked separately.
          </PageHeaderDescription>
        </PageHeader>
        {demo ? (
          <p className="demo-banner" role="status">
            <strong>Demo mode:</strong> memory-only state transitions. Live API
            actions are disabled.
          </p>
        ) : null}
        {failure ? <Failure failure={failure} /> : null}
        {operation?.error ? (
          <Failure
            failure={operation.error}
            actionLabel={
              operation.status === "failed" ? "Retry operation" : undefined
            }
            onAction={() => void perform("retry")}
          />
        ) : null}
        <div className="environment-actions">
          <StatusBadge tone={tone(current.state)}>
            VM {current.state}
          </StatusBadge>
          <StatusBadge tone={current.service_ready ? "success" : "danger"}>
            T3 {current.service_ready ? "ready" : "unavailable"}
          </StatusBadge>
          <span className="grow" />
          <Button
            size="sm"
            onClick={() => void perform("start")}
            disabled={
              busy || actionBusy(current) || current.state === "running"
            }
          >
            Start
          </Button>
          <Button
            size="sm"
            onClick={() => void perform("stop")}
            disabled={
              busy || actionBusy(current) || current.state !== "running"
            }
          >
            Stop
          </Button>
          <Button
            size="sm"
            onClick={() => void perform("restart")}
            disabled={
              busy || actionBusy(current) || current.state !== "running"
            }
          >
            Restart
          </Button>
          <Button
            size="sm"
            onClick={() => void perform("bootstrap")}
            disabled={busy || actionBusy(current)}
          >
            Bootstrap
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="btn-danger-ghost"
            onClick={() => setConfirming(current)}
            disabled={busy || actionBusy(current)}
          >
            Delete
          </Button>
        </div>
        <Card className="environment-tabs">
          <Tabs key={current.id} defaultValue="configuration">
            <TabsList aria-label="Virtual machine details">
              <TabsTrigger value="configuration">Configuration</TabsTrigger>
              <TabsTrigger value="operation">Operation</TabsTrigger>
              <TabsTrigger value="connect">Connect to console</TabsTrigger>
            </TabsList>
            <TabsContent value="configuration">
            <EnvironmentEditor
              config={config}
              failure={null}
              busy={busy}
              onChange={patch}
              onRecipeChange={patchRecipe}
              onTemplate={chooseTemplate}
              onCancel={() => onOpen(null)}
              onSubmit={(event) => {
                event.preventDefault();
                void saveConfig();
              }}
              onApplyUpdate={() => void perform("apply_update")}
              existing
              dirty={configDirty}
              pendingApply={pendingApply}
            />
            </TabsContent>
            <TabsContent value="operation">
            <p>
              {operation
                ? `${operation.action}: ${operation.step}`
                : "No operation in progress."}
            </p>
            <LogView role="log" aria-label="Environment operation log">
              {(current.log || "No logs yet.")
                .split("\n")
                .map((line, index) => (
                  <LogViewLine key={index}>{line || " "}</LogViewLine>
                ))}
            </LogView>
            <p className="t-caps environment-subheading">Installed versions</p>
            {versions.length ? (
              <ul>
                {versions.map(([key, value]) => (
                  <li key={key}>
                    <code>{key}</code> {value}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="muted">No installed versions recorded yet.</p>
            )}
            </TabsContent>
            <TabsContent value="connect">
            <p>
              <strong>SSH</strong>
            </p>
            <code className="dev-image-code">
              {current.ssh_command || "SSH command is being prepared."}
            </code>
            <div className="row environment-access-actions">
              <Button
                type="button"
                size="sm"
                onClick={() => {
                  if (current.ssh_command)
                    void navigator.clipboard?.writeText(current.ssh_command);
                }}
                disabled={!current.ssh_command}
              >
                Copy SSH command
              </Button>
            </div>
            <p>
              <strong>Web access</strong>
            </p>
            {current.tunnel_command ? (
              <>
                <p className="muted">Run the tunnel before opening T3.</p>
                <code className="dev-image-code">{current.tunnel_command}</code>
                <div className="row environment-access-actions">
                  <Button
                    type="button"
                    size="sm"
                    onClick={() =>
                      void navigator.clipboard?.writeText(
                        current.tunnel_command ?? "",
                      )
                    }
                  >
                    Copy tunnel command
                  </Button>
                </div>
              </>
            ) : null}
            <p>
              {current.web_url ||
                "Available after T3 is ready. Use an SSH tunnel on the Host if this URL is not reachable from your network."}
            </p>
            {current.web_url ? (
              <Button
                type="button"
                size="sm"
                variant="primary"
                onClick={() =>
                  window.open(current.web_url, "_blank", "noopener,noreferrer")
                }
                disabled={!current.service_ready}
              >
                Open T3
              </Button>
            ) : null}
            <p className="muted">
              To pair T3 after SSH access, run{" "}
              <code>t3 pair --base-dir /home/dev/.local/state/t3</code> and
              paste the token into T3. Sign agents in with their own CLI.
            </p>
            <p className="muted">
              Keep the Platform running for this console. SSH remains the
              independent recovery path.
            </p>
            </TabsContent>
          </Tabs>
        </Card>
      </div>
      <AlertDialog
        open={!!confirming}
        onOpenChange={(open) => {
          if (!open) {
            setConfirming(null);
            setConfirmName("");
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete virtual machine</AlertDialogTitle>
          </AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              Deletion removes this virtual machine's disk, projects, credentials,
              and T3 state. Type <code>{confirming?.config.name}</code> to
              continue.
            </AlertDialogDescription>
            <Label htmlFor="confirm-environment-name">Virtual machine name</Label>
            <input
              id="confirm-environment-name"
              className="input"
              value={confirmName}
              onChange={(event) => setConfirmName(event.target.value)}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="btn-danger"
              disabled={confirmName !== confirming?.config.name || busy}
              onClick={() => {
                if (confirming) void perform("delete", confirming, confirmName);
              }}
            >
              Delete virtual machine
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function EnvironmentEditor({
  config,
  failure,
  busy,
  onChange,
  onRecipeChange,
  onTemplate,
  onCancel,
  onSubmit,
  onApplyUpdate,
  existing = false,
  dirty = false,
  pendingApply = false,
}: {
  config: EnvironmentConfig;
  failure: Report | null;
  busy: boolean;
  onChange: (change: Partial<EnvironmentConfig>) => void;
  onRecipeChange: (change: Partial<EnvironmentConfig["recipe"]>) => void;
  onTemplate: (id: string) => void;
  onCancel: () => void;
  onSubmit: (event: FormEvent) => void;
  onApplyUpdate?: () => void;
  existing?: boolean;
  dirty?: boolean;
  pendingApply?: boolean;
}) {
  const valid =
    config.name.trim().length > 0 &&
    config.cpus > 0 &&
    config.memory_gib > 0 &&
    config.disk_gib > 0 &&
    !!config.ssh_public_key.trim() &&
    config.recipe.dependencies.length > 0 &&
    config.recipe.dependencies.every((dependency) =>
      isVersion(dependency.version),
    );
  return (
    <form className="stack environment-editor" onSubmit={onSubmit}>
      <div className="field-row">
        <FormField
          id="environment-name"
          label="Virtual machine name"
          value={config.name}
          onChange={(event) => onChange({ name: event.target.value })}
          required
          disabled={busy}
        />
        <FormField id="environment-template" label="Recipe template">
          <select
            id="environment-template"
            className="input"
            value={config.recipe.template_id ?? ""}
            onChange={(event) => onTemplate(event.target.value)}
            disabled={busy}
          >
            {devImageTemplates.map((template) => (
              <option key={template.id} value={template.id}>
                {template.name}
              </option>
            ))}
          </select>
        </FormField>
      </div>
      <div className="field-row">
        <FormField
          id="environment-cpus"
          label="CPUs"
          type="number"
          min={1}
          max={64}
          value={String(config.cpus)}
          onChange={(event) => onChange({ cpus: Number(event.target.value) })}
          disabled={busy || existing}
          required
        />
        <FormField
          id="environment-memory"
          label="Memory (GiB)"
          type="number"
          min={1}
          max={512}
          value={String(config.memory_gib)}
          onChange={(event) =>
            onChange({ memory_gib: Number(event.target.value) })
          }
          disabled={busy || existing}
          required
        />
        <FormField
          id="environment-disk"
          label="Disk (GiB)"
          type="number"
          min={8}
          max={4096}
          value={String(config.disk_gib)}
          onChange={(event) =>
            onChange({ disk_gib: Number(event.target.value) })
          }
          disabled={busy || existing}
          required
        />
      </div>
      <FormField id="environment-os" label="Base OS">
        <input
          id="environment-os"
          className="input"
          value="Ubuntu LTS"
          readOnly
        />
      </FormField>
      <FormField
        id="environment-ssh-key"
        label="SSH public key"
        hint="Stored in the virtual machine configuration. The private key never leaves your machine."
      >
        <textarea
          id="environment-ssh-key"
          className="input"
          rows={3}
          value={config.ssh_public_key}
          onChange={(event) => onChange({ ssh_public_key: event.target.value })}
          disabled={busy}
          required
        />
      </FormField>
      <Dependencies
        id="environment-dependencies"
        value={config.recipe.dependencies}
        onChange={(dependencies) => onRecipeChange({ dependencies })}
        disabled={busy}
      />
      <FormField
        id="environment-checks"
        label="Build checks"
        hint="One verification command per line."
      >
        <ShellEditor
          id="environment-checks"
          value={config.recipe.build_checks.join("\n")}
          onChange={(value) =>
            onRecipeChange({
              build_checks: value
                .split("\n")
                .map((line) => line.trim())
                .filter(Boolean),
            })
          }
          disabled={busy}
        />
      </FormField>
      <div className="field-row">
        <FormField
          id="environment-command"
          label="T3 service command"
          value={config.command}
          onChange={(event) => onChange({ command: event.target.value })}
          disabled={busy}
          required
        />
        <FormField
          id="environment-port"
          label="T3 web port"
          type="number"
          min={1}
          max={65535}
          value={String(config.web_port)}
          onChange={(event) =>
            onChange({ web_port: Number(event.target.value) })
          }
          disabled={busy}
          required
        />
      </div>
      {failure ? <Failure failure={failure} /> : null}
      <div className="form-actions">
        <Button type="button" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="submit"
          size="sm"
          variant="primary"
          disabled={busy || !valid}
        >
          {existing ? "Save configuration" : "Create virtual machine"}
        </Button>
        {existing && pendingApply ? (
          <Button
            type="button"
            size="sm"
            onClick={onApplyUpdate}
            disabled={busy}
          >
            Apply update
          </Button>
        ) : null}
        {existing ? (
          <span className="muted t-label">
            {dirty
              ? "Pending changes. Save configuration before applying."
              : pendingApply
                ? "Saved changes are ready to apply."
                : "Saved configuration"}
          </span>
        ) : null}
      </div>
    </form>
  );
}
