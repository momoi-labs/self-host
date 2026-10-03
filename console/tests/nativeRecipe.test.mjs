import assert from "node:assert/strict";
import { test } from "node:test";
import { nativeCommand, nativeCommandFields } from "../src/lib/nativeCommand.ts";
import { nativeRecipe, nativeRecipeFields } from "../src/lib/nativeRecipe.ts";

test("native records without a recipe keep an empty environment definition", () => {
  const fields = nativeRecipeFields();
  assert.deepEqual(fields, { dependencies: [], setup: "" });
  assert.deepEqual(nativeRecipe(fields), { dependencies: [], setup: [] });
});

test("mise versions, build permissions and setup round-trip without changing argv", () => {
  const recipe = {
    dependencies: [
      { tool: "node", version: "24" },
      { tool: "npm:example-tool", version: "1.2.3", allow_builds: ["example-build"] },
    ],
    setup: ["mkdir -p app", "printf '%s' 'quoted value' > app/config", "printf trailing\\ "],
  };
  const command = ["node", "--eval", "console.log('one')\nconsole.log('two')", "", "  spaced  "];
  const fields = { ...nativeCommandFields(command), ...nativeRecipeFields(recipe) };
  assert.deepEqual(nativeRecipe(fields), recipe);
  assert.deepEqual(nativeCommand(fields), command);
});

test("editing and discarding a recipe preserves saved dependency options", () => {
  const recipe = {
    dependencies: [{ tool: "npm:example-tool", version: "1", allow_builds: ["example-build"] }],
    setup: ["mkdir -p app"],
  };
  const saved = nativeRecipeFields(recipe);
  const fields = nativeRecipeFields(recipe);
  fields.dependencies[0].version = "2";
  fields.dependencies[0].allow_builds.push("another-build");
  fields.setup = "mkdir -p other";
  assert.notEqual(JSON.stringify(fields), JSON.stringify(saved));
  assert.deepEqual(nativeRecipeFields(recipe), saved);
  assert.deepEqual(nativeRecipe(nativeRecipeFields(recipe)), recipe);
});

test("setup omits blank lines, normalizes line endings and preserves shell whitespace", () => {
  assert.deepEqual(nativeRecipe({ dependencies: [], setup: "\r\n  \n  printf ready  \r\nprintf trailing\\ \n" }).setup,
    ["  printf ready  ", "printf trailing\\ "]);
  assert.deepEqual(nativeRecipe({ dependencies: [], setup: "" }), { dependencies: [], setup: [] });
});
