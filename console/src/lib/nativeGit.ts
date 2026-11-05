import type { GitFields } from "../components/GitSourceFields.js";
import type { ImageDependency } from "./customImageTemplates.js";
import type { GitSource, NativeDefinition } from "./types.js";

/** Native checkouts share Git access settings, never container build settings. */
export function nativeGitSource(fields: GitFields): GitSource {
  return {
    repository: fields.repository.trim(),
    git_ref: fields.gitRef.trim() || "HEAD",
    revision: fields.revision.trim() || undefined,
    context: fields.context.trim() || ".",
    credential_id: fields.credentialId.trim() || undefined,
    dockerfile: "Dockerfile",
    build_args: {},
    build_secrets: {},
  };
}

export function nativeGitFields(native?: NativeDefinition | null) {
  return {
    build: (native?.build ?? []).join("\n"),
    worker: !!native?.worker,
    workerConfig: native?.worker?.config ?? "wrangler.jsonc",
  };
}

export function nativeGitBuild(fields: ReturnType<typeof nativeGitFields>): Pick<NativeDefinition, "build" | "worker"> {
  return {
    build: fields.build.split(/\r?\n/).filter((line) => line.trim().length > 0),
    worker: fields.worker ? { config: fields.workerConfig.trim() } : null,
  };
}

const workerTools: ImageDependency[] = [
  { tool: "node", version: "24.22.2" },
  { tool: "npm:wrangler", version: "4.157.0" },
  { tool: "github:cloudflare/workerd", version: "1.20261103.1" },
];

/** Start with the supported Worker tools without removing custom dependencies. */
export function workerDependencies(dependencies: ImageDependency[]): ImageDependency[] {
  return [
    ...dependencies.map((dependency) => {
      const required = workerTools.find(({ tool }) => tool === dependency.tool);
      return required ? { ...dependency, version: required.version } : dependency;
    }),
    ...workerTools.filter(({ tool }) => !dependencies.some((dependency) => dependency.tool === tool)).map((dependency) => ({ ...dependency })),
  ];
}
