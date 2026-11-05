import { useState, type FormEvent, type ReactNode } from "react";
import { Button, Form, FormActions, FormField, KV, KVKey, KVValue, RadioGroup, RadioGroupItem, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@momoi-labs/kiso-react";
import { parseAliases } from "../lib/status.js";
import { nativeCommand, nativeCommandFields } from "../lib/nativeCommand.js";
import { nativeRecipe, nativeRecipeFields } from "../lib/nativeRecipe.js";
import { nativeGitBuild, nativeGitFields, nativeGitSource, workerDependencies } from "../lib/nativeGit.js";
import { GitConnectionSelector } from "./GitConnections.js";
import { GitRepositoryPicker } from "./GitRepositoryPicker.js";
import { GitSourceFields, gitFieldsOf } from "./GitSourceFields.js";
import "./git-workflow.css";
import { isVersion } from "../lib/dependencies.js";
import type { App, Report } from "../lib/types.js";
import type { Submission } from "./AppForm.js";
import { Failure } from "./Failure.js";
import { Dependencies } from "./Dependencies.js";
import { ShellEditor } from "./ShellEditor.js";
import { Step, Steps } from "./Steps.js";
import { Icon } from "./Icon.js";
import { NativeVariables } from "./NativeVariables.js";

import { useNativeCapabilities } from "../lib/useNativeCapabilities.js";

function fieldsOf(app?: App) {
  const native = app?.runtime?.kind === "native" ? app.runtime : null;
  return {
    name: app?.name ?? "", ...nativeCommandFields(native?.command),
    ...nativeRecipeFields(native?.recipe),
    ...nativeGitFields(native),
    source: app?.git ? "git" : "home",
    git: gitFieldsOf(app?.git),
    workingDir: native?.working_dir ?? "",
    cpu: native?.limits?.cpu_percent?.toString() ?? "",
    memory: native?.limits?.memory_bytes ? String(native.limits.memory_bytes / 1048576) : "",
    tasks: native?.limits?.max_tasks?.toString() ?? "",
    web: app ? app.publication?.kind !== "unpublished" : true,
    port: native?.port?.toString() ?? "", hostname: app?.hostname ?? "",
    aliases: (app?.aliases ?? []).join(", "),
  };
}

export function NativeAppForm({ app, onSubmit, onCancel, onReload, definition }: {
  app?: App; dnsSuffix: string;
  onSubmit: (body: Submission, source: string) => Promise<Report | null>;
  onCancel?: () => void;
  /** The definition choice "New application" opens with, above the steps. */
  definition?: ReactNode;
  onReload?: () => Promise<App[]>;
}) {
  const { capabilities, error: capabilitiesError } = useNativeCapabilities();
  const saved = fieldsOf(app);
  const [fields, setFields] = useState(saved);
  const [variables, setVariables] = useState<{ key: string; value: string }[]>([]);
  const [failure, setFailure] = useState<Report | null>(null);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [editingVariable, setEditingVariable] = useState(false);
  const [gitMode, setGitMode] = useState<"repositories" | "url">(app ? "url" : "repositories");
  const gitSource = fields.source === "git";
  const dirty = JSON.stringify(fields) !== JSON.stringify(saved);
  const validDependencies = fields.dependencies.every((dependency) => isVersion(dependency.version));
  const native = app?.runtime?.kind === "native" ? app.runtime : null;
  function change<K extends keyof typeof fields>(key: K, value: typeof fields[K]) { setFields({ ...fields, [key]: value }); }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (saving || editingVariable || !validDependencies || !capabilities?.available) return;
    if (fields.worker && (!gitSource || !fields.web || !fields.port || !fields.workerConfig.trim())) return;
    setFailure(null);
    setCommandError(null);
    let command: string[];
    try {
      command = nativeCommand(fields);
    } catch (error) {
      setCommandError((error as Error).message);
      return;
    }
    const keys = variables.map(({ key }) => key.trim());
    if (new Set(keys).size !== keys.length || keys.some((key) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key))) {
      setFailure({ error: "Use a unique valid name for each Application Variable.", caused_by: [] });
      return;
    }
    setSaving(true);
    try {
      const report = await onSubmit({
        name: fields.name, aliases: fields.web ? parseAliases(fields.aliases) : [],
        ...(fields.web && (fields.hostname.trim() || app) ? { hostname: fields.hostname.trim() } : {}),
        publication: { kind: fields.web ? "web" : "unpublished" },
        ...(gitSource ? { git: nativeGitSource(fields.git) } : {}),
        runtime: {
          kind: "native", account: native?.account ?? "",
          command,
          recipe: nativeRecipe(fields),
          ...(gitSource ? nativeGitBuild(fields) : {}),
          working_dir: fields.workingDir.trim() || null,
          port: fields.web ? Number(fields.port) : null,
          limits: capabilities?.resource_limits ? {
            ...(fields.cpu ? { cpu_percent: Number(fields.cpu) } : {}),
            ...(fields.memory ? { memory_bytes: Math.round(Number(fields.memory) * 1048576) } : {}),
            ...(fields.tasks ? { max_tasks: Number(fields.tasks) } : {}),
          } : {},
        },
        ...(!app ? { environment: Object.fromEntries(variables.map(({ key, value }) => [key.trim(), value])) } : {}),
      }, gitSource ? "git" : "native");
      if (report) setFailure(report);
      else setVariables([]);
    } catch {
      setFailure({ error: "Could not save the native Application.", caused_by: [] });
    } finally { setSaving(false); }
  }

  return <Form id="app-form" onSubmit={save}>
    <div className="form-body">
      {definition}
      <Steps>
        <Step title="Application">
          <FormField id="native-name" label="Name" value={fields.name} onChange={(event) => change("name", event.target.value)} required autoFocus={!app} disabled={saving} />
          {app ? <KV><KVKey>Source</KVKey><KVValue>{gitSource ? "Git repository" : "Application home"}</KVValue></KV> : <RadioGroup label="Source" variant="tiles" value={fields.source} disabled={saving} onValueChange={(source) => setFields({ ...fields, source, ...(source === "home" ? { worker: false } : {}) })}>
            <RadioGroupItem value="home" label="Application home" description="Run a program installed by the Application Account recipe." />
            <RadioGroupItem value="git" label="Git repository" description="Build a commit and run it as a native release." />
          </RadioGroup>}
        </Step>
        {gitSource ? <Step title="Repository">
          <GitConnectionSelector value={fields.git.credentialId} disabled={saving} onChange={(credentialId) => change("git", { ...fields.git, credentialId })} />
          {!app && gitMode === "repositories" ? <>
            <GitRepositoryPicker credentialId={fields.git.credentialId} disabled={saving} onImport={(repository) => {
              setFields({ ...fields, name: fields.name || repository.full_name.split("/").pop() || "", git: { ...fields.git, repository: repository.clone_url, gitRef: repository.default_branch || "HEAD", revision: "" } });
              setGitMode("url");
            }} />
            <Button type="button" size="sm" variant="ghost" disabled={saving} onClick={() => setGitMode("url")}>Use a repository URL instead</Button>
          </> : !app ? <Button type="button" size="sm" variant="ghost" disabled={saving} onClick={() => setGitMode("repositories")}>Choose from connected repositories</Button> : null}
          <GitSourceFields fields={fields.git} onChange={(git) => change("git", git)} section="repository" includeRepository={!!app || gitMode === "url"} disabled={saving} />
          <FormField id="native-git-context" label="Repository context" value={fields.git.context} disabled={saving} onChange={(event) => change("git", { ...fields.git, context: event.target.value })} placeholder="." hint="Directory inside the checkout used for build and process commands." />
        </Step> : null}
        <Step title="Dependencies">
          <Dependencies id="native-dependencies" value={fields.dependencies} onChange={(dependencies) => change("dependencies", dependencies)} disabled={saving} hint="Enter to add a tool. Click its version to edit." />
        </Step>
        <Step title="Setup">
          <FormField id="native-setup" label="Custom commands" hint="One command per line, run as the app user.">
            <ShellEditor id="native-setup" value={fields.setup} onChange={(setup) => change("setup", setup)} disabled={saving} />
          </FormField>
        </Step>
        {gitSource ? <Step title="Build">
          <FormField id="native-build" label="Build commands" hint="One command per line, run for each Git candidate. Setup above runs for the Application Account.">
            <ShellEditor id="native-build" value={fields.build} onChange={(build) => change("build", build)} disabled={saving} placeholder={"npm ci\nnpm run build"} />
          </FormField>
        </Step> : null}
        <Step title="Process">
          <RadioGroup label="Run" variant="tiles" value={fields.worker ? "worker" : "command"} disabled={saving} description={!gitSource ? "Workers and static sites require a Git repository source." : undefined} onValueChange={(value) => {
            const worker = value === "worker";
            setFields({ ...fields, worker, ...(worker ? { web: true, dependencies: workerDependencies(fields.dependencies) } : {}) });
            setCommandError(null);
          }}>
            <RadioGroupItem value="command" label="Start command" description="Run a foreground process." />
            <RadioGroupItem value="worker" label="Worker or static site" description="Build with Wrangler and run locally with workerd." disabled={!gitSource || (!!app && !fields.web)} />
          </RadioGroup>
          {fields.worker ? <>
            <FormField id="native-worker-config" label="Wrangler config path" value={fields.workerConfig} onChange={(event) => change("workerConfig", event.target.value)} required disabled={saving} placeholder="wrangler.jsonc" hint="Relative to the repository context and working directory." />
            <p className="muted">A module Worker (main in the config) or a static site (assets in the config). No Cloudflare storage bindings. The result runs locally with workerd, without deploying to Cloudflare.</p>
          </> : <FormField id="native-command" label="Start command" error={commandError} hint="Run in the foreground. Use sh -c for shell syntax.">
            <ShellEditor id="native-command" required placeholder="node server.js --port 3000" value={fields.command} disabled={saving} onChange={(command) => { change("command", command); setCommandError(null); }} />
          </FormField>}
          <FormField id="native-directory" label="Working directory" value={fields.workingDir} onChange={(event) => change("workingDir", event.target.value)} disabled={saving} placeholder={gitSource ? "Repository context" : "Application home"} hint={gitSource ? "Relative to the repository context." : "Relative to the app home. Setup can create it."} />
        </Step>
        {capabilities?.resource_limits ? <Step title="Resource limits">
          <div className="field-row">
            <FormField id="native-cpu" label="CPU (%)" type="number" min={1} step={1} value={fields.cpu} onChange={(event) => change("cpu", event.target.value)} disabled={saving} placeholder="Unlimited" />
            <FormField id="native-memory" label="Memory (MiB)" type="number" min={1} step={1} value={fields.memory} onChange={(event) => change("memory", event.target.value)} disabled={saving} placeholder="Unlimited" />
          </div>
          <FormField id="native-tasks" label="Processes and threads" type="number" min={1} step={1} value={fields.tasks} onChange={(event) => change("tasks", event.target.value)} disabled={saving} placeholder="Unlimited" />
        </Step>
        : null}
        <Step title="Publication">
          {app ? <KV><KVKey>Access</KVKey><KVValue>{fields.web ? "Hostname" : "No hostname"}</KVValue></KV> : (
            <Select value={fields.web ? "web" : "unpublished"} onValueChange={(value) => change("web", value === "web")} disabled={saving || fields.worker}>
              <FormField id="native-publication" label="Access" hint={fields.worker ? "Workers and static sites require Web publication on 127.0.0.1." : undefined}>
                <SelectTrigger><SelectValue /></SelectTrigger>
              </FormField>
              <SelectContent><SelectItem value="web">Hostname</SelectItem><SelectItem value="unpublished">No hostname</SelectItem></SelectContent>
            </Select>
          )}
          {fields.web ? <>
            <FormField id="native-hostname" label="Hostname" value={fields.hostname} onChange={(event) => change("hostname", event.target.value)} placeholder="Automatic" required={!!app} disabled={saving} />
            <FormField id="native-port" label="Port on 127.0.0.1" type="number" min={1024} max={65535} step={1} value={fields.port} onChange={(event) => change("port", event.target.value)} required disabled={saving} />
            <FormField id="native-aliases" label="Hostname aliases" value={fields.aliases} onChange={(event) => change("aliases", event.target.value)} disabled={saving} hint="Separate with commas." />
          </> : null}
        </Step>
        <Step title="Variables">
          {app ? <NativeVariables app={app} reload={onReload} disabled={saving} onEditingChange={setEditingVariable} /> : <>
          {variables.map((variable, index) => <div className="row" key={index}>
            <div className="field-row grow">
              <FormField id={`native-key-${index}`} label="Name" value={variable.key} autoComplete="off" required disabled={saving} onChange={(event) => setVariables(variables.map((item, at) => at === index ? { ...item, key: event.target.value } : item))} />
              <FormField id={`native-value-${index}`} label="Value" type="password" autoComplete="new-password" value={variable.value} disabled={saving} onChange={(event) => setVariables(variables.map((item, at) => at === index ? { ...item, value: event.target.value } : item))} />
            </div>
            <Button type="button" size="sm" variant="ghost" aria-label={`Remove variable ${index + 1}`} disabled={saving} onClick={() => setVariables(variables.filter((_, at) => at !== index))}><Icon name="x" /></Button>
          </div>)}
          <div className="row"><Button type="button" size="sm" disabled={saving} onClick={() => setVariables([...variables, { key: "", value: "" }])}><Icon name="plus" />Add variable</Button></div>
          </>}
        </Step>
      </Steps>
      {capabilitiesError ? <Failure failure={{ error: "Could not read native Host capabilities.", caused_by: [] }} /> : null}
      {capabilities?.reason ? <p role="status">{capabilities.reason}</p> : null}
      {failure ? <Failure failure={failure} /> : null}
    </div>
    <FormActions sticky tone={failure || commandError ? "danger" : dirty ? "warning" : "neutral"} message={app ? dirty ? gitSource ? "Unsaved changes. Saving builds the selected Git source." : "Unsaved changes." : gitSource ? "Saved. Use Update to review newer commits, or save to rebuild." : "Saved." : undefined}>
      {onCancel ? <Button type="button" size="sm" onClick={onCancel}>Cancel</Button> : null}
      {app && dirty ? <Button type="button" size="sm" disabled={saving} onClick={() => { setFields(saved); setFailure(null); setCommandError(null); }}>Discard</Button> : null}
      <Button type="submit" size="sm" variant="primary" disabled={!capabilities?.available || saving || editingVariable || !validDependencies || !fields.name.trim() || (!fields.worker && !fields.command.trim()) || (fields.worker && (!gitSource || !fields.web || !fields.workerConfig.trim())) || (gitSource && !fields.git.repository.trim()) || (fields.web && !fields.port)}>{saving ? "Saving..." : app ? gitSource ? "Save and build" : "Save" : "Deploy"}</Button>
    </FormActions>
  </Form>;
}
