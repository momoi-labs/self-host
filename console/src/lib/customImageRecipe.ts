import type { ImageDependency } from "./customImageTemplates.js";

/** The editor's fields. `dockerfile` is the mode: a string means the Operator
 * took the file over, `null` means the Host still generates it. */
export type RecipeFields = {
  name: string;
  templateId: string;
  dependencies: ImageDependency[];
  setup: string;
  buildChecks: string;
  dockerfile: string | null;
};

/** A shell editor holds one command per line; blank lines are not commands. */
export function commands(text: string): string[] {
  return text.split("\n").map((command) => command.trim()).filter(Boolean);
}

/**
 * The recipe as the Host reads it, in either mode. An edited Dockerfile owns
 * the whole recipe, so the builder's three fields go out empty rather than as
 * values the build would quietly ignore (ADR-0022). The fields keep their
 * values in the editor, which is what makes going back possible.
 */
export function recipeBody(fields: RecipeFields) {
  const shared = { name: fields.name.trim(), template_id: fields.templateId || null };
  if (fields.dockerfile !== null) {
    return { ...shared, dockerfile: fields.dockerfile, dependencies: [], setup: [], build_checks: [] };
  }
  return {
    ...shared,
    dependencies: fields.dependencies,
    setup: commands(fields.setup),
    build_checks: commands(fields.buildChecks),
  };
}
