// PROTOTYPE. Throwaway state for the development image editor variants.
// Nothing here talks to the Host: the recipe lives in memory and the build
// is a timer that prints plausible lines.

import { useCallback, useRef, useState } from "react";

export type Dependency = { tool: string; version: string };

export type Recipe = {
  name: string;
  templateId: string;
  dependencies: Dependency[];
  setup: string;
  buildChecks: string;
};

export type Section = "image" | "dependencies" | "setup" | "checks";
export type Source = "builder" | "dockerfile";
export type Status = "idle" | "building" | "ready" | "failed";

export type Block = { section: Section | "base" | "runtime"; text: string };

export const templates = [
  {
    id: "t3-code",
    name: "T3 Code",
    imageName: "t3-code",
    dependencies: [
      { tool: "node", version: "24" },
      { tool: "npm:t3", version: "latest" },
      { tool: "claude-code", version: "latest" },
    ],
    setup: "",
    buildChecks: "node --version\nt3 --help\nclaude --version",
  },
  {
    id: "hermes",
    name: "Hermes agent",
    imageName: "hermes",
    dependencies: [{ tool: "python", version: "3.13" }, { tool: "uv", version: "latest" }],
    setup: "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash",
    buildChecks: "hermes --version",
  },
];

export const emptyRecipe: Recipe = {
  name: "",
  templateId: "",
  dependencies: [],
  setup: "",
  buildChecks: "",
};

export const sectionTitles: Record<Section, string> = {
  image: "Image",
  dependencies: "Dependencies",
  setup: "Setup",
  checks: "Build checks",
};

export const sectionHints: Record<Section, string> = {
  image: "Debian 13 with mise and Git. The name becomes the local tag.",
  dependencies: "Installed by mise, as root, with network.",
  setup: "One shell command per line. Runs as root, with network, after dependencies. Optional.",
  checks: "One command per line. Runs as dev, without network, after setup. Any failure stops the build. Optional.",
};

export function lines(text: string): string[] {
  return text.split("\n").map((line) => line.trim()).filter(Boolean);
}

/** The Dockerfile the Host would write for this recipe, as labelled blocks. */
export function dockerfileBlocks(recipe: Recipe): Block[] {
  const tools = recipe.dependencies.map((dep) => `${JSON.stringify(dep.tool)} = ${JSON.stringify(dep.version)}`);
  const checks = lines(recipe.buildChecks);
  const setup = lines(recipe.setup);
  return [
    {
      section: "base",
      text: [
        "FROM debian:13-slim",
        "RUN apt-get update && apt-get install -y --no-install-recommends \\",
        "    ca-certificates curl git gosu procps build-essential python3 unzip xz-utils \\",
        "    && rm -rf /var/lib/apt/lists/*",
        "ARG USERNAME=dev",
        "ARG USER_UID=1000",
        "RUN useradd --uid \"${USER_UID}\" --user-group --no-create-home \\",
        "    --home-dir /data/home --shell /bin/bash \"${USERNAME}\"",
        "ENV MISE_DATA_DIR=/opt/mise MISE_CONFIG_DIR=/opt/mise/config HOME=/data/home \\",
        "    PATH=/opt/mise/shims:/usr/local/bin:/usr/bin:/bin",
        "RUN curl -fsSL https://mise.run | sh",
      ].join("\n"),
    },
    {
      section: "image",
      text: `LABEL org.momoi.self-host.image=${JSON.stringify(recipe.name || "unnamed")}`,
    },
    {
      section: "dependencies",
      text: [
        "COPY <<EOF /opt/mise/config/config.toml",
        "[tools]",
        ...tools,
        ...(checks.length ? ["", "[tasks.check]", `run = ${JSON.stringify(checks)}`] : []),
        "EOF",
        "RUN mise install --yes && mise reshim \\",
        "    && chown -R \"${USERNAME}:${USERNAME}\" /opt/mise",
      ].join("\n"),
    },
    {
      section: "setup",
      text: setup.length
        ? ["# setup: runs as root, with network", ...setup.map((command) => `RUN ${command}`),
          "RUN chown -R \"${USERNAME}:${USERNAME}\" /opt/mise"].join("\n")
        : "# setup: none",
    },
    {
      section: "checks",
      text: checks.length
        ? [
          "COPY build-checks.py /usr/local/lib/self-host-build-checks.py",
          "RUN --mount=type=tmpfs,target=/data --network=none \\",
          "    self-host-development-image-entrypoint python3 /usr/local/lib/self-host-build-checks.py",
        ].join("\n")
        : "# build checks: none",
    },
    {
      section: "runtime",
      text: [
        "COPY runtime-entrypoint.sh /usr/local/bin/self-host-development-image-entrypoint",
        "WORKDIR /data/repos",
        "ENTRYPOINT [\"/usr/local/bin/self-host-development-image-entrypoint\"]",
        "CMD [\"sleep\", \"infinity\"]",
      ].join("\n"),
    },
  ];
}

