import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Card } from "@momoi-labs/kiso-react";
import { applyTheme, currentTheme } from "../src/lib/theme.ts";

const appearance = {
  lang: "en",
  "data-theme": "light",
  "data-accent": "tangerine",
  "data-neutral": "warm",
  "data-primary": "accent",
  "data-typeface": "inter",
  "data-chart-line": "stepped",
  "data-chart-fill": "squares",
  "data-chart-layout": "standard",
  "data-chart-legend": "inline",
  "data-shadow-style": "hard",
  "data-border-style": "solid",
  "data-corner-style": "pixel",
  "data-corner-marks": "none",
  "data-corner-size": "small",
  "data-mark-size": "medium",
  "data-visual-style": "default",
  "data-app-shell": "inset",
  "data-header-variant": "floating",
  "data-header-size": "compact",
  "data-header-width": "outset",
  "data-app-shell-outer": "none",
  "data-page-width": "full",
  "data-page-align": "center",
  "data-frame-scope": "all",
  "data-mark-scope": "panels",
  "data-mark-clearance": "normal",
  "data-background-style": "construction",
  "data-background-strength": "quiet",
  "data-background-placement": "both",
  "data-frame-detail": "medium",
  "data-paper-tone": "theme",
  "data-panel-fill": "translucent",
  "data-landing-bands": "full",
  "data-outer-border-style": "inherit",
  "data-outer-corner-style": "inherit",
  "data-outer-corner-marks": "inherit",
  "data-outer-shadow-style": "inherit",
};

for (const entry of ["index", "login", "setup"]) {
  const html = readFileSync(new URL(`../${entry}.html`, import.meta.url), "utf8");
  test(`${entry} uses the product appearance`, () => {
    const attributes = Object.fromEntries(
      [...html.match(/<html\b[^>]*>/)[0].matchAll(/([\w-]+)="([^"]*)"/g)]
        .map(([, name, value]) => [name, value]),
    );
    assert.deepEqual(attributes, appearance);
  });

  test(`${entry} applies the saved theme before first paint`, () => {
    const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
    for (const stored of [null, "invalid", "light", "dark", "system", "blocked"]) {
      const document = { documentElement: { dataset: { theme: "light" } } };
      runInNewContext(script, {
        document,
        localStorage: { getItem() {
          if (stored === "blocked") throw new Error("Storage unavailable");
          return stored;
        } },
      });
      const expected = stored === "system" ? undefined : stored === "dark" ? "dark" : "light";
      assert.equal(document.documentElement.dataset.theme, expected, stored);
    }
  });
}

test("theme selection agrees with the first-paint default and keeps system support", (t) => {
  const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  t.after(() => {
    for (const [name, descriptor] of [["localStorage", storageDescriptor], ["document", documentDescriptor]]) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  });
  Object.defineProperty(globalThis, "document", {
    configurable: true, value: { documentElement: { dataset: {} } },
  });
  for (const stored of [null, "invalid", "light", "dark", "system", "blocked"]) {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true, value: { getItem() {
        if (stored === "blocked") throw new Error("Storage unavailable");
        return stored;
      } },
    });
    const expected = ["light", "dark", "system"].includes(stored) ? stored : "light";
    assert.equal(currentTheme(), expected);
    applyTheme(currentTheme());
    assert.equal(document.documentElement.dataset.theme, expected === "system" ? undefined : expected);
  }
});

test("login inherits the product background instead of overriding it", () => {
  const source = readFileSync(new URL("../src/login.tsx", import.meta.url), "utf8");
  const main = source.match(/<main\b[^>]*>/)[0];
  assert.doesNotMatch(main, /data-background-/);
});

test("Kiso cards include the shaped shadow paint layer", () => {
  const markup = renderToStaticMarkup(createElement(Card, null, "Panel content"));
  assert.match(markup, /<span class="panel-shadow" aria-hidden="true"><span><span><\/span><\/span><\/span>/);
  assert.match(markup, /Panel content/);
});
