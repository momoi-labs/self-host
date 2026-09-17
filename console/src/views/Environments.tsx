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
  EmptyStateTitle,
  FormField,
  Label,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@momoi-labs/kiso-react";

import { Dependencies } from "../components/Dependencies.js";
import { Failure } from "../components/Failure.js";
import { Glance } from "../components/Glance.js";
import { Lifecycle } from "../components/Lifecycle.js";
import { ShellEditor } from "../components/ShellEditor.js";
import { Step, Steps } from "../components/Steps.js";
import { LogSurface } from "../components/LogSurface.js";
import { OperationSteps } from "../components/OperationProgress.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Terminal } from "../components/Terminal.js";
import { api, asReport, failureOf } from "../lib/api.js";
import {
  customImageTemplate,
  customImageTemplates,
} from "../lib/customImageTemplates.js";
import { isVersion } from "../lib/dependencies.js";
import { machineSeriesFor } from "../lib/useMetrics.js";
import type {
  Environment,
  EnvironmentConfig,
  Metrics,
  Report,
} from "../lib/types.js";


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
      customImageTemplate("t3-code")?.dependencies ?? [],
    ),
    setup: structuredClone(customImageTemplate("t3-code")?.setup ?? []),
    build_checks: structuredClone(
      customImageTemplate("t3-code")?.buildChecks ?? [],
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
    log: "Virtual machine booted\nSSH is ready\nService is responding",
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
      step: "Checking service readiness",
    },
    log: "Virtual machine booted\nInstalling tools\nChecking service readiness...",
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

