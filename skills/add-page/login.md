# Login screen

Use this branch for the unauthenticated console entry in
`console/src/login.tsx`. The approved layout and evidence are recorded in
[the login research](../../docs/research/login-usability-and-kiso.md).
Do not apply the authenticated list, detail or create layout to this screen.

## Composition

- Keep one API key field and one primary "Unlock console" action. Compose
  Kiso's BrandMark, Card, CardContent, FormField, PasswordInput, Button,
  Alert, Spinner and ThemeSelector. Keep authentication outside the
  application shell.
- At widths above 760px, the full-height help panel occupies half the page.
  Below that breakpoint, hide the panel and show the brand and server origin
  above the form. Keep key retrieval help in an expandable disclosure.
- Place the server origin under self-host. Keep a programmatic heading for
  the form without adding a second visible title or Host line.
- Keep the quiet Momoi background theme-aware and the form readable on its
  translucent surface. Use existing Kiso tokens. Scope product layout rules
  to the authentication screen.
- Explain that `self-host init --show-key` runs on the Host and prints the
  existing initial key. An uninitialized Host needs `self-host init` first.

## Credential behavior

- Use Kiso's PasswordInput inside the FormField, with labels that name the
  API key. It masks the key by default and reveals it in the monospace face
  at the same size and width. Its Show/Hide button has an accessible name,
  `aria-controls` and an announced state, and toggling preserves the value.
- Keep paste available. PasswordInput already sets
  `autocomplete="current-password"` and turns off capitalization,
  correction and spellcheck, including while revealed.
- Reject empty or malformed keys before sending a request. Keep the value on
  failure. Distinguish key rejection from service and connection failures,
  and give the Operator a useful next action without exposing exceptions.
- During submission, guard against duplicate requests, make the input
  read-only, disable submit and announce "Checking key". Keep the form visible.
- Preserve the existing success flow: check `/health`, save the accepted key
  in tab sessionStorage and open `/console/`. Do not change authentication or
  persistence as part of a layout adjustment.

## Verify

1. Run `npm run typecheck`, `npm test` and `npm run build` in `console/`.
2. Start the real console through `cargo run -- serve`. Verify that it serves
   the rebuilt bundle. Use isolated Platform State for disposable credentials.
3. At desktop and mobile widths, check the split, hidden panel, visible server
   origin and available help. Check light, dark and system themes.
4. Use the keyboard to paste, reveal, hide and submit. Revealing must keep the
   value and font size stable. Check the label, focus and error announcements.
5. Check empty input, rejected credentials, a successful unlock, service and
   network failures, and repeated submission while a request is pending.
6. Report automated results separately from manual checks. If browser checks
   are unavailable or the Operator asked to avoid the browser, provide the
   real console URL and concrete manual steps. Do not claim those checks ran.
