import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import {
  Button,
  Checkbox,
  Form,
  FormActions,
  FormField,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf, getJson } from "../lib/api.js";
import { isCompose, parseAliases } from "../lib/status.js";
import type { App, ComposeService, GitInspection, GitSource, Inspection, Publication, Report, Settings } from "../lib/types.js";
import { gitFieldsOf, gitSourceOf, type GitFields } from "./GitSourceFields.js";
import { GitWorkflowFields, type GitSourceMode } from "./GitWorkflowFields.js";
import type { GitRepository } from "./GitRepositoryPicker.js";
import { NativeAppForm } from "./NativeAppForm.js";
import { ComposeEditor } from "./ComposeEditor.js";
import { Failure } from "./Failure.js";
import { customImageTemplate } from "../lib/customImageTemplates.js";

export type Submission = {
  name: string;
  aliases: string[];
  hostname?: string;
  compose?: string;
  web_service?: string;
  web_port?: number;
  image?: string;
  git?: GitSource;
  refresh_source?: boolean;
  source_revision?: string;
  publication?: Publication;
  runtime?: import("../lib/types.js").Runtime;
  environment?: Record<string, string>;
  development?: {
    image_id: string;
    tag: string;
    command: string;
    web_port: number;
    persist_data: boolean;
  };
  /** Pull newer images before redeploying. Only an edit sends it. */
  pull?: boolean;
};

type CustomImage = { id: string; name: string; image: string; status: string; template_id?: string | null };

type Errors = {
  hostname?: string;
  aliases?: string;
  compose?: string;
};

/**
 * The fields as the Application has them, so the form can tell an edit from
 * the record it started with. Editing is against this; creating has nothing
 * to compare to.
 */
function fieldsOf(app?: App) {
  return {
    name: app?.name ?? "",
    image: app?.image ?? "",
    customImageId: app?.development?.image_id ?? "",
    customImageTag: app?.development?.tag ?? "",
    startCommand: app?.development?.command ?? "",
    devPort: app?.development ? String(app.development.web_port) : "",
    persistData: app?.development?.persist_data ?? false,
    compose: app?.compose ?? "",
    hostname: app?.hostname ?? "",
    aliases: (app?.aliases ?? []).join(", "),
    webService: app?.web_service ?? "",
    port: app?.web_port ? String(app.web_port) : "",
    unpublished: app?.publication?.kind === "unpublished",
    git: gitFieldsOf(app?.git),
  };
}
type Fields = ReturnType<typeof fieldsOf>;

function gitKey(fields: GitFields) {
  return JSON.stringify(fields);
}

/** Normalize technical fields while preserving the display name. */
function same(a: Fields, b: Fields) {
  const norm = (f: Fields) => JSON.stringify({ ...f, aliases: parseAliases(f.aliases), hostname: f.hostname.trim(), image: f.image.trim(), startCommand: f.startCommand.trim(), port: f.port.trim() });
  return norm(a) === norm(b);
}

/**
 * The same form serves "New application" and the detail page. Creating offers
 * the choice between an image and a Compose file; editing keeps the definition
 * the Application already has.
 */
