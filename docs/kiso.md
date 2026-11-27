# Kiso in self-host

The console follows Kiso. This file records product decisions. The installed
contracts own the shared rules.

## Contracts

- Packages in `console/`: `@momoi-labs/kiso-react` 0.37.0 and
  `@momoi-labs/kiso` 1.15.0, resolved in `console/package-lock.json`.
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
- `Failure` is a tinted error Alert by default. Pass `incident` only for a
  blocking incident at the top of a detail screen: the last failed deploy,
  database operation, image build or virtual machine operation. It then
  renders Kiso's loud Alert with the animated top tape. The header holds
  `AlertLabel`, a Details toggle and the recovery action. The causes read
  bottom-up as a Git graph: the root cause on a solid trunk, the steps it was
  triggered while on a dashed branch in mono, and the result where the branch
  merges back. Closed, the Alert shows the root cause and a faded peek of the
  next row. A report with no causes shows the error as the single Cause, with
  no graph and no Details. Show at most one loud Alert per view. Form, dialog
  and action errors stay tinted.
- The loud Alert means the workload is down. Its label says so: `Application
  is down` or `Database is down`. When the workload still runs and only its
  last deploy or operation failed, pass `blocking={false}`. `Failure` then
  keeps the label and the graph in a tinted Alert, with no tape.
- A status badge starts with a capital: `Running`, `Failed`, `Stopped`,
  `Pending`. Use `statusLabel` for a status the backend sends in lowercase.
- An Application that does not run shows its status only. `HttpStatus` and
  `DeploymentReadiness` join the `Lifecycle` cluster when it runs.
- A detail screen's header row has no editorial block padding and centers
  the title with the `Lifecycle` cluster. A loud Alert right after it sits
  closer to the header than to the content below.
- The real console is the bundle embedded by `cargo run -- serve`. A Vite
  preview or prototype does not verify backend behavior.
- Keep the console's list, detail and create compositions from
  `skills/add-page/SKILL.md` and ADR-0024.
- Use the same appearance attributes on all three HTML entry points, except
  the header variant noted below.
  `console/tests/appearance.test.mjs` records the selected values.
- Default to light. A saved light, dark or system selection takes precedence
  before first paint. System removes `data-theme` and follows the OS.
- Use the stone accent, the warm neutral, the accent as primary, Inter,
  sans technical text, flat controls, continuous progress, no panel labels,
  the default presentation, the default shell with automatic outer chrome, a
  separate brand bar, a default-size header, full page width and start
  alignment. The console uses the default header variant. Login and setup
  keep the floating header, because they have no sidebar.
- The DNS suffix is the breadcrumb root and links to the console
  home. Without a suffix the root reads "Console". Below 700px the
  breadcrumb shows only the current page. The lockup stands alone, with no
  suffix under it.
- Use solid square frames, medium corners, corner ticks on panels, a soft
  shadow on the outermost panel only (`data-shadow-scope="first"`), solid
  panels and a quiet solid background. A panel inside another panel never
  casts a shadow. Diagrams inherit the console's frame, corners and marks.
- Use the series chart palette, straight chart lines, no chart fill, the standard chart layout and
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
- A list page is Kiso's `ListPage`: the title and the primary action, then
  the filters, then the table, with the count in its footer. Loading is
  `Skeleton`. An empty list is the header and one `Card` with the
  `EmptyState`, edge to edge: no filters, no table, no count. The
  `EmptyState` has a title and one sentence, no icon and no button. A filter
  that empties a non-empty list keeps the table and shows a "No X match your
  filters." row. Events has no header action and ListPage requires one, so
  Events writes the same markup by hand.
- An `EmptyState` touches its frame on all four sides. In an Overview group
  or a Settings pane, the group keeps only its title row and the
  `EmptyState` in a `div.empty-frame`: no search, no table header. The frame
  draws the divider under the title, so the stripes start at the title row.
  In a tab with other content, it sits alone in a `Card`, where the table
  would be.
- Below 560px an Overview group's title row holds the title and a "New"
  button, whose accessible name names the object ("New database"). The
  search takes its own row below. From 560px up the button keeps its full
  label and the search sits on the title row.
- A screen header is the title and at most one action. No
  `PageHeaderDescription` on list, create, Host, Events or Settings screens,
  so every title sits where the Kiso CRUD puts it. When all of Overview is
  empty, Create sits in the header and the card holds only the
  `EmptyState`.
- No disabled header action. Hide an action the Operator cannot take, and let
  the empty state say why. Routes always offers Add route, because an
  External app needs no Application.
- A list's `TableFrame` fits its rows. `data-fill="false"` opts out of Kiso's
  stretch for a last-child table, so Databases and Events do not fill the
  window.
- Settings has four sections: General, Access, Network, and Backups and
  tasks. One section shows at a time, as one column of `Card`s, one per
  setting. A sub-nav of Kiso `Navigation` links sits beside the column and
  lists the open section's settings. Below a 52rem content width a `Select`
  replaces it. `#settings/<section>/<setting>` opens a section and scrolls to
  the card. A section with a failing setting carries a danger `Badge` count,
  and Settings in the sidebar carries the total. Every section stays mounted,
  so an unsaved form survives a section switch.
- A table has one frame, like Kiso's CRUD Products list. A table that is
  its own panel keeps its `TableFrame`. A table inside a `Card`, a detail
  tab or another panel uses `TableFrame frame="none"`. A setting card has
  no frame inside it, and its table sits flush with the card edges, with no
  search or count. Kiso clips a form footer only when the form is the card's
  last child. A card body is not, so `console.css` applies the same
  `--pixel-footer-clip` to the body when a form, a flush table or an empty
  frame ends it.
  Kiso has no rule for this case, so it is product CSS.
- Stack sibling panels in `.panel-stack`, not `.stack`. Kiso's `.stack` gap
  is a fixed 16px. A `.panel-stack` uses `--presentation-grid-gap`, as Kiso asks of custom page
  compositions. Settings sections use the same gap.
- The `.detail-tabs` card is the outermost panel and casts the shadow. The
  panels, diagram nodes and log views in a tab keep their frame and cast
  none.
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
| Settings: General, Access, Network, and Backups and tasks sections | `settings` |
| Settings, Access, Passkeys | `settings`; adding a passkey or creating a Person or `host` key confirms with a passkey from the card's Dialog |
| Login: passkey login, first passkey setup, CLI key step and the non-canonical name message | `login-authentication`, plus `skills/add-page/login.md` |
| Device setup | `onboarding`; outside the authenticated shell |
| Host and standalone remote screen | No full-screen pattern; product-specific terminal and remote screen |

## Exceptions

| Where | Kiso rule | Console rule | Why |
| --- | --- | --- | --- |
| `.list-filters` below 560px, `console.css` | The search and the select each take a full row. | The search and the select share one 36px row. Clear filters wraps to a second row. | The Operator compares every list with the Kiso CRUD list, whose filter row is one line at 390px. |

Existing contract gaps are not approved exceptions.

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
