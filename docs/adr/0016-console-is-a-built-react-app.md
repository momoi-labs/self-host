# The console is a built React app, and its bundles are committed

The console started as four HTML pages and 1300 lines of JavaScript that built
every view by concatenating strings. The design system it draws on ships React
components now (`@momoi-labs/kiso-react`), and reimplementing a sidebar, a
splitter and a log view by hand — three of the components that package
exports — is work the Platform should not be doing.

The console is a React application built with Vite. `console/` holds the
sources; `npm run build` renders them into `console/dist/`, which is what
`rust_embed` carries into the binary. The four URLs are unchanged: `/console`
is the login page, `/console/` is the application, and the DNS setup and API
key pages stay linkable on their own. Each is a separate entry point rather
than a route in a client-side router, so the Rust routes and every existing
link keep working.

Adding a Node build to a Rust project raises the question of who runs it.
`npm run build` is a step before cargo, in CI and in the release workflow, and
a command a contributor runs after changing `console/src/`. `console/dist/` is
not committed: a bundle is not something a person writes, and it does not
belong in a diff.

The directory itself is tracked, empty, because `rust_embed` needs it to exist
to compile — without it `cargo build` fails inside the derive macro, which
tells the reader nothing. With it, a crate that was never given a console
compiles and `cargo test` says what is missing.

Committing the output was the alternative: it would keep `cargo build` and the
six cross-compiled targets working with no Node, at the price of generated
files in the history and a CI job to prove they were not hand-edited. A
`build.rs` that shells out to npm was the other: it makes the coupling
automatic, and puts Node in the way of every `cargo test`.

The design system arrives as a dependency rather than a copy. kiso publishes
`tokens.css` and `ui.css` as package exports, and
`@momoi-labs/kiso-react/styles.css` imports both, so the vendored copies of
those two files and the script that checked them for drift are gone. The
console keeps one stylesheet of its own for what kiso deliberately leaves to
the product.

**Status:** accepted

**Supersedes:** the vendored-CSS drift check
(`scripts/check-vendored-css.sh`), removed with this change.