export function dockerfileText(recipe: Recipe): string {
  return dockerfileBlocks(recipe).map((block) => block.text).join("\n\n") + "\n";
}

function buildScript(recipe: Recipe, source: Source, dockerfile: string): string[] {
  const out: string[] = [
    "#1 [internal] load build definition from Dockerfile",
    `#1 transferring dockerfile: ${dockerfile.length}B done`,
    "#2 [internal] load metadata for docker.io/library/debian:13-slim",
    "#2 DONE 0.6s",
    "#3 [1/8] FROM docker.io/library/debian:13-slim",
    "#3 CACHED",
    "#4 [2/8] RUN apt-get update && apt-get install -y --no-install-recommends ...",
    "#4 CACHED",
  ];
  if (source === "builder") {
    out.push("#5 [3/8] COPY <<EOF /opt/mise/config/config.toml", "#5 DONE 0.0s", "#6 [4/8] RUN mise install --yes && mise reshim");
    for (const dep of recipe.dependencies) {
      out.push(`#6 0.412 mise ${dep.tool}@${dep.version} ⏳ installing`, `#6 3.108 mise ${dep.tool}@${dep.version} ✓ installed`);
    }
    out.push("#6 DONE 4.2s");
    for (const [index, command] of lines(recipe.setup).entries()) {
      out.push(`#7 [5/8] RUN ${command}`);
      if (/\bfail\b|exit 1/.test(command)) {
        out.push(`#7 0.031 bash: line 1: ${command.split(" ")[0]}: command not found`, `#7 ERROR: process "/bin/sh -c ${command}" did not complete successfully: exit code: 1`, "", `Dockerfile:${20 + index}`, `--------------------`, `  RUN ${command}`, `--------------------`, "ERROR: failed to solve: exit code 1");
        return out;
      }
      out.push(`#7 0.502 ...`, `#7 DONE 1.9s`);
    }
    for (const command of lines(recipe.buildChecks)) {
      out.push(`#8 [6/8] check: ${command}`, `#8 0.210 ok`);
    }
  } else {
    let step = 5;
    for (const line of dockerfile.split("\n")) {
      if (!/^(RUN|COPY|WORKDIR|ENV|LABEL)\b/.test(line)) continue;
      out.push(`#${step} [${step - 2}/8] ${line.slice(0, 90)}`);
      if (/\bfail\b|exit 1/.test(line)) {
        out.push(`#${step} ERROR: process did not complete successfully: exit code: 1`, "ERROR: failed to solve: exit code 1");
        return out;
      }
      out.push(`#${step} DONE 0.8s`);
      step += 1;
    }
  }
  out.push("#9 exporting to image", "#9 exporting layers done", `#9 naming to docker.io/library/self-host-dev/${recipe.name || "unnamed"}:a1b2c3d done`, "#9 DONE 1.1s", "", `Image ${recipe.name || "unnamed"} is ready.`);
  return out;
}

export type Prototype = ReturnType<typeof usePrototype>;

export function usePrototype() {
  const [recipe, setRecipeState] = useState<Recipe>(emptyRecipe);
  const [source, setSource] = useState<Source>("builder");
  const [dockerfile, setDockerfile] = useState("");
  const [status, setStatus] = useState<Status>("idle");
  const [log, setLog] = useState<string[]>([]);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  const setRecipe = useCallback((patch: Partial<Recipe>) => {
    setRecipeState((current) => ({ ...current, ...patch }));
  }, []);

  const chooseTemplate = useCallback((id: string) => {
    const template = templates.find((entry) => entry.id === id);
    setRecipeState((current) => ({
      name: current.name || template?.imageName || "",
      templateId: id,
      dependencies: structuredClone(template?.dependencies ?? []),
      setup: template?.setup ?? "",
      buildChecks: template?.buildChecks ?? "",
    }));
  }, []);

  /** Leaving the builder copies its Dockerfile so the paste starts full. */
  const changeSource = useCallback((next: Source) => {
    setSource(next);
    if (next === "dockerfile") setDockerfile((current) => current || dockerfileText(recipe));
  }, [recipe]);

  const build = useCallback(() => {
    if (timer.current) clearInterval(timer.current);
    const file = source === "builder" ? dockerfileText(recipe) : dockerfile;
    const script = buildScript(recipe, source, file);
    const failed = script.at(-1)?.startsWith("ERROR") ?? false;
    setLog([]);
    setStatus("building");
    let index = 0;
    timer.current = setInterval(() => {
      const line = script[index] ?? "";
      setLog((current) => [...current, line]);
      index += 1;
      if (index >= script.length) {
        if (timer.current) clearInterval(timer.current);
        timer.current = null;
        setStatus(failed ? "failed" : "ready");
      }
    }, 160);
  }, [recipe, source, dockerfile]);

  const generated = dockerfileText(recipe);
  const canBuild = status !== "building" && (source === "dockerfile" ? dockerfile.trim().length > 0 : recipe.dependencies.length > 0);

  return { recipe, setRecipe, chooseTemplate, source, changeSource, dockerfile, setDockerfile, generated, status, log, build, canBuild };
}
