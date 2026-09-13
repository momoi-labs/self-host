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

## Kept here

`Failure` and `Causes` render `{ error, caused_by }` (ADR-0010). The cause
chain is the Platform's error contract, and Kiso's Alert already carries the
surface it sits on.

`useToast` attaches a Report to a Kiso toast. `StatusBadge` maps an
Application's status to a Badge variant. Both are the domain mapping and
nothing else.

`Icon` is a local sprite. Kiso ships BrandMark and TerminalIcon and leaves
the icon set to the product, which is the right split.

`Traffic` now maps Platform samples into Kiso's Chart,
shipped in 0.7.0. Self-host keeps the per-minute conversion, shared zero-based
scale and proxy error count. Kiso owns the plot and inspection. The console
hides the summary legend and exact-value disclosure to keep the charts compact.

`Meters` composes Kiso's Meter for CPU and memory, also shipped in 0.7.0.
Self-host keeps capacity ratios and displayed units. Network traffic now uses
Kiso Chart with timestamps and a shared byte scale for cumulative counters.
Received bytes plot above zero in green; sent bytes plot below zero in red. Inspection shows positive byte values and timestamps.
The time-axis labels stay hidden to keep the panel compact. Overview sums only
readings collected at the same timestamp.
Unknown capacity produces an uncollected track; readings above capacity keep
their displayed value while Kiso clamps the track.

`AppForm`, `ComposeEditor`, `ContainerResources`, `HttpStatus`, `LogPane` and
the views are Applications, Docker and the self-host API. Nothing to extract.

## Raised with Kiso, not extractions

Sparkline still defaults to a border color. The console assigns Kiso's
published `--color-chart-1` token so trends use the same data color as the
traffic chart, replacing the local accent-ramp choice.

Sparkline and Chart use Recharts. The production build reports a main page
chunk above 500 kB minified after adding the framed chart.

The console was writing Kiso's `.logview` and `.log-scroll` classes by hand
for the log pane's failure state. Fixed here by using LogView.
