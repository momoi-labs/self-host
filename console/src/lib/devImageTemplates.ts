export type ImageDependency = { tool: string; version: string; allow_builds?: string[] };

export type DevImageTemplate = {
  id: string;
  name: string;
  imageName: string;
  dependencies: ImageDependency[];
  buildChecks: string[];
  application: { command: string; web_port: number; persist_data: boolean };
};

export const devImageTemplates: DevImageTemplate[] = [{
  id: "t3-code",
  name: "T3 Code",
  imageName: "t3-code",
  dependencies: [
    { tool: "node", version: "24" },
    { tool: "npm:t3", version: "latest", allow_builds: ["node-pty"] },
    { tool: "claude-code", version: "latest" },
    { tool: "npm:@openai/codex", version: "latest" },
  ],
  buildChecks: [
    "node --version",
    "t3 --help",
    "claude --version",
    "codex --version",
    // Resolve through T3's real package directory, including mise's symlinks.
    `node -e 'const {realpathSync}=require("node:fs"); const {createRequire}=require("node:module"); const r=createRequire(realpathSync(process.argv[1]+"/node_modules/t3/package.json")); r("node-pty").spawn("/bin/true",[],{name:"xterm",cols:80,rows:24}).onExit(e=>process.exit(e.exitCode))' "$(mise where npm:t3)"`,
  ],
  application: {
    command: "t3 serve --host 0.0.0.0 --port 3000 --base-dir /data/t3home",
    web_port: 3000,
    persist_data: true,
  },
}];

export function devImageTemplate(id?: string | null) {
  return devImageTemplates.find((template) => template.id === id);
}
