---
name: add-page
description: Build a new console screen on the standard layout, or check an existing one against it. Use when adding a page, view, list or detail screen to console/src, or when reviewing whether a screen follows the console's layout rules.
---

# Add a console page

The console has one layout for list screens and one for detail screens
(ADR-0024). A new screen fills the shape in; it does not invent one. This walks
the shape, then checks the result.

Read `docs/adr/0024-detail-and-list-screens-share-one-layout.md` before
starting. This file is the checklist; the ADR is the reasoning, and the
reasoning is what tells you which rule a genuinely new kind of screen is
allowed to break.

## First: which kind of screen is it?

- **List** — many records of one kind, and a way to add one more.
- **Detail** — one record, its configuration, and whatever it prints.

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
- Creating is a dialog if it is a form, not a card parked above the table.
- Empty state: `EmptyState variant="first-run"` when there are no records at
  all, and a plain "No things match your filters." row when a filter emptied
  it. They are different situations and say different things.

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
      <form className="stack" onSubmit={save}>
        …the fields…
        <SaveBar
          tone={dirty ? "warning" : "neutral"}
          message={dirty ? <><b>Unsaved changes.</b> …</> : "Saved."}
          actions={<>
            {dirty ? <Button size="sm" type="button" onClick={reset}>Discard</Button> : null}
            <Button size="sm" type="submit" variant="primary">Save</Button>
          </>}
        />
      </form>
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
  measure → no `Glance`. No hostname → no description line.
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
- The form ends in `SaveBar` from `console/src/components/SaveBar.js`, never
  in a `.form-actions` row. Its tone is the record's state (neutral, warning
  for unsaved edits, accent for saved-not-applied, danger for a failed
  attempt) and Discard renders only when there is something to discard.
- Facts the Host reads back about the record (what is installed, how the last
  run went) go in a Summary tab before Configuration, and the screen opens
  on it. They are not configuration and do not go under the form.
- A long-running action is a tab (`LastRun`), not a progress bar in the
  header. The header adds a badge naming the action and its fraction.

What goes **inside** the Configuration tab is the screen's own business. The
Application has four fields, a machine has a five-step `Steps` recipe. Do not
make them look alike for its own sake.

## Then check it

Work through these against the screen you just wrote or are reviewing. Each
one is a question with a yes or no answer — if it is no, fix it or say why the
screen is the exception.

1. Does it reuse `PageHeader`, `Lifecycle`, `Glance`, `SaveBar`,
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
6. At 1280px, does the header row still fit on one line? Four verbs plus two
   badges plus a long destructive label does not.
7. Does it work in both themes, and does the sidebar still stack below 1024px?
8. `npm run typecheck && npm run build`, then load the screen in the real
   console from `cargo run -- serve`. A screenshot of the prototype is not
   evidence the real screen renders.

## If the shape does not fit

Say so, in one or two sentences, with what the screen needs that the layout
does not give it. Then either extend the layout for every screen or record why
this one is an exception. Do not quietly fork it — four screens each inventing
their own answer is exactly what ADR-0024 was written to end.
