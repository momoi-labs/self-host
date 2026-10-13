import assert from "node:assert/strict";
import { test } from "node:test";
import { variableAssignment } from "../src/lib/variableAssignment.ts";

test("a pasted assignment splits into name and value", () => {
  assert.deepEqual(variableAssignment("APP_URL=https://app.ficoucombinado.com.br"), { name: "APP_URL", value: "https://app.ficoucombinado.com.br" });
  assert.deepEqual(variableAssignment("export TOKEN='a=b c'\n"), { name: "TOKEN", value: "a=b c" });
  assert.deepEqual(variableAssignment('EMPTY=""'), { name: "EMPTY", value: "" });
});

test("text that is not one assignment is left alone", () => {
  assert.equal(variableAssignment("APP_URL"), null);
  assert.equal(variableAssignment("1BAD=x"), null);
  assert.equal(variableAssignment("A=1\nB=2"), null);
});

test("typing the equals sign ends the name", () => {
  assert.deepEqual(variableAssignment("X="), { name: "X", value: "" });
});
