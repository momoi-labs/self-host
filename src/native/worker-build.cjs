// Executed under the Application Account. Resolve Wrangler from its installed
// binary, never through the repository's node_modules or NODE_PATH.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const [binary, configPath, output] = process.argv.slice(1);
let packageRoot = path.dirname(fs.realpathSync(binary));
let wrangler;
for (let depth = 0; depth < 6 && !wrangler; depth++, packageRoot = path.dirname(packageRoot)) {
  // npm uses a symlink; mise's npm backend can instead create a shell shim
  // next to the package. Both resolve inside the installed tool directory.
  for (const candidate of [packageRoot, path.join(packageRoot, 'wrangler')]) {
    const manifest = path.join(candidate, 'package.json');
    if (!fs.existsSync(manifest)) continue;
    const pkg = JSON.parse(fs.readFileSync(manifest, 'utf8'));
    if (pkg.name === 'wrangler') {
      wrangler = require(path.join(candidate, pkg.main));
      break;
    }
  }
}
if (!wrangler?.unstable_readConfig) throw new Error('Installed Wrangler does not expose unstable_readConfig. Use the tested Wrangler version.');
const config = wrangler.unstable_readConfig({ config: configPath });
const supported = new Set([
  'configPath', 'userConfigPath', 'topLevelName', 'definedEnvironments',
  'name', 'main', 'compatibility_date', 'compatibility_flags', 'vars',
  'jsx_factory', 'jsx_fragment', 'rules', 'build', 'define', 'alias', 'tsconfig',
  'minify', 'no_bundle', 'find_additional_modules', 'base_dir',
  'dev', 'python_modules', 'account_id', 'workers_dev', 'preview_urls',
  'routes', 'route', 'upload_source_maps', 'keep_vars', 'observability',
  'assets',
]);
function populated(value) {
  if (Array.isArray(value)) return value.length > 0;
  if (value && typeof value === 'object') return Object.values(value).some(populated);
  return value !== undefined && value !== null && value !== false && value !== '';
}
for (const [key, value] of Object.entries(config)) {
  if (!supported.has(key) && populated(value)) {
    throw new Error(`Standalone Workers do not support Wrangler setting '${key}'. Cloudflare services and Pages assets are not available.`);
  }
}
const isSite = populated(config.assets);
if (isSite) {
  writeSite(config, configPath, output);
} else {
  writeModule(config, configPath, output);
}

function writeSite(config, configPath, output) {
  const assets = config.assets;
  if (typeof assets !== 'object' || assets === null || typeof assets.directory !== 'string' || !assets.directory) {
    throw new Error('Static sites require assets.directory in Wrangler config.');
  }
  if (config.main) {
    throw new Error('Workers with assets (a main module plus static assets) are not supported yet. Use a module Worker or a static site.');
  }
  const notFound = assets.not_found_handling || 'none';
  if (notFound !== 'none' && notFound !== 'single-page-application') {
    throw new Error(`Static sites support not_found_handling 'none' or 'single-page-application', not '${notFound}'.`);
  }
  const directory = path.resolve(path.dirname(configPath), assets.directory);
  if (!fs.statSync(directory, { throwIfNoEntry: false })?.isDirectory()) {
    throw new Error(`Static site assets directory '${assets.directory}' was not built. Check the build commands.`);
  }
  fs.writeFileSync(path.join(output, 'worker.json'), JSON.stringify({
    kind: 'site',
    directory,
    not_found_handling: notFound === 'single-page-application' ? 'spa' : 'none',
    compatibility_date: config.compatibility_date || '2025-01-01',
    compatibility_flags: config.compatibility_flags || [],
  }), { mode: 0o600 });
}

function writeModule(config, configPath, output) {
  if (!config.main || !config.compatibility_date) throw new Error('Workers require main and compatibility_date in Wrangler config.');
  for (const value of Object.values(config.vars || {})) {
    if (typeof value !== 'string') throw new Error('Standalone Workers currently support only string vars.');
  }
  const bundle = path.join(output, 'bundle.mjs');
  const meta = path.join(output, 'bundle-meta.json');
  const result = spawnSync(binary, [
    'deploy', '--dry-run', '--config', configPath, '--outdir', output,
    '--metafile', meta, '--no-autoconfig',
  ], { stdio: 'inherit', env: { ...process.env, CI: 'true', WRANGLER_SEND_METRICS: 'false' } });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status || 1);
  const metadata = JSON.parse(fs.readFileSync(meta, 'utf8'));
  // Wrangler's metafile retains its temporary output paths even with --outdir.
  const entries = Object.values(metadata.outputs).filter(output => output.entryPoint);
  if (entries.length !== 1 || !entries[0].exports?.includes('default')) throw new Error('Only ES module Workers with a default export are supported.');
  const files = fs.readdirSync(output);
  const modules = files.filter(file => /\.(mjs|js)$/.test(file));
  if (modules.length !== 1) throw new Error('Workers must bundle into one JavaScript module.');
  for (const file of files) {
    if (![modules[0], `${modules[0]}.map`, 'bundle-meta.json', 'README.md'].includes(file)) {
      throw new Error('Additional Worker modules and assets are not supported. Bundle JavaScript into one ES module.');
    }
  }
  // --outfile writes an upload multipart body, not JavaScript. Keep --outdir
  // and normalize only its single module's filename for the generated config.
  fs.renameSync(path.join(output, modules[0]), bundle);
  fs.writeFileSync(path.join(output, 'worker.json'), JSON.stringify({
    kind: 'module',
    compatibility_date: config.compatibility_date,
    compatibility_flags: config.compatibility_flags || [],
    vars: config.vars || {},
  }), { mode: 0o600 });
}
