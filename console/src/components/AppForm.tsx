import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import {
  Button,
  Checkbox,
  FormField,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import { isCompose, parseAliases } from "../lib/status.js";
import type { App, ComposeService, Inspection, Report } from "../lib/types.js";
import { ComposeEditor } from "./ComposeEditor.js";
import { Failure } from "./Failure.js";
import { Services } from "./Services.js";
import { devImageTemplate } from "../lib/devImageTemplates.js";

export type Submission = {
  name: string;
  aliases: string[];
  hostname?: string;
  compose?: string;
  web_service?: string;
  web_port?: number;
  image?: string;
  development?: {
    image_id: string;
    tag: string;
    command: string;
    web_port: number;
    persist_data: boolean;
  };
};

type DevImage = { id: string; name: string; image: string; status: string; template_id?: string | null };

type Errors = {
  hostname?: string;
  aliases?: string;
  compose?: string;
};

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
  onRemove,
  removing,
}: {
  app?: App;
  dnsSuffix: string;
  onSubmit: (body: Submission, source: string) => Promise<Report | null>;
  onCancel?: () => void;
  onRemove?: () => void;
  removing?: boolean;
}) {
  const creating = !app;
  const [source, setSource] = useState(creating ? "image" : app?.development ? "dev-image" : isCompose(app) ? "compose" : "image");
  const [name, setName] = useState(app?.name ?? "");
  const [image, setImage] = useState(app?.image ?? "");
  const [devImageId, setDevImageId] = useState(app?.development?.image_id ?? "");
  const [devImageTag, setDevImageTag] = useState(app?.development?.tag ?? "");
  const [startCommand, setStartCommand] = useState(app?.development?.command ?? "");
  const [devPort, setDevPort] = useState(app?.development ? String(app.development.web_port) : "");
  const [persistData, setPersistData] = useState(app?.development?.persist_data ?? false);
  const runtimeEdited = useRef(false);
  const [devImages, setDevImages] = useState<DevImage[]>([]);
  const [devImagesLoading, setDevImagesLoading] = useState(false);
  const [devImagesFailure, setDevImagesFailure] = useState<Report | null>(null);
  const [compose, setCompose] = useState(app?.compose ?? "");
  const [hostname, setHostname] = useState(app?.hostname ?? "");
  const [aliases, setAliases] = useState((app?.aliases ?? []).join(", "));
  const [webService, setWebService] = useState(app?.web_service ?? "");
  const [port, setPort] = useState(app?.web_port ? String(app.web_port) : "");
  const [other, setOther] = useState(false);
  const [inspected, setInspected] = useState<Inspection | null>(null);
  const [errors, setErrors] = useState<Errors>({});
  const [failure, setFailure] = useState<Report | null>(null);
  const hostnameField = useRef<HTMLInputElement>(null);
  const aliasesField = useRef<HTMLInputElement>(null);

  const services = inspected?.services ?? [];
  const chosen: ComposeService | undefined = services.find((s) => s.name === webService);
  const ports = chosen ? [...new Set(chosen.ports.map((p) => String(p.container)))] : [];

  useEffect(() => {
    if (source !== "dev-image") return;
    const controller = new AbortController();
    setDevImagesLoading(true);
    setDevImagesFailure(null);
    void (async () => {
      try {
        const response = await api("/dev-images", { signal: controller.signal });
        if (!response.ok) throw await failureOf(response);
        const images = await response.json() as typeof devImages;
        if (!controller.signal.aborted) {
          const ready = images.filter((image) => image.status === "ready");
          setDevImages(ready);
        }
      } catch (cause) {
        if (!controller.signal.aborted) setDevImagesFailure(asReport(cause));
      } finally {
        if (!controller.signal.aborted) setDevImagesLoading(false);
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
    if (!services.length) return;
    if (services.some((s) => s.name === webService)) return;
    setWebService(app?.web_service && services.some((s) => s.name === app.web_service)
      ? app.web_service
      : inspected?.web_service && services.some((s) => s.name === inspected.web_service)
        ? inspected.web_service
        : services[0].name);
  }, [services, webService, app?.web_service, inspected?.web_service]);

  // A listed port needs no choosing; anything else the file does not mention
  // is typed into the number input.
  useEffect(() => {
    if (!ports.length) return;
    if (port && ports.includes(port)) return;
    if (port) setOther(true);
    else setPort(ports[0]);
  }, [ports, port]);

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
    setErrors({});
    setFailure(null);

    const sent = parseAliases(aliases);
    const body: Submission = { name: name.trim(), aliases: sent };
    // Editing always sends the Hostname: an empty one is a mistake here, not
    // a request for the default.
    if (hostname.trim() || !creating) body.hostname = hostname.trim();
    if (source === "dev-image") {
      if (!devImageId || !devImageTag || devImagesLoading || devImagesFailure || !startCommand.trim() || !devPort) return;
      body.development = {
        image_id: devImageId,
        tag: devImageTag,
        command: startCommand.trim(),
        web_port: Number(devPort),
        persist_data: persistData,
      };
    } else if (source === "compose") {
      body.compose = compose;
      body.web_service = webService.trim();
      if (port.trim()) body.web_port = Number(port.trim());
    } else {
      body.image = image.trim();
    }

    const report = await onSubmit(body, source);
    if (report) place(report, sent);
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

  return (
    <form className="stack" id="app-form" onSubmit={submit}>
      <p className="t-caps">Configuration</p>

      <FormField
        label="Name"
        id="f-name"
        value={name}
        onChange={(event) => setName(event.target.value)}
        placeholder="my-app"
        required
        autoFocus={creating}
        hint="Lowercase letters, numbers and hyphens; at most 63 characters."
      />

      {creating ? (
        <fieldset className="field source-choice">
          <legend>Definition</legend>
          {[
            ["image", "Container image"],
            ["dev-image", "Development image"],
            ["compose", "Compose file"],
          ].map(([value, label]) => (
            <label className="row" key={value}>
              <input
                type="radio"
                name="f-source"
                value={value}
                checked={source === value}
                onChange={() => setSource(value)}
              />{" "}
              {label}
            </label>
          ))}
        </fieldset>
      ) : null}

      {source === "compose" ? composeFields : source === "dev-image" ? (
        <>
          <FormField id="f-dev-image" label="Development image">
            <select id="f-dev-image" className="input" required value={devImageTag}
              disabled={devImagesLoading || !!devImagesFailure || (!devImages.length && !devImageTag)}
              onChange={(event) => {
                const selected = devImages.find((image) => image.image === event.target.value);
                if (selected) {
                  const template = devImageTemplate(selected.template_id);
                  if (creating && !devImageId && !runtimeEdited.current && template) {
                    setStartCommand(template.application.command);
                    setDevPort(String(template.application.web_port));
                    setPersistData(template.application.persist_data);
                  }
                  setDevImageId(selected.id);
                  setDevImageTag(selected.image);
                }
              }}>
              <option value="">{devImagesLoading ? "Loading images..." : "Select a saved image"}</option>
              {devImageTag && !devImages.some((image) => image.image === devImageTag) ? (
                <option value={devImageTag}>Deployed build: {devImageTag}</option>
              ) : null}
              {devImages.map((image) => <option key={image.image} value={image.image}>{image.name}</option>)}
            </select>
          </FormField>
          {devImageTag ? <code className="dev-image-tag">{devImageTag}</code> : null}
          {devImageId && devImages.some((image) => image.id === devImageId && image.image !== devImageTag) ? (
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
          {devImagesFailure ? <Failure failure={devImagesFailure} /> : null}
          {!devImagesLoading && !devImagesFailure && !devImages.length && !devImageTag ? (
            <p className="muted t-label">No development images are ready. <a href="/console/#dev-images">Save and build an image first.</a></p>
          ) : null}
        </>
      ) : imageField}

      <FormField
        id="f-hostname"
        label="Hostname"
        error={errors.hostname}
        hint={
          creating
            ? "Leave empty to use the application name and DNS suffix."
            : "Takes effect immediately; the container keeps running."
        }
      >
        <Input
          ref={hostnameField}
          className="mono"
          id="f-hostname"
          value={hostname}
          onChange={(event) => setHostname(event.target.value)}
          placeholder={`my-app.${dnsSuffix}`}
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

      {failure ? <Failure failure={failure} /> : null}

      {app ? <Services services={app.services ?? []} /> : null}

      <div className="form-actions">
        {creating ? (
          <>
            <Button size="sm" onClick={onCancel}>
              Cancel
            </Button>
            <Button size="sm" variant="primary" type="submit"
              disabled={source === "dev-image" && (!devImageId || !devImageTag || devImagesLoading || !!devImagesFailure)}>
              Deploy
            </Button>
          </>
        ) : (
          <>
            <Button size="sm" variant="ghost" className="btn-danger-ghost" onClick={onRemove} disabled={removing}>
              {removing ? "Removing..." : "Remove application"}
            </Button>
            <Button size="sm" variant="primary" type="submit" disabled={removing}>
              Save and redeploy
            </Button>
          </>
        )}
      </div>
    </form>
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
