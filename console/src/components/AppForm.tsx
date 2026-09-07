import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import {
  Button,
  FormField,
  Input,
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

export type Submission = {
  name: string;
  aliases: string[];
  hostname?: string;
  compose?: string;
  web_service?: string;
  web_port?: number;
  image?: string;
};

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
}: {
  app?: App;
  dnsSuffix: string;
  onSubmit: (body: Submission, source: string) => Promise<Report | null>;
  onCancel?: () => void;
  onRemove?: () => void;
}) {
  const creating = !app;
  const [source, setSource] = useState(creating ? "image" : isCompose(app) ? "compose" : "image");
  const [name, setName] = useState(app?.name ?? "");
  const [image, setImage] = useState(app?.image ?? "");
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
    if (source === "compose") {
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

      {source === "image" ? imageField : composeFields}

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
            <Button size="sm" variant="primary" type="submit">
              Deploy
            </Button>
          </>
        ) : (
          <>
            <Button size="sm" variant="ghost" className="btn-danger-ghost" onClick={onRemove}>
              Remove application
            </Button>
            <Button size="sm" variant="primary" type="submit">
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
