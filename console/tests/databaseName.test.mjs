import assert from "node:assert/strict";
import { test } from "node:test";
import { defaultDatabaseName, validDatabaseName } from "../src/lib/databaseName.ts";

test("a new connection's database is named after its Application", () => {
  assert.equal(defaultDatabaseName("mochi-cut"), "mochi_cut");
  assert.equal(defaultDatabaseName("Mochi Cut 2"), "mochi_cut_2");
  assert.equal(defaultDatabaseName("2048"), "db_2048");
  assert.equal(defaultDatabaseName("postgres"), "db_postgres");
  assert.equal(defaultDatabaseName("pg-admin"), "db_pg_admin");
  assert.equal(defaultDatabaseName("x".repeat(80)).length, 63);
  for (const name of ["mochi-cut", "2048", "postgres", "pg-admin", "---", "x".repeat(80)]) {
    assert.ok(validDatabaseName(defaultDatabaseName(name)), name);
  }
});

test("a database name is a plain lowercase identifier", () => {
  assert.ok(validDatabaseName("mochi_cut"));
  for (const name of ["", "Mochi", "mochi-cut", "1st", "pg_mine", "postgres", "template1", "a\"b", "x".repeat(64)]) {
    assert.equal(validDatabaseName(name), false, name);
  }
});
