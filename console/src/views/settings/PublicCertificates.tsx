import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Button,
  Form,
  FormActions,
  FormField,
  GridPane,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Failure } from "../../components/Failure.js";
import { api, asReport, failureOf, readJson } from "../../lib/api.js";
import { bootstrapStatusQuery } from "../../lib/queries.js";
import type { AcmeSettings, Certificate, Report } from "../../lib/types.js";
import type { PaneProps } from "./General.js";

const LETS_ENCRYPT = "https://acme-v02.api.letsencrypt.org/directory";
const LETS_ENCRYPT_STAGING = "https://acme-staging-v02.api.letsencrypt.org/directory";

/** The CAs the select names; anything else is a custom directory URL. */
const authorities = [
  { value: LETS_ENCRYPT, label: "Let's Encrypt" },
  { value: LETS_ENCRYPT_STAGING, label: "Let's Encrypt staging" },
  { value: "custom", label: "Another ACME service" },
] as const;

function authorityOf(directoryUrl: string): string {
  return authorities.some((one) => one.value === directoryUrl) ? directoryUrl : "custom";
}

const noCertificates: Certificate[] = [];

const tone: Record<Certificate["state"], "success" | "danger" | "neutral"> = {
  valid: "success",
  renewal_failed: "danger",
  expired: "danger",
  pending: "neutral",
};

const label: Record<Certificate["state"], string> = {
  valid: "Valid",
  renewal_failed: "Renewal failed",
  expired: "Expired",
  pending: "Pending",
};

function day(seconds: number | null): string {
  return seconds === null ? "" : new Date(seconds * 1000).toLocaleDateString();
}

/**
 * The public certificates card of Settings: the ACME account every Hostname
 * outside the DNS Suffix is issued under, and how each certificate stands.
 * Saving writes the account; the daemon reads it, and the Applications'
 * Hostnames, within a minute.
 */