export function AppForm({
  app,
  dnsSuffix,
  onSubmit,
  onCancel,
  onReload,
}: {
  app?: App;
  dnsSuffix: string;
  onSubmit: (body: Submission, source: string) => Promise<Report | null>;
  onCancel?: () => void;
  onReload?: () => Promise<App[]>;
}) {
  const creating = !app;
  const saved = fieldsOf(app);
  const [source, setSource] = useState(creating ? "image" : app?.runtime?.kind === "native" ? "native" : app?.git ? "git" : app?.development ? "custom-image" : isCompose(app) ? "compose" : "image");
  const [git, setGit] = useState(saved.git);
  const [gitMode, setGitMode] = useState<GitSourceMode>("repositories");
  const [gitSetup, setGitSetup] = useState(false);
  const [gitInspection, setGitInspection] = useState<GitInspection | null>(null);
  const [gitReviewKey, setGitReviewKey] = useState("");
  const [checkingGit, setCheckingGit] = useState(false);
  const [recipeChosen, setRecipeChosen] = useState(!creating);
  const gitRequest = useRef(0);
  const gitDraft = useRef(git);
  gitDraft.current = git;
  const gitReviewed = gitInspection !== null && gitReviewKey === gitKey(git);
  const [name, setName] = useState(saved.name);
  const [image, setImage] = useState(saved.image);
  const [customImageId, setCustomImageId] = useState(saved.customImageId);
  const [customImageTag, setCustomImageTag] = useState(saved.customImageTag);
  const [startCommand, setStartCommand] = useState(saved.startCommand);
  const [devPort, setDevPort] = useState(saved.devPort);
  const [persistData, setPersistData] = useState(saved.persistData);
  const runtimeEdited = useRef(false);
  const [customImages, setCustomImages] = useState<CustomImage[]>([]);
  const [customImagesLoading, setCustomImagesLoading] = useState(false);
  const [customImagesFailure, setCustomImagesFailure] = useState<Report | null>(null);
  const [compose, setCompose] = useState(saved.compose);
  const [hostname, setHostname] = useState(saved.hostname);
  const [aliases, setAliases] = useState(saved.aliases);
  const [webService, setWebService] = useState(saved.webService);
  const [port, setPort] = useState(saved.port);
  const [unpublished, setUnpublished] = useState(saved.unpublished);
  const [gitDomains, setGitDomains] = useState(false);
  const portEdited = useRef(false);
  // Starts where the Platform setting is; this redeploy can go the other way.
  const [pull, setPull] = useState(false);
  useEffect(() => {
    if (creating) return;
    void getJson<Settings>("/settings").then((settings) => setPull(settings?.pullNewerImages.effective ?? false));
  }, [creating]);
  const fields: Fields = { name, image, customImageId, customImageTag, startCommand, devPort, persistData, compose, hostname, aliases, webService, port, unpublished, git };
  const dirty = !creating && !same(fields, saved);
  const changedGitSelection = source === "git" && (git.repository.trim() !== saved.git.repository.trim() || git.gitRef.trim() !== saved.git.gitRef.trim() || git.revision.trim() !== saved.git.revision.trim());

  /** Back to the record, field by field, the way Discard is read. */
  function reset() {
    setName(saved.name);
    setImage(saved.image);
    setCustomImageId(saved.customImageId);
    setCustomImageTag(saved.customImageTag);
    setStartCommand(saved.startCommand);
    setDevPort(saved.devPort);
    setPersistData(saved.persistData);
    setCompose(saved.compose);
    setHostname(saved.hostname);
    setAliases(saved.aliases);
    setWebService(saved.webService);
    setPort(saved.port);
    setUnpublished(saved.unpublished);
    setGit(saved.git);
    gitDraft.current = saved.git;
    ++gitRequest.current;
    setCheckingGit(false);
    setGitSetup(false);
    setGitInspection(null);
    setGitReviewKey("");
    setRecipeChosen(!creating);
    portEdited.current = false;
    setErrors({});
  }
  const [other, setOther] = useState(false);
  const [inspected, setInspected] = useState<Inspection | null>(null);
  const [errors, setErrors] = useState<Errors>({});
  const [failure, setFailure] = useState<Report | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const hostnameField = useRef<HTMLInputElement>(null);
  const aliasesField = useRef<HTMLInputElement>(null);

  useEffect(() => () => { ++gitRequest.current; }, []);

  function changeGit(next: GitFields) {
    if (gitKey(next) === gitKey(gitDraft.current)) return;
    const old = gitDraft.current;
    gitDraft.current = next;
    ++gitRequest.current;
    setGit(next);
    setGitReviewKey("");
    setCheckingGit(false);
    if (next.repository !== old.repository || next.gitRef !== old.gitRef || next.credentialId !== old.credentialId) {
      if (next.repository !== old.repository || next.credentialId !== old.credentialId) setGitSetup(false);
      setGitInspection(null);
      if (next.repository !== old.repository) setRecipeChosen(!creating);
    }
  }

  async function inspectGit(candidate = gitDraft.current) {
    if (checkingGit || submitting) return;
    const generation = ++gitRequest.current;
    gitDraft.current = candidate;
    setGit(candidate);
    setGitReviewKey("");
    setCheckingGit(true);
    setFailure(null);
    try {
      let selected = candidate;
      let result: GitInspection | null = null;
      // A discovered Compose file needs a second check using that recipe so
      // its ports describe what will actually be built.
      for (let attempt = 0; attempt < 2; ++attempt) {
        const response = await api("/source/inspect", { method: "POST", body: JSON.stringify({ git: gitSourceOf(selected) }) });
        if (!response.ok) throw await failureOf(response);
        result = await response.json() as GitInspection;
        if (gitRequest.current !== generation) return;
        if (result.build_files.length !== 1) break;
        const file = result.build_files[0];
        const detected = file.kind === "compose"
          ? { ...selected, composePath: file.path }
          : { ...selected, composePath: "", dockerfile: file.path };
        if (gitKey(detected) === gitKey(selected)) break;
        if (attempt === 1) throw new Error("The repository changed while checking. Check it again.");
        selected = detected;
        gitDraft.current = selected;
        setGit(selected);
      }
      if (!result || gitRequest.current !== generation) return;
      gitDraft.current = selected;
      setGit(selected);
      setGitInspection(result);
      setGitReviewKey(gitKey(selected));
      setGitSetup(true);
      if (result.build_files.length === 1) setRecipeChosen(true);
      if (!name.trim()) {
        const repositoryName = selected.repository.replace(/\.git\/?$/, "").split("/").pop() ?? "";
        setName(repositoryName);
      }
      const service = selected.composePath.trim()
        ? result.services.find((service) => service.name === webService) ?? result.services.find((service) => service.ports.length) ?? result.services[0]
        : undefined;
      setWebService(service?.name ?? "");
      const candidates = service?.ports ?? result.ports;
      if (!portEdited.current) setPort(candidates.length === 1 ? String(candidates[0]) : "");
    } catch (cause) {
      if (gitRequest.current === generation) setFailure(asReport(cause));
    } finally {
      if (gitRequest.current === generation) setCheckingGit(false);
    }
  }

  function importRepository(repository: GitRepository) {
    const candidate = { ...git, repository: repository.clone_url, gitRef: repository.default_branch || "HEAD", revision: "", composePath: "", dockerfile: "Dockerfile" };
    setGitInspection(null);
    setRecipeChosen(false);
    portEdited.current = false;
    void inspectGit(candidate);
  }

  function chooseGitRecipe(kind: "dockerfile" | "compose", path: string) {
    changeGit(kind === "compose" ? { ...git, composePath: path } : { ...git, composePath: "", dockerfile: path });
    setGitReviewKey("");
    setRecipeChosen(true);
    portEdited.current = false;
    setPort("");
    setWebService("");
  }

  function chooseGitService(service: string) {
    setWebService(service);
    if (!creating) return;
    const candidates = gitInspection?.services.find((item) => item.name === service)?.ports ?? [];
    portEdited.current = false;
    setPort(candidates.length === 1 ? String(candidates[0]) : "");
  }

  const services = inspected?.services ?? [];
  const chosen: ComposeService | undefined = services.find((s) => s.name === webService);
  const ports = chosen ? [...new Set(chosen.ports.map((p) => String(p.container)))] : [];

  useEffect(() => {
    if (source !== "custom-image") return;
    const controller = new AbortController();
    setCustomImagesLoading(true);
    setCustomImagesFailure(null);
    void (async () => {
      try {
        const response = await api("/custom-images", { signal: controller.signal });
        if (!response.ok) throw await failureOf(response);
        const images = await response.json() as typeof customImages;
        if (!controller.signal.aborted) {
          const ready = images.filter((image) => image.status === "ready");
          setCustomImages(ready);
        }
      } catch (cause) {
        if (!controller.signal.aborted) setCustomImagesFailure(asReport(cause));
      } finally {
        if (!controller.signal.aborted) setCustomImagesLoading(false);
      }
    })();
    return () => controller.abort();
  }, [source]);

  /*
   * What the pasted file declares, as the Platform reads it. Asked for as the
   * Operator types, so the web service and port come from what is in the file
   * rather than from memory — 9191 for 9119 is one keystroke, and a 502.
   */
  const inspect = useCallback(async (text: string) => {
    if (!text.trim()) {
      setInspected(null);
      return;
    }
    try {
      const res = await api("/compose/inspect", {
        method: "POST",
        body: JSON.stringify({ compose: text }),
      });
      if (!res.ok) {
        setInspected(null);
        const report = await failureOf(res);
        setErrors((current) => ({ ...current, compose: report.caused_by[0] || report.error }));
        return;
      }
      setErrors((current) => ({ ...current, compose: undefined }));
      setInspected((await res.json()) as Inspection);
    } catch {
      setInspected(null);
    }
  }, []);

  // The file may have changed while the request was out; a stale answer would
  // describe a file that is no longer in the editor.
  useEffect(() => {
    if (source !== "compose") return;
    const timer = window.setTimeout(() => void inspect(compose), 400);
    return () => window.clearTimeout(timer);
  }, [compose, source, inspect]);

  // The service to route to: what the Application already picked, else the
  // Platform's own default, else the first one in the file.
  useEffect(() => {
    if (source !== "compose") return;
    if (!services.length) return;
    if (services.some((s) => s.name === webService)) return;
    setWebService(app?.web_service && services.some((s) => s.name === app.web_service)
      ? app.web_service
      : inspected?.web_service && services.some((s) => s.name === inspected.web_service)
        ? inspected.web_service
        : services[0].name);
  }, [source, services, webService, app?.web_service, inspected?.web_service]);

  // A listed port needs no choosing; anything else the file does not mention
  // is typed into the number input.
  useEffect(() => {
    if (source !== "compose") return;
    if (!ports.length) return;
    if (port && ports.includes(port)) return;
    if (port) setOther(true);
    else setPort(ports[0]);
  }, [source, ports, port]);

  function chooseService(next: string) {
    setWebService(next);
    setPort("");
    setOther(false);
  }

  function choosePort(next: string) {
    if (next === "other") {
      setOther(true);
      setPort("");
    } else {
      setOther(false);
      setPort(next);
    }
  }

  /*
   * Puts a refusal on the field it is about, or under the form when it is
   * about nothing in particular.
   */
  function place(report: Report, sent: string[]) {
    if (report.error.startsWith("invalid Application Hostname:")) {
      const onAlias = sent.some((alias) => report.error.includes(`'${alias}'`));
      setErrors({ [onAlias ? "aliases" : "hostname"]: asReport(report).error });
      setGitDomains(true);
      (onAlias ? aliasesField : hostnameField).current?.focus();
      return;
    }
    if (report.error.startsWith("invalid Compose definition") && source === "compose") {
      setErrors({ compose: report.caused_by[0] || report.error });
      return;
    }
    setFailure(report);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (submitting || checkingGit) return;
    if (creating && source === "git" && !gitReviewed) {
      await inspectGit();
      return;
    }
    if (creating && source === "git" && !recipeChosen) return;
    setSubmitting(true);
    try {
      setErrors({});
      setFailure(null);

      const sent = source === "git" && unpublished ? [] : parseAliases(aliases);
      const body: Submission = { name, aliases: sent };
      // Editing always sends the Hostname: an empty one is a mistake here, not
      // a request for the default.
      if ((hostname.trim() || !creating) && !(source === "git" && unpublished)) body.hostname = hostname.trim();
      if (source === "custom-image") {
        if (!customImageId || !customImageTag || customImagesLoading || customImagesFailure || !startCommand.trim() || !devPort) return;
        body.development = {
          image_id: customImageId,
          tag: customImageTag,
          command: startCommand.trim(),
          web_port: Number(devPort),
          persist_data: persistData,
        };
      } else if (source === "git") {
        try { body.git = gitSourceOf(git); } catch (cause) {
          setFailure({ error: (cause as Error).message, caused_by: [] });
          return;
        }
        if (git.token) {
          if (!git.username.trim()) { setFailure({ error: "A Git token needs a username.", caused_by: [] }); return; }
          try {
            const response = await api("/source/credentials", { method: "POST", body: JSON.stringify({ kind: "git", username: git.username.trim(), value: git.token }) });
            if (!response.ok) { setFailure(await failureOf(response)); return; }
            const credential = await response.json() as { id: string };
            body.git.credential_id = credential.id;
            setGit({ ...git, credentialId: credential.id, token: "", username: "" });
          } catch { setFailure({ error: "Could not save the Git credential.", caused_by: [] }); return; }
        }
        if (!unpublished) {
          if (git.composePath.trim()) body.web_service = webService.trim();
          if (port.trim()) body.web_port = Number(port.trim());
        }
        if (creating) {
          body.source_revision = gitInspection?.revision;
          body.publication = { kind: unpublished ? "unpublished" : "web" };
        } else body.refresh_source = false;
      } else if (source === "compose") {
        body.compose = compose;
        body.web_service = webService.trim();
        if (port.trim()) body.web_port = Number(port.trim());
      } else {
        body.image = image.trim();
      }

      if (!creating && source !== "git") body.pull = pull;

      const report = await onSubmit(body, source);
      if (report) place(report, sent);
    } finally {
      setSubmitting(false);
    }
  }

  const webHelp = !chosen
    ? "The service and the port it listens on inside its container. The hostname reaches it through Traefik; nothing is published on the Host."
    : chosen.ports.length > 1
      ? `${chosen.name} listens on ${chosen.ports.map((p) => p.container).join(", ")} inside its container. Pick the one the browser should reach; the hostname gets there through Traefik.`
      : chosen.ports.length === 1
        ? `${chosen.name} listens on ${chosen.ports[0].container} inside its container; the hostname reaches it through Traefik.`
        : `${chosen.name} declares no ports. Type the port it listens on inside its container.`;

  const composeFields = (
    <>
      <FormField
        id="f-compose"
        label="Compose file"
        error={errors.compose}
        hint={
          creating
            ? "The supported subset is in docs/compose-applications.md. ~ and ./ paths land in the application's data directory."
            : "Saving brings the project up again; only services whose definition changed are recreated."
        }
      >
        <ComposeEditor
          id="f-compose"
          value={compose}
          onChange={setCompose}
          required={source === "compose"}
        />
      </FormField>

      <div className="field-row">
        <Select value={webService} onValueChange={chooseService} disabled={!services.length}>
          <FormField id="f-web-service" label="Web service">
            <SelectTrigger className="mono">
              <SelectValue placeholder="Paste a Compose file first" />
            </SelectTrigger>
          </FormField>
          <SelectContent>
            {services.map((service) => (
              <SelectItem key={service.name} value={service.name}>
                {service.name}
                {service.ports.length ? "" : " (no ports)"}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <div className="field">
          {ports.length ? (
            <Select value={other ? "other" : port} onValueChange={choosePort}>
              <FormField id="f-web-port" label="Web port">
                <SelectTrigger className="mono">
                  <SelectValue />
                </SelectTrigger>
              </FormField>
              <SelectContent>
                {ports.map((value) => (
                  <SelectItem key={value} value={value}>
                    {value}
                  </SelectItem>
                ))}
                <SelectItem value="other">Other…</SelectItem>
              </SelectContent>
            </Select>
          ) : null}
          {!ports.length || other ? (
            <FormField
              id="f-web-port-manual"
              label={ports.length ? "Custom web port" : "Web port"}
              className="mono"
              type="number"
              min={1}
              max={65535}
              placeholder="9119"
              value={port}
              onChange={(event) => setPort(event.target.value)}
              onWheel={(event) => event.currentTarget.blur()}
            />
          ) : null}
        </div>
      </div>

      <small className="field-hint" id="f-web-help">
        {webHelp}
      </small>

      <Storage services={services} />
    </>
  );

  const imageField = (
    <FormField
      label="Container image"
      id="f-image"
      className="mono"
      value={image}
      onChange={(event) => setImage(event.target.value)}
      placeholder="nginx:alpine"
      required={source === "image"}
      hint="One container serving HTTP on port 80."
    />
  );

  const routingFields = <>
      <FormField
        id="f-hostname"
        label="Hostname"
        error={errors.hostname}
        hint={
          creating
            ? "Leave empty to generate a hostname."
            : "Takes effect immediately; the container keeps running."
        }
      >
        <Input
          ref={hostnameField}
          className="mono"
          id="f-hostname"
          value={hostname}
          onChange={(event) => setHostname(event.target.value)}
          placeholder={creating ? "Automatic" : `my-app.${dnsSuffix}`}
          required={!creating}
        />
      </FormField>

      <FormField
        id="f-aliases"
        label="Aliases"
        error={errors.aliases}
        hint={`Other hostnames this application also answers on, comma separated.${
          creating ? "" : " Keep the old one here to change the Hostname without breaking it."
        }`}
      >
        <Input
          ref={aliasesField}
          className="mono"
          id="f-aliases"
          value={aliases}
          onChange={(event) => setAliases(event.target.value)}
          placeholder={`old-name.${dnsSuffix}`}
        />
      </FormField>

  </>;

  if (source === "native") return <NativeAppForm app={app} dnsSuffix={dnsSuffix} onSubmit={onSubmit} onCancel={onCancel} onReload={onReload} onChangeDefinition={creating ? () => setSource("image") : undefined} />;

  return (
    <Form id="app-form" onSubmit={submit}>
      <div className="form-body">
      {source !== "git" ? <p className="t-caps">Configuration</p> : null}

      {source !== "git" ? <FormField
        label="Name"
        id="f-name"
        value={name}
        onChange={(event) => setName(event.target.value)}
        placeholder="My app"
        required
        autoFocus={creating}
      /> : null}

      {creating ? (
        <fieldset className="field source-choice">
          <legend>Definition</legend>
          {[
            ["image", "Container image"],
            ["custom-image", "Custom image"],
            ["compose", "Compose file"],
            ["git", "Git repository"],
            ["native", "Native process"],
          ].map(([value, label]) => (
            <label className="row" key={value}>
              <input
                type="radio"
                name="f-source"
                value={value}
                checked={source === value}
                onChange={() => {
                  if (value !== "git") { ++gitRequest.current; setCheckingGit(false); }
                  setSource(value);
                }}
              />{" "}
              {label}
            </label>
          ))}
        </fieldset>
      ) : null}

      {source === "git" ? <GitWorkflowFields
        fields={git} onChange={changeGit} creating={creating}
        mode={gitMode} onModeChange={setGitMode} setup={gitSetup}
        onEditSource={() => { setGitSetup(false); setGitReviewKey(""); }}
        inspection={gitInspection} reviewed={gitReviewed} checking={checkingGit}
        disabled={submitting} onImport={importRepository}
        recipeChosen={recipeChosen} onRecipeChange={chooseGitRecipe}
        name={name} onNameChange={setName} port={port}
        onPortChange={(next) => { portEdited.current = true; setPort(next); }}
        webService={webService} onWebServiceChange={chooseGitService}
        unpublished={unpublished} onUnpublishedChange={setUnpublished} dnsSuffix={dnsSuffix}
        routingFields={!unpublished && (!creating || gitSetup) ? creating ? (
          <details className="disclosure" open={gitDomains} onToggle={(event) => setGitDomains(event.currentTarget.open)}>
            <summary>Domain settings</summary>
            <div className="git-settings-fields">{routingFields}</div>
          </details>
        ) : routingFields : null}
      /> : source === "compose" ? composeFields : source === "custom-image" ? (
        <>
          <FormField id="f-custom-image" label="Custom image">
            <select id="f-custom-image" className="input" required value={customImageTag}
              disabled={customImagesLoading || !!customImagesFailure || (!customImages.length && !customImageTag)}
              onChange={(event) => {
                const selected = customImages.find((image) => image.image === event.target.value);
                if (selected) {
                  const template = customImageTemplate(selected.template_id);
                  if (creating && !customImageId && !runtimeEdited.current && template) {
                    setStartCommand(template.application.command);
                    setDevPort(String(template.application.web_port));
                    setPersistData(template.application.persist_data);
                  }
                  setCustomImageId(selected.id);
                  setCustomImageTag(selected.image);
                }
              }}>
              <option value="">{customImagesLoading ? "Loading images..." : "Select a saved image"}</option>
              {customImageTag && !customImages.some((image) => image.image === customImageTag) ? (
                <option value={customImageTag}>Deployed build: {customImageTag}</option>
              ) : null}
              {customImages.map((image) => <option key={image.image} value={image.image}>{image.name}</option>)}
            </select>
          </FormField>
          {customImageTag ? <code className="custom-image-tag">{customImageTag}</code> : null}
          {customImageId && customImages.some((image) => image.id === customImageId && image.image !== customImageTag) ? (
            <p className="muted t-label">A newer successful build is available. Select it and save to redeploy.</p>
          ) : null}
          <FormField id="f-start-command" label="Start command" className="mono"
            value={startCommand} onChange={(event) => { runtimeEdited.current = true; setStartCommand(event.target.value); }} required
            placeholder="t3 serve --host 0.0.0.0 --port 3000"
            hint="Call installed tools directly, e.g. t3. The server must listen on 0.0.0.0 and the web port below." />
          <FormField id="f-dev-port" label="Web port" type="number" min={1} max={65535}
            value={devPort} onChange={(event) => { runtimeEdited.current = true; setDevPort(event.target.value); }} required placeholder="3000"
            onWheel={(event) => event.currentTarget.blur()}
            hint="The port your server listens on inside the container." />
          <div className="field">
            <div className="check">
              <Checkbox id="f-persist-data" checked={persistData}
                onCheckedChange={(checked) => { runtimeEdited.current = true; setPersistData(checked === true); }}
                aria-describedby="f-persist-data-hint" />
              <Label htmlFor="f-persist-data">Persist data</Label>
            </div>
            <p id="f-persist-data-hint" className="muted t-label">
              Keep files in /data when the container is recreated. Configure your server to store its data there.
            </p>
          </div>
          {customImagesFailure ? <Failure failure={customImagesFailure} /> : null}
          {!customImagesLoading && !customImagesFailure && !customImages.length && !customImageTag ? (
            <p className="muted t-label">No custom images are ready. <a href="/console/#custom-images">Save and build an image first.</a></p>
          ) : null}
        </>
      ) : imageField}

      {source !== "git" ? routingFields : null}

      {failure ? <Failure failure={failure} /> : null}
      </div>

      {creating ? (
        <FormActions sticky message={source === "git" ? checkingGit ? "Checking the selected repository." : !gitSetup ? gitMode === "repositories" ? "Import a repository, or use its URL." : "Check the repository before building." : !gitReviewed ? "Check the changed source before building." : "Build and deploy the reviewed commit." : undefined}>
          <Button size="sm" type="button" onClick={onCancel}>
            Cancel
          </Button>
          {source !== "git" || gitSetup || gitMode === "url" ? <Button size="sm" variant="primary" type="submit"
            disabled={submitting || checkingGit || source === "custom-image" && (!customImageId || !customImageTag || customImagesLoading || !!customImagesFailure) || source === "git" && (!git.repository.trim() || gitSetup && gitReviewed && (!recipeChosen || !unpublished && (!Number.isInteger(Number(port)) || Number(port) < 1 || Number(port) > 65535)))}>
            {source === "git" ? checkingGit ? "Checking..." : gitReviewed && recipeChosen ? "Build and deploy" : gitSetup ? "Check repository" : "Continue" : "Deploy"}
          </Button> : null}
        </FormActions>
      ) : (
        /* Removing lives in the header row with the other lifecycle verbs,
           so the form's footer is only about saving. A redeploy without an
           edit is still an ask when it pulls newer images. An image built on
           this Host has nothing to pull, so it gets no checkbox. */
        <FormActions
          sticky
          tone={dirty ? "warning" : "neutral"}
          message={dirty ? <><strong>Unsaved changes.</strong> {changedGitSelection ? "Saving builds the selected repository, ref or pinned commit." : "The Application keeps running as it is until you save."}</> : source === "git" ? app?.git_build?.revision ? `Saved. Rebuild commit ${app.git_build.revision.slice(0, 12)}. Check for updates to review a newer version.` : "Saved. A failed build keeps the running Application." : pull && source !== "custom-image" ? "Saved. Redeploying pulls newer images first." : "Saved."}
        >
          {source !== "git" && source !== "custom-image" ? (
            <div className="check">
              <Checkbox id="f-pull" checked={pull} onCheckedChange={(checked) => setPull(checked === true)} />
              <Label htmlFor="f-pull">Pull newer images</Label>
            </div>
          ) : null}
          {dirty ? (
            <Button size="sm" type="button" onClick={reset}>
              Discard
            </Button>
          ) : null}
          <Button size="sm" variant="primary" type="submit" disabled={submitting}>
            {source === "git" ? "Save and build" : "Save and redeploy"}
          </Button>
        </FormActions>
      )}
    </Form>
  );
}

/**
 * What each service keeps, and where the Platform puts it. A service with no
 * storage at all is worth a sentence: whatever it writes goes with the
 * container on the next redeploy.
 */
function Storage({ services }: { services: ComposeService[] }) {
  if (!services.length) return null;

  const rows: React.ReactNode[] = [];
  for (const service of services) {
    if (!service.volumes.length) {
      rows.push(
        <li key={service.name}>
          <span className="mono">{service.name}</span> keeps nothing: what it writes is lost when
          its container is recreated.
        </li>,
      );
      continue;
    }
    for (const volume of service.volumes) {
      const where =
        volume.kind === "data" ? (
          <>
            the application's directory, <span className="mono">{volume.data_path}</span>
          </>
        ) : volume.kind === "named" ? (
          <>
            a named volume, <span className="mono">{volume.source}</span>
          </>
        ) : volume.kind === "host" ? (
          <>
            the Host path <span className="mono">{volume.source}</span>, as written
          </>
        ) : (
          <>an anonymous volume, gone with the container</>
        );
      rows.push(
        <li key={`${service.name}:${volume.target}`}>
          <span className="mono">{service.name}</span>: <span className="mono">{volume.target}</span>{" "}
          lives in {where}.
        </li>,
      );
    }
  }

  return (
    <div className="field">
      <p className="t-caps">Storage</p>
      <ul className="storage-list">{rows}</ul>
      <small className="field-hint">
        Created on deploy and kept across redeploys, restarts and removal.
      </small>
    </div>
  );
}
