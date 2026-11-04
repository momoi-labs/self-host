# Kiso in self-host

The console follows Kiso. This file records product decisions. The installed
contracts own the shared rules.

## Contracts

- Packages in `console/`: `@momoi-labs/kiso-react` 0.27.0 and
  `@momoi-labs/kiso` 1.5.1, resolved in `console/package-lock.json`.
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
| `Shell` | Console navigation, health and theme selection |
| `AppForm`, `NativeAppForm`, `PostgresFields` | Application and database configuration |
| `CustomImageEditor`, `EnvironmentEditor` | Image and virtual machine configuration |
| `Glance` | Detail-page metrics |
| `LastRun`, `AppLogPane`, `LogSurface` | Run progress and output |
| `Failure`, `Causes` | Backend error reports |
| `RouteTable`, `RouteDialog`, `RedirectDialog` | Routes and redirects |

`EnvironmentEditor` is exported from `console/src/views/Environments.tsx`.
Other components above live in `console/src/components/`.

## Product rules

- Keep the console's list, detail and create compositions from
  `skills/add-page/SKILL.md` and ADR-0024.
- Use the same appearance attributes on all three HTML entry points.
  `console/tests/appearance.test.mjs` records the selected values.
- Default to light. A saved light, dark or system selection takes precedence
  before first paint. System removes `data-theme` and follows the OS.
- Use the tangerine accent, default presentation, an inset shell with compact
  outer chrome, full page width and start alignment.
- Use solid pixel frames, small corners, no corner marks, soft shadows,
  translucent panels and quiet fibers inside and outside the frame.
- Use stepped chart lines, dotted fills, standard chart layout and inline
  legends. Keep existing product-specific chart legend visibility.
- Use Kiso's `Card` paint layer for shaped shadows. Native frames need
  `PanelShadow` as their first child, as documented in the installed tokens.

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
| Settings: general, Host, DNS, certificates, API keys and scheduled tasks | `settings` |
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