export function PublicCertificates({ id, size }: PaneProps) {
  const queryClient = useQueryClient();
  const acmeKey = ["certificates", "acme"];
  const acme = useQuery({
    queryKey: acmeKey,
    queryFn: ({ signal }) => readJson<AcmeSettings | null>("/certificates/acme", signal),
  });
  const saved = acme.data ?? null;
  const enabled = saved !== null;
  const certificates = useQuery({
    queryKey: ["certificates"],
    queryFn: ({ signal }) => readJson<Certificate[]>("/certificates", signal),
    refetchInterval: 15_000,
  });
  const suffix = useQuery(bootstrapStatusQuery).data?.dns_suffix;

  const [authority, setAuthority] = useState<string>(LETS_ENCRYPT);
  const [directoryUrl, setDirectoryUrl] = useState(LETS_ENCRYPT);
  const [email, setEmail] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<Report | null>(null);

  // The fields start from what is saved, once, so a later read never undoes
  // an edit in progress.
  const started = useRef(false);
  useEffect(() => {
    if (acme.data === undefined || started.current) return;
    started.current = true;
    reset(acme.data);
  }, [acme.data]);

  function reset(settings: AcmeSettings | null) {
    const url = settings?.directory_url ?? LETS_ENCRYPT;
    setAuthority(authorityOf(url));
    setDirectoryUrl(url);
    setEmail(settings?.email ?? "");
  }

  const url = authority === "custom" ? directoryUrl.trim() : authority;
  const dirty = !saved ? email.trim() !== "" || url !== LETS_ENCRYPT : url !== saved.directory_url || email.trim() !== saved.email;
  const valid = email.trim().includes("@") && url.startsWith("https://");

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!valid || saving) return;
    setSaving(true);
    setError(null);
    try {
      const response = await api("/certificates/acme", {
        method: "PUT",
        body: JSON.stringify({ directory_url: url, email: email.trim() }),
      });
      if (!response.ok) { setError(await failureOf(response)); return; }
      const next = (await response.json()) as AcmeSettings;
      queryClient.setQueryData(acmeKey, next);
      reset(next);
    } catch (cause) {
      setError(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  async function turnOff() {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      const response = await api("/certificates/acme", { method: "DELETE" });
      if (!response.ok) { setError(await failureOf(response)); return; }
      // The fields keep the account that was just turned off, so turning
      // it on again is one Save, not a form to fill in from memory.
      queryClient.setQueryData(acmeKey, null);
      queryClient.setQueryData(["certificates"], noCertificates);
    } catch (cause) {
      setError(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  const authorityName = authorities.find((one) => one.value === saved?.directory_url)?.label ?? saved?.directory_url;
  const message = dirty ? (
    <><strong>Unsaved changes.</strong> The daemon picks the account up within a minute of saving.</>
  ) : enabled ? (
    `Saved. Every Hostname outside .${suffix ?? "the Suffix"} gets a certificate from ${authorityName}.`
  ) : (
    "Off. Hostnames outside the DNS Suffix refuse HTTPS. Save an account to turn it on."
  );

  const rows = certificates.data ?? noCertificates;

  return (
    <GridPane id={id} size={size} title="Public certificates">
      <Form id="public-certificates" onSubmit={(event) => void save(event)}>
        <div className="form-body">
          <p className="muted t-label">
            Certificates from a public CA for the Hostnames a browser cannot trust the Platform's own CA for.
            Their public DNS must point at this Host, and the CA must reach port 80.
          </p>
          <Select value={authority} disabled={acme.data === undefined || saving} onValueChange={(next) => {
            // Radix answers a value set before its items mounted with an empty
            // change; every real choice here has a value.
            if (!next) return;
            setAuthority(next);
            if (next !== "custom") setDirectoryUrl(next);
          }}>
            <FormField id="acme-authority" label="Certificate authority">
              <SelectTrigger><SelectValue /></SelectTrigger>
            </FormField>
            <SelectContent>
              {authorities.map((one) => <SelectItem key={one.value} value={one.value}>{one.label}</SelectItem>)}
            </SelectContent>
          </Select>
          {authority === "custom" ? (
            <FormField
              id="acme-directory-url"
              label="Directory URL"
              className="mono"
              placeholder="https://acme.example.com/directory"
              value={directoryUrl}
              disabled={saving}
              onChange={(event) => setDirectoryUrl(event.target.value)}
              error={directoryUrl.trim() && !directoryUrl.trim().startsWith("https://") ? "The directory must be an https:// URL." : null}
            />
          ) : null}
          <FormField
            id="acme-email"
            label="Contact email"
            type="email"
            placeholder="operator@example.com"
            autoComplete="off"
            value={email}
            disabled={acme.data === undefined || saving}
            onChange={(event) => setEmail(event.target.value)}
            hint="The CA writes here before a certificate it issued expires unrenewed. Saving accepts its terms of service."
          />
          {error ? <Failure failure={error} /> : null}
          {acme.error ? <Failure failure={acme.error} actionLabel="Retry" onAction={() => void acme.refetch()} /> : null}
        </div>
        <FormActions tone={dirty ? "warning" : "neutral"} message={message}>
          {dirty ? <Button size="sm" type="button" disabled={saving} onClick={() => reset(saved)}>Discard</Button> : null}
          {enabled ? <Button size="sm" type="button" variant="ghost" className="btn-danger-ghost" disabled={saving} onClick={() => void turnOff()}>Turn off</Button> : null}
          <Button size="sm" variant="primary" type="submit" disabled={saving || !dirty || !valid}>Save</Button>
        </FormActions>
      </Form>

      {enabled ? (
        <div className="table-wrap">
          <div className="table-scroll">
            <Table aria-label="Public certificates">
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">Hostname</TableHead>
                  <TableHead scope="col">Status</TableHead>
                  <TableHead scope="col">Expires</TableHead>
                  <TableHead scope="col">Renews</TableHead>
                  <TableHead scope="col">Last error</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={5} className="muted">
                      No public Hostnames yet. Give an Application a Hostname outside .{suffix ?? "the Suffix"} and it shows here within a minute.
                    </TableCell>
                  </TableRow>
                ) : rows.map((certificate) => (
                  <TableRow key={certificate.hostname}>
                    <TableCell className="mono">{certificate.hostname}</TableCell>
                    <TableCell><StatusBadge tone={tone[certificate.state]}>{label[certificate.state]}</StatusBadge></TableCell>
                    <TableCell>{day(certificate.expires_at)}</TableCell>
                    <TableCell>{day(certificate.renew_at)}</TableCell>
                    <TableCell className="muted">{certificate.last_error ?? ""}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
          <p className="table-footer"><span>{rows.length} {rows.length === 1 ? "certificate" : "certificates"}</span></p>
        </div>
      ) : null}
    </GridPane>
  );
}
