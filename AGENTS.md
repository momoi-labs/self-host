# self-host

Super easy home lab PaaS for a LAN Host.

## Language

**All repository docs, GitHub issues, and pull requests must be written in English** (the project will be open-sourced). Domain terms follow `CONTEXT.md`.

## Agent skills

### Issue tracker

Issues and PRDs live as GitHub issues in this repo (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Canonical triage roles use matching label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `CONTEXT.md` + `docs/adr/`. See `docs/agents/domain.md`.

## Console

The console is a React app in `console/`, built with Vite onto
`@momoi-labs/kiso-react`, and embedded in the binary from `console/dist/`.
That directory is generated and not committed, so building the console is a
step before cargo:

```sh
cd console && npm ci && npm run build
```

Run it after changing anything under `console/src/`. `cargo test` fails with
`the console bundle is missing` when you have not. See
[ADR-0016](docs/adr/0016-console-is-a-built-react-app.md).

## Change handoff and testing

After implementing a change:

1. Start the final response with a short summary of what changed.
2. Give concrete manual test steps for the self-host console, including what
   to click and the expected result of each step.
3. Use the real console served by `cargo run -- serve` from the repository
   root. Rebuild the console as described above when needed, verify the server
   is running the updated code, and provide its console URL.
4. Report automated checks separately from the manual tests the user should
   perform. State any blockers that prevent testing against the real backend.
