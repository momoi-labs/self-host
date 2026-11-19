# Kiso in self-host

The console follows Kiso. This file records product decisions. The installed
contracts own the shared rules.

## Contracts

- Packages in `console/`: `@momoi-labs/kiso-react` 0.35.0 and
  `@momoi-labs/kiso` 1.12.1, resolved in `console/package-lock.json`.
- Contracts: `console/node_modules/@momoi-labs/kiso/kiso/`. Start at `AGENTS.md`.

## Paths

| What | Where |
| --- | --- |
| Screens | `console/src/views/` |
| Entry points | `console/src/main.tsx`, `console/src/login.tsx`, `console/src/setup.tsx` |
| Device setup | `console/src/device-setup.tsx` |
| Product components | `console/src/components/` |
| Product CSS | `console/src/console.css`, `console/src/views/events.css`, `console/src/components/git-workflow.css` |
| Appearance attributes | `console/index.html`, `console/login.html`, `console/setup.html` |

`console.css` imports `@momoi-labs/kiso-react/styles.css` once, before product
rules. All entry points use it.

## Product components

| Component | Purpose |
| --- | --- |
| `Shell` | The console's `ConsoleShell`: navigation, breadcrumb, the health status with the version, and the account menu |
| `BrandSymbol`, `Lockup` | The self-host brand: the lockup alone in the sidebar bar (`shellLayout`), the symbol as the rail's `brandMark` and in the phone header, and the lockup with the origin in the login brand row. The sidebar bar, its stripe and the brand inset are Kiso's `shellLayout`. The lockup's torii is the rail mark's size (`--size-control-sm`), so the torii does not move or resize on collapse. One drawing per theme. The console always shows the terminal prompt; only the favicon files are the torii alone |
| `AppForm`, `NativeAppForm`, `PostgresFields` | Application and database configuration |
| `CustomImageEditor`, `EnvironmentEditor` | Image and virtual machine configuration |
| `Glance` | Detail-page metrics |
| `LastRun`, `AppLogPane`, `LogSurface` | Run progress and output |
| `Failure`, `Causes` | Backend error reports |
| `RouteTable`, `RouteDialog`, `RedirectDialog` | Routes and redirects |
| `DefinitionPicker`, `definitionOf` | Application definition choice, with native runtime taking precedence over Git source |
| `GitSourceFields`, `GitConnectionSelector`, `GitRepositoryPicker` | Repository URL, ref, pin and saved repository access |
| `GitWorkflowFields`, `GitUpdateDialog`, `GitBuildRun` | Container Git setup, reviewed updates and build task output |
| `Dependencies`, `ShellEditor`, `Steps`, `Step` | Runtime tool versions, shell commands and recipe sections |
| `NativeVariables` | Saved Application Variables, separate from configuration submission |
| `AppSummary`, `GroupCard`, `ProcessCard` | Application facts and running processes |
| `LogPane`, `Logs`, `useToast` | Logs and action feedback |
| `PrivateAccessField`, `DatabaseConnections` | Application access grants and managed database connections |
| `DatabaseBackups` | A managed database server's backups: on or off, days to keep, the repository's backups, restore to a new server and verification |
| `RestoreDialog` | Restores a server's backups into a new server. The Backups tab opens it for one server, and the Databases list lets the Operator pick the source |
| `barSteps`, `phase` in `console/src/lib/runSteps.ts` | Task progress and phase labels |

`EnvironmentEditor` is exported from `console/src/views/Environments.tsx`.
Other components above live in `console/src/components/`.

## Product rules

- Domain names follow `CONTEXT.md`. Repository documentation is English.
- Application source and runtime are separate. A Git-backed native Application
  uses the native form and the existing Git update review workflow.
- Settings > General > Applications edits Host native build limits with explicit
  Save and Discard controls, using MiB for source and payload sizes.
- Native Git build commands run for each candidate checkout. Setup commands
  belong to the Application Account recipe, not the Git build.
- Workers and static sites are native Git Applications. Show the runtime tools
  in `Dependencies`; require Web publication and a loopback port. A config with
  `main` supports JS/TS module Workers; a config with `assets` serves a static
  site with `single-page-application` fallback. Cloudflare storage bindings are
  not supported.
- The real console is the bundle embedded by `cargo run -- serve`. A Vite
  preview or prototype does not verify backend behavior.
- Keep the console's list, detail and create compositions from
  `skills/add-page/SKILL.md` and ADR-0024.
- Use the same appearance attributes on all three HTML entry points, except
  the header variant noted below.
  `console/tests/appearance.test.mjs` records the selected values.
- Default to light. A saved light, dark or system selection takes precedence
  before first paint. System removes `data-theme` and follows the OS.
- Use the tangerine accent, the warm neutral, the accent as primary, Inter,
  default presentation, the inset shell with automatic outer chrome and a
  full brand bar, a default-size header set to the page width, full page
  width and start alignment. The console uses the default header variant.
  Login and setup keep the floating header, because they have no sidebar.
  The default header size gives the bar sewa's height.
- The DNS suffix is the breadcrumb root and links to the console
  home. Without a suffix the root reads "Console". Below 700px the
  breadcrumb shows only the current page. The lockup stands alone, with no
  suffix under it.
