# Console components reviewed for Kiso

The review [#99](https://github.com/momoi-labs/self-host/issues/99) asked
which console components belong in Kiso and which stay here. This is the
result, with the state of each decision. It is a review, not a licence to
migrate the console onto a different framework.

The rule applied throughout: Kiso takes the box, the keyboard and the
surface. Self-host keeps what a value means. A component that reads an
Application, a Docker container, a mise key or the Platform's error shape
stays here even when its markup looks generic.

## Already Kiso's

The console consumes these and owns nothing of them: AppShell, Sidebar,
Navigation, Breadcrumb, PageHeader, Card, Table, Stat, Tabs, EmptyState,
Search, FormField, Input, Textarea, Select, Button, Badge, Dot, Alert,
Spinner, ThemeSelector, Split with Pane and Splitter, LogView, AlertDialog,
and Toasts.

Split-pane layout, log display, confirmation dialogs and toast integration
were on the candidate list. All four were already published before this
review, so there is nothing left to extract from them. The console's
remaining code around them is wiring, not a component.

## Extracted, and now consumed

**Sparkline.** The console's inline SVG was the evidence the Kiso contract
cites: an even sample window, needed three times over, in a table cell, under
a value and once per container. Shipped in `@momoi-labs/kiso-react@0.5.0`.
The local copy is gone and the four call sites take the published one.

**ChipInput.** The dependency editor was one field holding several structured
values, each with a version and installer options, hand-built from spans and
a draft input. Shipped in 0.5.0 with the chip parts and the combobox keys.
The dependency editor is now composed from them, and `allow_builds` moved out
of its own form field into the chip it configures.

## Proposed, with a reuse case

**A code field, from `ShellEditor`.** A textarea over a syntax layer with a
synced line-number gutter. Two callers here already, build checks and the
Compose editor, and any console that edits a command or a config file needs
the same thing. The blocker is the highlighter: this one reads a global
`window.Prism` that the pages load themselves, which a package cannot assume.
Scope for Kiso: the frame, the gutter and the scroll syncing, with the
highlighting passed in as `highlight?: (code: string) => string`. Self-host
keeps loading Prism and keeps the token colours.

**A meter, from `Meters`.** A ratio against a known ceiling, drawn as a
clamped track with the reading beside it. Kiso has Stat for a value and
Sparkline for a shape, and nothing for a bounded fraction. Scope for Kiso:
`Meter` with `value`, `max` and a label, `role="meter"`, and the clamp that
keeps a Host busier than its cores from drawing past the end of its track.
Self-host keeps which numbers are meters and which are lines, per ADR-0020.

## Kept here

`Failure` and `Causes` render `{ error, caused_by }` (ADR-0010). The cause
chain is the Platform's error contract, and Kiso's Alert already carries the
surface it sits on.

`useToast` attaches a Report to a Kiso toast. `StatusBadge` maps an
Application's status to a Badge variant. Both are the domain mapping and
nothing else.

`Icon` is a local sprite. Kiso ships BrandMark and TerminalIcon and leaves
the icon set to the product, which is the right split.

`Traffic` draws two series on one axis with a legend. It is not proposed:
Kiso's Sparkline contract explicitly defers the framed chart, and a legend
with axis labels is that chart. Revisit when Kiso opens the contract.

`AppForm`, `ComposeEditor`, `ContainerResources`, `HttpStatus`, `LogPane` and
the views are Applications, Docker and the self-host API. Nothing to extract.

## Raised with Kiso, not extractions

Sparkline's tones do not survive this console's dark card. `neutral` is
`--color-border-strong` and `primary` is `--color-primary`, which measures
L 0.83 at chroma 0.082 as a data mark and reads gray. The console overrides
`.sparkline` with `--color-accent-600`. That override should be a question
about the token, not a permanent local rule.

Sparkline brings Recharts, which costs the console page 95 kB gzipped. The
console is embedded in the binary and served over a LAN, so it is affordable,
but the drawing is a single path and the dependency is worth questioning.

The console was writing Kiso's `.logview` and `.log-scroll` classes by hand
for the log pane's failure state. Fixed here by using LogView.
