# Login screen

Use this branch for the unauthenticated console entry in
`console/src/login.tsx`. The approved layout and evidence are recorded in
[the login research](../../docs/research/login-usability-and-kiso.md).
Do not apply the authenticated list, detail or create layout to this screen.

## Composition

- The screen has one card and one step at a time. `/account/status` picks
  the step: a non-canonical name shows a link to the console address, an
  account without a usable passkey shows setup, and any other account shows
  login. A login that retired the Platform key shows the CLI key step.
- Compose Kiso's BrandMark, Card, CardContent, FormField, PasswordInput,
  Button, Alert, Spinner and ThemeSelector. Keep authentication outside the
  application shell.
- At widths above 760px, the full-height help panel occupies half the page.
  Below that breakpoint, hide the panel and show the brand and server origin
  above the form. Keep key help in an expandable disclosure.
- Place the server origin under self-host. The step title is the page's
  visible heading.
- Inherit the quiet fibers background from the HTML appearance attributes in
  `docs/kiso.md`. Keep it theme-aware and the form readable on its translucent
  surface. Use existing Kiso tokens. Scope layout rules to authentication.
- Explain that `self-host init --show-key` prints the Platform key for the
  first passkey, and that `self-host account recover` prints a setup key when
  every passkey is lost.

## Credential behavior

- Login is one primary "Log in with a passkey" button. There is no password
  and no API key login. "Use a setup key" opens setup for recovery.
- Setup takes the Platform key or a setup key in a PasswordInput with labels
  that name the key, plus a passkey name. The key goes only to
  `/account/setup/*` as a Bearer header and is never stored.
- While a passkey prompt is open, disable the action and announce "Waiting for
  your passkey". Map a closed prompt, an existing credential and a refused
  origin to plain messages, and show the Platform's own refusal otherwise.
- The session is the `__Host-` cookie the Platform sets. The console keeps no
  credential in storage. A page that gets 401 or 403 from `/whoami` goes back
  to `/console`.

## Verify

1. Run `npm run typecheck`, `npm test` and `npm run build` in `console/`.
2. Start the real console through `cargo run -- serve` behind HTTPS, with
   `SELF_HOST_CONSOLE_ORIGIN` set to that origin. Passkeys need a secure
   context. Verify that it serves the rebuilt bundle. Use isolated Platform
   State initialized with `cargo run -- init --dns prototype.lan --api-key
   local`, and use `local` to set up the first passkey.
3. At desktop and mobile widths, check the split, hidden panel, visible server
   origin and available help. Check light, dark and system themes.
4. Use the keyboard to reach every action. Check the label, focus and error
   announcements.
5. With a virtual authenticator, check setup, login, a closed prompt, a
   rejected setup key, the CLI key step and the non-canonical message.
6. Report automated results separately from manual checks. If browser checks
   are unavailable or the Operator asked to avoid the browser, provide the
   real console URL and concrete manual steps. Do not claim those checks ran.