- Use solid pixel frames, small corners, no corner marks, no shadows,
  solid panels, landing bands inside the frame and a quiet construction
  background inside and outside the frame.
- Use straight chart lines, no chart fill, the standard chart layout and
  table legends. Keep existing product-specific chart legend visibility.
- The shell is `ConsoleShell`: a collapsible sidebar with the toggle in the
  header, before the breadcrumb. From 700px to 1023px the navigation opens
  in a drawer. Below 700px it moves to ConsoleShell's bottom navigation: the
  first four destinations, then More for the rest.
- The sidebar lists pages, not records. Applications and virtual machines open
  from the Overview's tables, and their detail screens mark Overview as the
  current destination. A database's screen marks Databases.
- Settings is the last navigation group. The header ends with ConsoleShell's
  `status`, the health line and the Host version, then the account button.
  Its `Popover` holds the identity, `ThemeSelector` and Sign out. When a
  workload is down, the status links to it, and Overview in the navigation
  shows a warning dot.
- Use Kiso's `Card` paint layer for shaped shadows. A table that is its own
  panel sits in `TableFrame`, never in a hand-written `div.table-wrap`. Other native frames
  need `PanelShadow` as their first child, as documented in the installed
  tokens.
- A list page is a `PageHeader` with a title, a one-line description and the
  primary action, then the filters, then the table. The table and its headers
  always render. Loading is `Skeleton` rows. An empty list is an `EmptyState`
  in the table body, with a title and one sentence, no icon and no button.
  The header holds the action. Overview's group tables follow the same rule.
  Only
  Overview's empty Host is a standalone `EmptyState` in a `Card`.
- No disabled header action. Hide an action the Operator cannot take, and let
  the empty state say why. Routes hides Add route until an Application exists.
- A list's `TableFrame` fits its rows. `data-fill="false"` opts out of Kiso's
  stretch for a last-child table, so Databases and Events do not fill the
  window.
- Settings has one `Card` per group, and no frame inside it. A table in a
  group uses `TableFrame frame="none"` and sits flush with the card edges,
  with no search or count. Kiso clips a form footer only when the form is the
  card's last child. A `GridPane` body is not, so `console.css` applies the
  same `--pixel-footer-clip` to the body when a form or a flush table ends it.
  Kiso has no rule for this case, so it is product CSS.
- A detail screen's `.detail-tabs` card is the only panel that casts a
  shadow. Its `Tabs` take `data-shadow-style="none"`, a nested appearance
  scope, so the panels, diagram nodes and log views in a tab keep their frame
  and drop the cast.
- A table that would scroll sideways drops its secondary columns below a
  container width and shows them in the row, as `data-interfaces.md` orders.
  Events hides Actor and Last updated below 56rem and lists them under the
  action. Kiso has no column priority, so this is product CSS.
- Kiso suggests a no-results `EmptyState` with Clear filters. The console
  keeps a plain "No X match your filters." row, per `skills/add-page/SKILL.md`.

## Pattern choices

These map the existing screens. They do not certify a full contract audit.

| Screen | Pattern |
| --- | --- |
| Console chrome | `application-shell`, `page-layout` |
| Overview | `dashboard` |
| Application and managed PostgreSQL detail | `crud`, with ADR-0024 detail layout |
| New application and database | `crud`, with ADR-0024 create layout |
| Databases, DNS, routes, events and custom image lists | `crud`, with ADR-0024 list layout |
| Custom image editor | `crud`, with ADR-0024 create/detail layouts |
| Virtual machine create/detail | `crud`, `guided-flow`, with ADR-0024 layouts |
| Settings: general, Host, DNS, certificates, API keys, scheduled tasks, backup storage, maintenance window and Git connections | `settings` |
| Login | `login-authentication`, plus `skills/add-page/login.md` |
| Device setup | `onboarding`; outside the authenticated shell |
| Host and standalone remote screen | No full-screen pattern; product-specific terminal and remote screen |

## Exceptions

None recorded. Existing contract gaps are not approved exceptions.

## Verify

1. Install locked dependencies: `npm ci --prefix console`.
2. Run `npm test --prefix console` and `npm run typecheck --prefix console`.
3. Build the embedded bundle: `npm run build --prefix console`.
4. From the repository root, run `cargo run -- serve`. For an isolated check,
   use a separate `SELF_HOST_DATA_DIR`, a free `--port`, `--no-dns` and a
   nonexistent `DOCKER_HOST` socket so production workloads stay untouched.
5. Open `/console`, sign in and check `/console/` and
   `/console/setup.html`. Check fresh storage, saved themes, desktop and 390px.
6. Create an Application with the Native process definition. Check both
   Application home and Git repository sources. For Git, import a connected
   repository or enter a URL, then set the ref, context and build commands.
7. Select Worker or static site in the native process form. Confirm the tool
   versions appear, the config path is editable, and a hostname and loopback
   port are required. Deploy a module Worker and a static site, then check each
   hostname.
8. Open the saved Application's Configuration tab. Save an edit, discard an
   edit, rebuild the deployed commit, and use Update to review newer commits.
   Confirm native Applications have no Docker deployment history.

Report automated checks separately from real-console checks. Keyboard, theme,
390px layout and backend checks require the running console; do not infer them
from a successful build.
