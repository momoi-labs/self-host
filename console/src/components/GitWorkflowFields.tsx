import { useState } from "react";
import {
  Button,
  Checkbox,
  FormField,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  StatusBadge,
} from "@momoi-labs/kiso-react";

import type { GitInspection } from "../lib/types.js";
import { GitConnectionSelector } from "./GitConnections.js";
import { GitRepositoryPicker, type GitRepository } from "./GitRepositoryPicker.js";
import { GitSourceFields, type GitFields } from "./GitSourceFields.js";
import "./git-workflow.css";

export type GitSourceMode = "repositories" | "url";

export function GitWorkflowFields({
  fields, onChange, creating, mode, onModeChange, setup, onEditSource,
  inspection, reviewed, checking, disabled, onImport, recipeChosen, onRecipeChange,
  name, onNameChange, port, onPortChange, webService, onWebServiceChange,
  unpublished, onUnpublishedChange, dnsSuffix,
}: {
  fields: GitFields;
  onChange: (fields: GitFields) => void;
  creating: boolean;
  mode: GitSourceMode;
  onModeChange: (mode: GitSourceMode) => void;
  setup: boolean;
  onEditSource: () => void;
  inspection: GitInspection | null;
  reviewed: boolean;
  checking: boolean;
  disabled: boolean;
  onImport: (repository: GitRepository) => void;
  recipeChosen: boolean;
  onRecipeChange: (kind: "dockerfile" | "compose", path: string) => void;
  name: string;
  onNameChange: (name: string) => void;
  port: string;
  onPortChange: (port: string) => void;
  webService: string;
  onWebServiceChange: (service: string) => void;
  unpublished: boolean;
  onUnpublishedChange: (unpublished: boolean) => void;
  dnsSuffix: string;
}) {
  const [showPort, setShowPort] = useState(false);
  const [showBuild, setShowBuild] = useState(false);
  const repositoryName = fields.repository.replace(/\.git\/?$/, "").split("/").slice(-2).join("/");
  const buildFile = fields.composePath.trim() || fields.dockerfile.trim() || "Dockerfile";
  const buildValue = recipeChosen ? `${fields.composePath.trim() ? "compose" : "dockerfile"}:${buildFile}` : "";
  const services = reviewed ? inspection?.services ?? [] : [];
  const chosenService = fields.composePath.trim() ? services.find((service) => service.name === webService) : undefined;
  const availablePorts = chosenService?.ports ?? (reviewed ? inspection?.ports ?? [] : []);
  const needsPort = !unpublished && (showPort || !reviewed || availablePorts.length !== 1 || port !== String(availablePorts[0]));

  const connection = <GitConnectionSelector value={fields.credentialId} disabled={disabled || checking} onChange={(credentialId) => onChange({ ...fields, credentialId })} />;
  const advanced = <details className="disclosure git-build-settings" open={showBuild} onToggle={(event) => setShowBuild(event.currentTarget.open)}>
    <summary>Advanced build settings</summary>
    <div className="git-settings-fields"><GitSourceFields fields={fields} onChange={onChange} includeRepository={!creating} /></div>
  </details>;

  if (!creating) return <div className="git-workflow">
    <dl className="git-detected-plan"><div><dt>Repository</dt><dd>{repositoryName}</dd></div><div><dt>Branch or tag</dt><dd>{fields.gitRef || "HEAD"}</dd></div><div><dt>Build file</dt><dd>{buildFile}</dd></div></dl>
    {connection}
    {advanced}
    <p className="muted t-label">Changing the repository, branch, tag or pinned commit builds that selection when you save.</p>
    {fields.composePath.trim() && !unpublished ? <FormField id="f-git-web-service" label="Web service" value={webService} onChange={(event) => onWebServiceChange(event.target.value)} hint="Leave empty to use the first service that declares a port." /> : null}
    {!unpublished ? <FormField id="f-git-web-port" label="Application port" type="number" min={1} max={65535} value={port} onChange={(event) => onPortChange(event.target.value)} hint="The port the Application listens on inside its container." /> : null}
  </div>;

  if (!setup) return <div className="git-workflow">
    {connection}
    {mode === "repositories" ? <>
      <GitRepositoryPicker credentialId={fields.credentialId} disabled={disabled || checking} onImport={onImport} />
      <Button type="button" size="sm" variant="ghost" disabled={disabled || checking} onClick={() => onModeChange("url")}>Use a repository URL instead</Button>
    </> : <>
      <FormField id="f-git-repository" label="Repository URL" type="url" required disabled={disabled || checking} value={fields.repository} placeholder="https://github.com/team/app.git" onChange={(event) => onChange({ ...fields, repository: event.target.value })} />
      <Button type="button" size="sm" variant="ghost" disabled={disabled || checking} onClick={() => onModeChange("repositories")}>Choose from connected repositories</Button>
    </>}
    {checking ? <div role="status"><StatusBadge tone="success" pulse>Checking repository</StatusBadge></div> : null}
  </div>;

  return <div className="git-workflow">
    <div className="git-selected-repository"><div><p className="t-caps">Repository selected</p><h2>{repositoryName}</h2></div><Button type="button" size="sm" variant="ghost" disabled={disabled || checking} onClick={onEditSource}>Edit source</Button></div>
    {reviewed && inspection ? <dl className="git-detected-plan">
      <div><dt>Branch or tag</dt><dd>{inspection.git_ref}</dd></div>
      <div><dt>Commit</dt><dd><code title={inspection.revision}>{inspection.revision.slice(0, 12)}</code></dd></div>
      <div><dt>Build file</dt><dd>{recipeChosen ? buildFile : "Choose a build file"}</dd></div>
      <div><dt>Web access</dt><dd>{unpublished ? "No web address" : port ? <>Port {port}<Button type="button" size="sm" variant="ghost" disabled={disabled || checking} onClick={() => setShowPort(true)}>Change</Button></> : "Application port needed"}</dd></div>
    </dl> : <p className="muted" role="status">{checking ? "Checking repository..." : "The source changed. Check the repository again before building."}</p>}
    <FormField id="f-name" label="Application name" required disabled={disabled || checking} value={name} placeholder="my-app" onChange={(event) => onNameChange(event.target.value)} />
    {inspection && inspection.build_files.length > 1 ? <FormField id="f-git-build-file" label="Which build file should we use?">
      <Select value={buildValue} disabled={disabled || checking} onValueChange={(value) => {
        const file = inspection.build_files.find((file) => `${file.kind}:${file.path}` === value);
        if (file) onRecipeChange(file.kind, file.path);
      }}>
        <SelectTrigger id="f-git-build-file"><SelectValue placeholder="Choose a build file" /></SelectTrigger>
        <SelectContent>{inspection.build_files.map((file) => <SelectItem key={`${file.kind}:${file.path}`} value={`${file.kind}:${file.path}`}>{file.path}</SelectItem>)}</SelectContent>
      </Select>
    </FormField> : null}
    {inspection && !inspection.build_files.length ? <p className="muted">No build file was found. Set its path in Advanced build settings and check again.</p> : null}
    {services.length > 1 && fields.composePath.trim() && !unpublished ? <FormField id="f-git-web-service" label="Web service">
      <Select value={webService} disabled={disabled || checking} onValueChange={(value) => {
        if (services.some((service) => service.name === value)) onWebServiceChange(value);
      }}><SelectTrigger id="f-git-web-service"><SelectValue placeholder="Choose a service" /></SelectTrigger><SelectContent>{services.map((service) => <SelectItem key={service.name} value={service.name}>{service.name}</SelectItem>)}</SelectContent></Select>
    </FormField> : null}
    {needsPort ? availablePorts.length > 1 && !showPort ? <FormField id="f-git-web-port" label="Application port">
      <Select value={port} disabled={disabled || checking} onValueChange={(value) => {
        if (value === "other") { setShowPort(true); onPortChange(""); }
        else if (availablePorts.some((item) => String(item) === value)) onPortChange(value);
      }}><SelectTrigger id="f-git-web-port"><SelectValue placeholder="Choose a port" /></SelectTrigger><SelectContent>{availablePorts.map((value) => <SelectItem key={value} value={String(value)}>{value}</SelectItem>)}<SelectItem value="other">Other...</SelectItem></SelectContent></Select>
    </FormField> : <FormField id="f-git-web-port" label="Application port" type="number" min={1} max={65535} required={reviewed && recipeChosen} disabled={disabled || checking} value={port} onChange={(event) => onPortChange(event.target.value)} hint="The port the Application listens on inside its container." /> : null}
    <div className="check"><Checkbox id="f-git-unpublished" checked={unpublished} disabled={disabled || checking} onCheckedChange={(value) => onUnpublishedChange(value === true)} /><Label htmlFor="f-git-unpublished">Run without a web address</Label></div>
    {!unpublished && name.trim() ? <p className="muted git-application-address">{name.trim()}.{dnsSuffix}</p> : null}
    {advanced}
  </div>;
}
