import assert from "node:assert/strict";
import { test } from "node:test";
import { commands, recipeBody } from "../src/lib/devImageRecipe.ts";

const fields = {
  name: "  web-dev  ",
  templateId: "",
  dependencies: [{ tool: "node", version: "24" }],
  setup: "curl -fsSL https://example.test/install.sh | bash\n\n  uv tool install ruff  \n",
  buildChecks: "node --version\n",
  dockerfile: null,
};

test("sends the builder's fields as the Host reads them", () => {
  assert.deepEqual(recipeBody(fields), {
    name: "web-dev",
    template_id: null,
    dependencies: [{ tool: "node", version: "24" }],
    setup: ["curl -fsSL https://example.test/install.sh | bash", "uv tool install ruff"],
    build_checks: ["node --version"],
  });
});

test("an edited Dockerfile empties the fields it replaces", () => {
  assert.deepEqual(recipeBody({ ...fields, templateId: "t3-code", dockerfile: "FROM debian:13-slim\n" }), {
    name: "web-dev",
    template_id: "t3-code",
    dockerfile: "FROM debian:13-slim\n",
    dependencies: [],
    setup: [],
    build_checks: [],
  });
});

test("reads one command per line and nothing from a blank one", () => {
  assert.deepEqual(commands("\n \n\t\n"), []);
  assert.deepEqual(commands("true\nfalse"), ["true", "false"]);
});
