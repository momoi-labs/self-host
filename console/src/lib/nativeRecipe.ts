import type { NativeRecipe } from "./types.js";

export type NativeRecipeFields = {
  dependencies: NativeRecipe["dependencies"];
  setup: string;
};

export function nativeRecipeFields(recipe?: NativeRecipe): NativeRecipeFields {
  return {
    dependencies: structuredClone(recipe?.dependencies ?? []),
    setup: (recipe?.setup ?? []).join("\n"),
  };
}

/** Keep shell quoting and whitespace intact. Only blank lines are omitted. */
export function nativeRecipe(fields: NativeRecipeFields): NativeRecipe {
  return {
    dependencies: structuredClone(fields.dependencies),
    setup: fields.setup.split(/\r?\n/).filter((line) => line.trim().length > 0),
  };
}
