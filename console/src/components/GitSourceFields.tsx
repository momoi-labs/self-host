import { FormField, Input, Textarea } from "@momoi-labs/kiso-react";
import type { GitSource } from "../lib/types.js";

export type GitFields = {
  repository: string;
  gitRef: string;
  revision: string;
  context: string;
  dockerfile: string;
  composePath: string;
  args: string;
  secrets: string;
  credentialId: string;
  registryCredentialId: string;
  username: string;
  token: string;
};

export function gitFieldsOf(source?: GitSource): GitFields {
  const lines = (values?: Record<string, string>) => Object.entries(values ?? {}).map(([name, value]) => `${name}=${value}`).join("\n");
  return {
    repository: source?.repository ?? "", gitRef: source?.git_ref ?? "HEAD",
    revision: source?.revision ?? "", context: source?.context ?? ".",
    dockerfile: source?.dockerfile ?? "Dockerfile", composePath: source?.compose_path ?? "",
    args: lines(source?.build_args), secrets: lines(source?.build_secrets),
    credentialId: source?.credential_id ?? "", registryCredentialId: source?.registry_credential_id ?? "",
    username: "", token: "",
  };
}

export function gitSourceOf(fields: GitFields): GitSource {
  const pairs = (text: string) => Object.fromEntries(text.split("\n").filter((line) => line.trim()).map((line) => {
    const separator = line.indexOf("=");
    if (separator < 1) throw new Error("Build inputs need one NAME=value per line.");
    return [line.slice(0, separator).trim(), line.slice(separator + 1)];
  }));
  return {
    repository: fields.repository.trim(), git_ref: fields.gitRef.trim() || "HEAD",
    context: fields.context.trim() || ".", dockerfile: fields.dockerfile.trim() || "Dockerfile",
    revision: fields.revision.trim() || undefined, compose_path: fields.composePath.trim() || undefined,
    build_args: pairs(fields.args), build_secrets: pairs(fields.secrets),
    credential_id: fields.credentialId.trim() || undefined,
    registry_credential_id: fields.registryCredentialId.trim() || undefined,
  };
}

export function GitSourceFields({ fields, onChange, includeRepository = true }: {
  fields: GitFields;
  onChange: (fields: GitFields) => void;
  includeRepository?: boolean;
}) {
  const set = (key: keyof GitFields, value: string) => onChange({ ...fields, [key]: value });
  return <>
    {includeRepository ? <FormField id="f-git-repository" label="Repository URL" hint="HTTP or HTTPS URL.">
      <Input id="f-git-repository" type="url" required value={fields.repository} placeholder="https://git.example.invalid/team/app.git" onChange={(event) => set("repository", event.target.value)} />
    </FormField> : null}
    <div className="field-row">
      <FormField id="f-git-ref" label="Branch or tag" value={fields.gitRef} onChange={(event) => set("gitRef", event.target.value)} placeholder="HEAD" />
      <FormField id="f-git-revision" label="Pinned commit" className="mono" value={fields.revision} onChange={(event) => set("revision", event.target.value)} hint="Optional full commit id. Updates keep this commit until you change or clear it." />
    </div>
    <FormField id="f-git-compose" label="Compose path" value={fields.composePath} onChange={(event) => set("composePath", event.target.value)} placeholder="compose.yaml" hint="Optional path inside the checkout. Its build and env_file inputs also stay inside the checkout." />
    {!fields.composePath.trim() ? <div className="field-row">
      <FormField id="f-git-context" label="Build context" value={fields.context} onChange={(event) => set("context", event.target.value)} placeholder="." />
      <FormField id="f-git-dockerfile" label="Dockerfile path" value={fields.dockerfile} onChange={(event) => set("dockerfile", event.target.value)} placeholder="Dockerfile" hint="Relative to the checkout root. Every RUN needs a numeric nonzero USER." />
    </div> : null}
    <FormField id="f-git-args" label="Build arguments" hint="Ordinary inputs only. One NAME=value per line.">
      <Textarea id="f-git-args" className="mono" rows={3} value={fields.args} onChange={(event) => set("args", event.target.value)} />
    </FormField>
    <FormField id="f-git-secrets" label="Build secrets" hint="One BuildKit mount name=credential id per line. Secret values stay in private storage.">
      <Textarea id="f-git-secrets" className="mono" rows={3} value={fields.secrets} onChange={(event) => set("secrets", event.target.value)} />
    </FormField>
    <FormField id="f-registry-credential" label="Registry credential id" value={fields.registryCredentialId} onChange={(event) => set("registryCredentialId", event.target.value)} hint="Optional saved registry credential for private base images." />
  </>;
}