/// A machine starts empty. A template is an offer, not a default, so nothing
/// is installed and no service runs until the Operator asks for one.
function freshConfig(): EnvironmentConfig {
  return {
    name: "",
    cpus: 4,
    memory_gib: 8,
    disk_gib: 40,
    ssh_public_key: "",
    recipe: {
      name: "",
      template_id: null,
      dependencies: [],
      setup: [],
      build_checks: [],
    },
    command: "",
    web_port: 0,
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
/**
 * The Platform omits an empty list rather than sending `[]`, so a machine with
 * no build checks arrives without the field at all. Put the empty lists back
 * once, here, so nothing downstream has to ask whether they are there.
 */
function normalizeConfig(value: EnvironmentConfig): EnvironmentConfig {
  return {
    ...value,
    recipe: {
      ...value.recipe,
      dependencies: (value.recipe?.dependencies ?? []).map((dependency) => ({
        ...dependency,
        allow_builds: dependency.allow_builds ?? [],
      })),
      setup: value.recipe?.setup ?? [],
      build_checks: value.recipe?.build_checks ?? [],
    },
  };
}

function normalizeEnvironment(value: Environment): Environment {
  return {
    ...value,
    config: normalizeConfig(value.config),
    applied_config: value.applied_config
      ? normalizeConfig(value.applied_config)
      : value.applied_config,
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
  metrics,
  onOpen,
}: {
  mode: "new" | "detail";
  selected: string | null;
  metrics: Metrics | null;
  onOpen: (id: string | null) => void;
}) {
  const demo =
    new URLSearchParams(location.search).get("demo") === "environments";
  const [records, setRecords] = useState<Environment[]>(
    demo ? demoEnvironments : [],
  );
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
  const [logs, setLogs] = useState("");
  const [tab, setTab] = useState("configuration");
  const [confirmName, setConfirmName] = useState("");
  const requestId = useRef<string | null>(null);
  const opened = useRef<string | null>(null);
  const wasRunning = useRef(false);
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
        if (active) setRecords(next);
      } catch (cause) {
        // The detail screen shows its own failures; a poll that missed one
        // round is not one of them.
        if (active) console.warn("Could not list virtual machines", cause);
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

  /*
   * A machine's own account of itself: the kernel and systemd coming up. It is
   * read out of the guest, so it answers once the guest is far enough along to
   * be asked.
   */
  useEffect(() => {
    if (demo || !selected) {
      setLogs("");
      return;
    }
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const running = current?.operation?.status === "running";
    const path = `/environments/${selected}/logs/boot`;
    const poll = async () => {
      try {
        const response = await api(path);
        if (!active) return;
        // A machine that is not running has no log of its own to give, and
        // saying so beats an empty pane that looks like a stall.
        setLogs(
          response.ok
            ? await response.text()
            : (await failureOf(response)).error,
        );
      } catch {
        if (active) setLogs("");
      } finally {
        if (active && running) timer = setTimeout(poll, 3000);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [demo, selected, current?.operation?.status]);

  /*
   * Where the Operator lands. A machine with something happening to it opens on
   * its log, and a create that starts while they are reading something else
   * takes them there once, the way the image builder's dock opens on a build.
   * Every move after that is theirs to keep.
   */
  useEffect(() => {
    if (!selected) return;
    const running = current?.operation?.status === "running";
    if (opened.current !== selected) {
      opened.current = selected;
      wasRunning.current = running;
      setTab(running ? "logs" : "configuration");
      return;
    }
    if (running && !wasRunning.current) setTab("logs");
    wasRunning.current = running;
  }, [selected, current?.operation?.status]);

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
    const template = customImageTemplate(id);
    if (!template) {
      patchRecipe({
        name: "",
        template_id: null,
        dependencies: [],
        setup: [],
        build_checks: [],
      });
      patch({ command: "", web_port: 0 });
      return;
    }
    patchRecipe({
      name: template.name,
      template_id: id,
      dependencies: structuredClone(template.dependencies),
      setup: structuredClone(template.setup),
      build_checks: structuredClone(template.buildChecks),
    });
    // A template brings its own service. The machine keeps its state under the
    // dev user's home rather than the data volume a container would have.
    patch({
      command: template.application.command.replace(
        "/data/t3home",
        "/home/dev/.local/state/t3",
      ),
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
          web_url: null,
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
                      log: `${item.log}\nSSH is ready\nService is responding`,
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
            <Button onClick={() => onOpen(null)}>Back to the Overview</Button>
          </EmptyStateActions>
        </EmptyState>
      </Card>
    );
  const operation = current.operation;
  const versions = installedVersions(current.installed_versions);
  // A half-provisioned guest reports numbers that read as a machine sitting
  // idle, so the glance waits for the service to answer. What it is using is
  // measured inside it: the Host only sees one hypervisor process.
  const samples = current.service_ready ? machineSeriesFor(metrics, current.id) : null;
  return (
    <>
      <div className="environment-page">
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
        {/* The machine says its own name now, the way every other detail
            screen does. The breadcrumb repeating it is what a breadcrumb is
            for; a screen with no heading was the odd one out. */}
        <div className="between">
          <PageHeader>
            <PageHeaderTitle>{current.config.name}</PageHeaderTitle>
            {/* Where the machine is on the LAN, the way an Application's
                screen shows its Hostnames: the name, linked to the service
                when there is one, and the address its lease gave it. */}
            <PageHeaderDescription>
              {current.web_url ? (
                <a href={current.web_url} target="_blank" rel="noreferrer" className="mono">
                  {current.web_url.replace(/^https?:\/\//, "")}
                </a>
              ) : (
                <span className="mono">{current.hostname || "No name"}</span>
              )}
              {" · "}
              {current.lan_address ? <span className="mono">{current.lan_address}</span> : "No lease"}
            </PageHeaderDescription>
            {samples ? <Glance samples={samples} /> : null}
          </PageHeader>
          {/* Neither badge moves during a create, so while one runs the row
              carries the operation instead: a bar that walks is the only thing
              on this screen that says the minutes are passing. */}
          {operation?.status === "running" ? (
            <div className="lifecycle">
              <OperationSteps operation={operation} />
            </div>
          ) : (
            <Lifecycle
              status={
                <>
                  <StatusBadge tone={tone(current.state)}>
                    VM {current.state}
                  </StatusBadge>
                  {/* A machine nobody signed into has a T3 service that is off,
                      which is the normal state and not an error to report. */}
                  <StatusBadge tone={current.service_ready ? "success" : "neutral"}>
                    Service {current.service_ready ? "ready" : "disabled"}
                  </StatusBadge>
                </>
              }
              actions={
                <>
                  <Button
                    size="sm"
                    onClick={() => void perform("start")}
                    disabled={busy || actionBusy(current) || current.state === "running"}
                  >
                    Start
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => void perform("stop")}
                    disabled={busy || actionBusy(current) || current.state !== "running"}
                  >
                    Stop
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => void perform("restart")}
                    disabled={busy || actionBusy(current) || current.state !== "running"}
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
                </>
              }
              destructive={
                <Button
                  size="sm"
                  variant="ghost"
                  className="btn-danger-ghost"
                  onClick={() => setConfirming(current)}
                  disabled={busy || actionBusy(current)}
                >
                  Delete virtual machine
                </Button>
              }
            />
          )}
        </div>
        <Card className="detail-tabs">
          <Tabs key={current.id} value={tab} onValueChange={setTab}>
            <TabsList aria-label="Virtual machine details">
              <TabsTrigger value="configuration">Configuration</TabsTrigger>
              <TabsTrigger value="logs">Logs</TabsTrigger>
              <TabsTrigger value="connect">Terminal</TabsTrigger>
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
            {versions.length ? (
              <>
                <p className="t-caps environment-subheading">Installed versions</p>
                <ul>
                  {versions.map(([key, value]) => (
                    <li key={key}>
                      <code>{key}</code> {value}
                    </li>
                  ))}
                </ul>
              </>
            ) : null}
            </TabsContent>
            <TabsContent value="logs" className="detail-logs">
            <LogSurface label="Virtual machine log" text={logs}
              placeholder="Nothing recorded yet." />
            </TabsContent>
            <TabsContent value="connect" className="detail-terminal">
            <Terminal id={current.id} machine />
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
              and service state. Type <code>{confirming?.config.name}</code> to
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

export function EnvironmentEditor({
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
  // A machine needs a name and a size. Everything else is an offer: no
  // template, no tools and no service is a plain Ubuntu, which is a thing an
  // Operator may well want.
  const valid =
    config.name.trim().length > 0 &&
    config.cpus > 0 &&
    config.memory_gib > 0 &&
    config.disk_gib > 0 &&
    config.recipe.dependencies.every((dependency) =>
      isVersion(dependency.version),
    );
  return (
    <form className="stack environment-editor" onSubmit={onSubmit}>
      <Steps>
        <Step title="Machine">
          <div className="field-row">
            <FormField
              id="environment-name"
              label="Name"
              value={config.name}
              onChange={(event) => onChange({ name: event.target.value })}
              required
              disabled={busy}
            />
            <FormField id="environment-template" label="Template">
              <select
                id="environment-template"
                className="input"
                value={config.recipe.template_id ?? ""}
                onChange={(event) => onTemplate(event.target.value)}
                disabled={busy}
              >
                <option value="">None</option>
                {customImageTemplates.map((template) => (
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
            <select
              id="environment-os"
              className="input"
              value="ubuntu-lts"
              disabled
            >
              <option value="ubuntu-lts">Ubuntu LTS</option>
            </select>
          </FormField>
        </Step>
        <Step title="Mise packages and dependencies">
          <Dependencies
            id="environment-dependencies"
            value={config.recipe.dependencies}
            onChange={(dependencies) => onRecipeChange({ dependencies })}
            disabled={busy}
          />
        </Step>
        <Step title="Custom commands">
          <FormField
            id="environment-setup"
            label="Custom commands"
            hint="One command per line. Runs as root, with network, after the dependencies. This is where a shell installer or an apt package goes."
          >
            <ShellEditor
              id="environment-setup"
              value={config.recipe.setup.join("\n")}
              onChange={(value) =>
                onRecipeChange({
                  setup: value
                    .split("\n")
                    .map((line) => line.trim())
                    .filter(Boolean),
                })
              }
              disabled={busy}
            />
          </FormField>
        </Step>
        <Step title="Build checks">
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
        </Step>
        <Step title="Service">
          <div className="field-row">
            <FormField
              id="environment-command"
              label="Service command"
              value={config.command}
              onChange={(event) => onChange({ command: event.target.value })}
              disabled={busy}
            />
            <FormField
              id="environment-port"
              label="Web port"
              type="number"
              min={1}
              max={65535}
              value={config.web_port ? String(config.web_port) : ""}
              placeholder="None"
              onChange={(event) =>
                onChange({ web_port: Number(event.target.value) || 0 })
              }
              disabled={busy}
            />
          </div>
        </Step>
      </Steps>
      {/* The console opens its own terminal, so a key is only for reaching
          the machine from somewhere else. Out of the way, not out of reach. */}
      <details className="advanced">
        <summary>Advanced</summary>
        <FormField
          id="environment-ssh-key"
          label="SSH public key"
          hint="Optional. Lets you reach the machine with your own ssh client. The private key never leaves your computer."
        >
          <textarea
            id="environment-ssh-key"
            className="input"
            rows={3}
            value={config.ssh_public_key}
            onChange={(event) =>
              onChange({ ssh_public_key: event.target.value })
            }
            disabled={busy}
          />
        </FormField>
      </details>
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
