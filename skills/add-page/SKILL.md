---
name: add-page
description: Build a new console screen on the standard layout, or check an existing one against it. Use when adding a page, view, list or detail screen to console/src, or when reviewing whether a screen follows the console's layout rules.
---

# Add a console page

The console has one layout for list screens, one for detail screens and one
for create screens (ADR-0024). A new screen fills the shape in; it does not
invent one. This walks the shape, then checks the result.

Read `docs/adr/0024-detail-and-list-screens-share-one-layout.md` before
starting. This file is the checklist; the ADR is the reasoning, and the
reasoning is what tells you which rule a genuinely new kind of screen is
allowed to break.

## First: which kind of screen is it?

- **List** — many records of one kind, and a way to add one more.
- **Detail** — one record, its configuration, and whatever it prints.
- **Create** — one record that does not exist yet, reached from "Create
  resource": a form on a page of its own.

Anything else (the Overview, DNS setup, login) is neither, and these rules do
not apply. Say so rather than forcing it.

## A list screen

```tsx
<PageHeader actions={<Button size="sm" variant="primary"><Icon name="plus" />New thing</Button>}>
  <PageHeaderTitle>Things</PageHeaderTitle>
  <PageHeaderDescription>One sentence on what these are.</PageHeaderDescription>
</PageHeader>
<div className="list-filters">
  <Search aria-label="Search things" placeholder="Search things…" value={query}
    onChange={(event) => setQuery(event.target.value)} />
  {/* any extra filter goes here */}
  {filtering ? <Button size="sm" variant="ghost" onClick={clear}>Clear filters</Button> : null}
</div>
<div className="table-wrap">
  <div className="table-scroll"><Table>…</Table></div>
  <p className="table-footer"><span>{visible.length} of {all.length} things</span></p>
</div>
```

Rules:

- The create action goes in `PageHeader`'s `actions` slot. Not in the filter
  row. Narrowing a list and adding to it are opposite intentions.
- Search comes first in the filter row and grows.
- "Clear filters" renders only once something is filtered. A permanent one is
  a button that does nothing most of the time.
- The count goes in `.table-footer`, not above the table.
- Creating a small record (a DNS record, an API key) is a dialog, not a card
  parked above the table. A record with a recipe gets a create screen.
- Empty state: `EmptyState variant="first-run"` when there are no records at
  all, and a plain "No things match your filters." row when a filter emptied
  it. They are different situations and say different things.
- A record mid-operation shows it in one line: a `StatusBadge` naming the
  phase, kiso's `StepBar` from `barSteps` in `console/src/lib/runSteps.js`,
  and the count. No ticker in a cell; the record's own screen has the
  output. A record doing nothing says "Idle", and the bar is not drawn.

## A detail screen

```tsx
<div className="between">
  <PageHeader>
    <PageHeaderTitle>{record.name}</PageHeaderTitle>
    {link ? <PageHeaderDescription>…</PageHeaderDescription> : null}
    {samples ? <Glance samples={samples} /> : null}
  </PageHeader>
  <Lifecycle
    status={<>…badges…</>}
    actions={verbs.length ? <>…buttons…</> : undefined}
    destructive={<Button size="sm" variant="ghost" className="btn-danger-ghost">Delete thing</Button>}
  />
</div>

<Card className="detail-tabs">
  <Tabs value={tab} onValueChange={setTab}>
    <TabsList aria-label="Thing details">
      {/* only when the Host reads facts back about the record */}
      <TabsTrigger value="summary">Summary</TabsTrigger>
      <TabsTrigger value="configuration">Configuration</TabsTrigger>
      <TabsTrigger value="logs">Logs</TabsTrigger>
    </TabsList>
    <TabsContent value="configuration">
      <Form onSubmit={save}>
        <div className="form-body">…the fields…</div>
        <FormActions
          sticky
          tone={dirty ? "warning" : "neutral"}
          message={dirty ? <><strong>Unsaved changes.</strong> …</> : "Saved."}
        >
          {dirty ? <Button size="sm" type="button" onClick={reset}>Discard</Button> : null}
          <Button size="sm" type="submit" variant="primary">Save</Button>
        </FormActions>
      </Form>
    </TabsContent>
    <TabsContent value="logs" className="detail-logs">…the log…</TabsContent>
  </Tabs>
</Card>
```

Rules:

- The screen says its own name. The breadcrumb repeating it is what a
  breadcrumb is for.
- Use `Lifecycle` from `console/src/components/Lifecycle.js`. Do not hand-roll
  the row, and do not add a fourth group to it.
- Fill only the slots the screen has. No verbs → pass no `actions`, and the
  group is absent rather than an empty bordered box. Nothing running to
  measure → no `Glance`. No hostname → no description line. No service yet
  on a machine being created → no "Service disabled" badge.
- Destructive actions live in this row, never at the bottom of the form.
- `Glance` from `console/src/components/Glance.js` is the only metrics surface
  a detail screen gets. No meters card, no resources tab: the per-container
  breakdown belongs on the Overview.
- One `.detail-tabs` card. Not a split.
- A log or terminal panel takes `className="detail-logs"` or
  `"detail-terminal"` so it reaches the card's edges.
