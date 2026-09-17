export type ImageDependency = { tool: string; version: string; allow_builds?: string[] };

export type CustomImageTemplate = {
  id: string;
  name: string;
  imageName: string;
  dependencies: ImageDependency[];
  setup: string[];
  buildChecks: string[];
  application: { command: string; web_port: number; persist_data: boolean };
};

export const customImageTemplates: CustomImageTemplate[] = [{
  id: "t3-code",
  name: "T3 Code",
  imageName: "t3-code",
  dependencies: [
    { tool: "node", version: "24" },
    { tool: "npm:t3", version: "latest", allow_builds: ["node-pty"] },
    { tool: "claude-code", version: "latest" },
    { tool: "npm:@openai/codex", version: "latest" },
  ],
  setup: [],
  buildChecks: [
    "node --version",
    "t3 --help",
    "claude --version",
    "codex --version",
    // Resolve the way the t3 launcher does: through the platform package that
    // owns node-pty, by real path so mise's symlinks do not get in the way.
    `node -e 'const {realpathSync}=require("node:fs"); const {createRequire}=require("node:module"); const t3=createRequire(realpathSync(process.argv[1]+"/node_modules/t3/package.json")); const r=createRequire(realpathSync(t3.resolve("@t3code/t3-"+process.platform+"-"+process.arch+"/package.json"))); r("node-pty").spawn("/bin/true",[],{name:"xterm",cols:80,rows:24}).onExit(e=>process.exit(e.exitCode))' "$(mise where npm:t3)"`,
  ],
  application: {
    command: "t3 serve --host 0.0.0.0 --port 3000 --base-dir /data/t3home",
    web_port: 3000,
    persist_data: true,
  },
}];

export function customImageTemplate(id?: string | null) {
  return customImageTemplates.find((template) => template.id === id);
}