- The tab already names the panel. Do not caption the panel with the same
  word: the Application's log pane said "Logs" under a tab called Logs.
- A single tab means no tab bar. A lone tab is not a choice.
- Status is a `StatusBadge` from `console/src/components/StatusBadge.js`
  with one of its three tones. Green is alive: running, building, a run
  that is going or that ended well. Red is failed. Neutral is what is not
  happening: pending, stopped, disabled. No `Badge variant="info"`, no
  fourth colour for "running". A run or build in progress passes `pulse`
  and names its phase, "Starting" or "Provisioning" from `phase()` in
  `console/src/lib/runSteps.js`, never "create: running".
- The form is kiso's `Form`, its fields in a `.form-body`, and it ends in
  kiso's `FormActions` with `sticky`, never in a hand-rolled footer row. Its
  tone is the record's state (neutral, warning for unsaved edits or saved
  and not applied, danger for a failed attempt) and Discard renders only
  when there is something to discard. A tab panel that holds a `.form` gives
  its padding to the body, so the bar spans the panel and a form shorter
  than the panel still ends at the panel's edge.
- Facts the Host reads back about the record (what is installed, how the last
  run went) go in a Summary tab before Configuration, and the screen opens
  on it. They are not configuration and do not go under the form.
- A long-running action is a tab (`LastRun`: kiso's `StepList` laid over
  the run, in a `Split` beside the selected step's `LogView`, under the row
  below 1024px), not a progress bar in the header and not a dock under the
  form. Its panel takes `className="detail-run"`.
  The header adds a pulsing badge naming the phase the run is in, from
  `phase()` in `console/src/lib/runSteps.js`, and the screen opens on that
  tab when the action starts. Every move after that is the Operator's to
  keep.
- What the record prints (a boot log, a build log) is a tab with
  `className="detail-logs"`, opened by itself when the printing starts.

What goes **inside** the Configuration tab is the screen's own business. The
Application has four fields, a machine has a five-step `Steps` recipe. Do not
make them look alike for its own sake.

## A create screen

```tsx
<PageHeader>
  <PageHeaderTitle>New thing</PageHeaderTitle>
  <PageHeaderDescription>One sentence on what you are about to make.</PageHeaderDescription>
</PageHeader>
<Card className="form-page">
  <Form onSubmit={create}>
    <div className="form-body">…the fields…</div>
    <FormActions sticky>
      <Button size="sm" type="button" onClick={onCancel}>Cancel</Button>
      <Button size="sm" type="submit" variant="primary" disabled={!valid}>Create thing</Button>
    </FormActions>
  </Form>
</Card>
```

Rules:

- The screen says what it makes, in the title, and says it once. No
  `Lifecycle` row: there is nothing to start, stop or delete yet.
- The form lives in `Card.form-page`, never bare on the page. The machine's
  used to be, and its footer had no edge to reach.
- No tabs. There is nothing to print yet; the tabs come with the record.
- The footer is `FormActions` with Cancel and the one primary verb, in that
  order. The verb names what happens: "Deploy", "Create virtual machine",
  "Save and build".
- The same form component serves the create screen and the detail screen's
  Configuration tab. Do not write it twice.

## Then check it

Work through these against the screen you just wrote or are reviewing. Each
one is a question with a yes or no answer — if it is no, fix it or say why the
screen is the exception.

1. Does it reuse `PageHeader`, `Lifecycle`, `Glance`, kiso's `Form` and `FormActions`,
   `.table-wrap`, `.detail-tabs` rather than restating them in new CSS?
2. Is every new class in `console/src/console.css` a rule kiso leaves to the
   product? If a second Momoi product would want it, it belongs upstream in
   the blueprint.
3. Does every new class name describe the thing rather than the screen that
   happened to need it first? `.detail-tabs`, not `.environment-tabs`.
4. Does every class you used actually exist? Custom images spent months
   reaching for `.dashboard-filters`, which was never defined.
5. Does the screen degrade when a field is null? The machine's `web_url` is
   `Option<String>` on the Rust side; a type that says `string` will crash the
   screen the first time the service is not answering.
6. At 1280px, does the header row still fit on one line, including any badge
   a running action adds? Four verbs plus three badges plus a long
   destructive label does not.
7. Does it work in both themes, and does the sidebar still stack below 1024px?
8. Is every status a `StatusBadge` in one of the three tones, with running
   green and only failed red, and does the dot pulse only while a run or a
   build is going? Search the screen for `variant="info"`.
9. Does the form end in `FormActions sticky`, does it stick when the form is taller than
   the panel, and does it reach the panel's edges when it is shorter?
10. Does the screen open on the right tab: Summary when it has one, the run
    or the log when one is going, and does it stay where the Operator put it
    afterwards?
11. `npm run typecheck && npm run build`, then load the screen in the real
    console from `cargo run -- serve`. A screenshot of the prototype is not
    evidence the real screen renders.

## If the shape does not fit

Say so, in one or two sentences, with what the screen needs that the layout
does not give it. Then either extend the layout for every screen or record why
this one is an exception. Do not quietly fork it — four screens each inventing
their own answer is exactly what ADR-0024 was written to end.
