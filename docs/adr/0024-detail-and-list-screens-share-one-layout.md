# Detail and list screens share one layout

Every screen in the console had invented its own. Four list screens answered
"where does search go, and where does the way to add one more go" four
different ways, and three detail screens disagreed about whether a screen even
says its own name. This settles both, so a new screen has a shape to fill in
rather than a decision to relitigate.

## The list screen

Title and description in a `PageHeader`, with the primary create action in its
`actions` slot. One filter row under it: search first and growing, any extra
filter after it, and "Clear filters" only once something is filtered. Then the
table in a `.table-wrap`, with the count in its `.table-footer`.

The create action sits beside the title because it is the page's one primary
verb, and a page has one. It does not sit in the filter row: narrowing a list
and adding to it are opposite intentions, and a row that holds both makes the
reader sort them out every time. The count rides in the footer because it is a
fact about the table, and it belongs with the table rather than above it where
it competes with the search for the same glance.

`PageHeader` already had the `actions` slot. No screen used it. Custom images
reached for a `.dashboard-filters` class that was never defined anywhere, which
is how its create action ended up on a line of its own.

Creating from a list is a dialog, not a card parked above the table. API keys
kept a "Create a key" panel on screen permanently for a form of one field.

## The detail screen

One header row, then one card.

The row is three groups, in this order: status, the lifecycle verbs, and the
destructive action.

    [ VM running │ Service ready ]  [ Start │ Stop │ Restart │ Bootstrap ]   Delete

Status is one object and the verbs are another because they are different
kinds of thing, and eight controls in a line said they were the same kind.
Each cluster gives its children's borders and corners to a shared frame, and a
hairline between them is what says they are separate readings, or separate
choices. The difference between the two clusters is the difference between
reading and pressing: status carries no shadow and each badge keeps the tone
surface that carries its meaning; the verbs carry the surface and the shadow
each button used to carry alone.

The destructive action stands outside both with a wider gap. Nothing is flush
against it, so a slip on Restart cannot land on Delete. It lives here rather
than at the bottom of the form, where an Application's used to: removing a
thing is a lifecycle action, and the lifecycle actions are in this row.

A screen fills only the slots it has. A custom image is built, not run, so it
offers no verbs — the middle group is absent rather than rendered empty,
because a bordered box with nothing in it reads as something that failed to
load. An image has no hostname and nothing running to measure, so it carries
neither the link line nor the glance.

Under the row, `.glance` keeps the single line of numbers it already carried
(ADR-0020) — CPU, memory, network, each with the shape of its window.

Below that, one card of tabs: Configuration, then whatever the screen prints.
Not a split. A split gives the form half a wide screen, which is more than the
fields need and less than a two-thousand-line boot log wants, and it asks the
Operator to keep two scrollers in their head. The log takes the card out to
its edges, because a boot does not read in a box three lines tall.

What goes *inside* the Configuration tab is not standardised. The Application
has four fields, a machine has a five-step recipe, and a custom image has four
steps. These describe different things and there is nothing to gain by making
them look alike.

### The form's footer is a save bar

Every detail form ends in `SaveBar`, and the bar sticks to the bottom of
whatever scrolls. It says where the record stands and offers the one move
that follows: save what changed, apply what was saved, retry what failed. The
tone is the state: nothing to say, edits not saved, saved and waiting for the
machine, the last attempt failed.

The machine's form used to end in a row of three buttons and a grey caption
that read "Saved configuration" whether or not anything was pending, five
steps below the fold. An Operator who edited a port and left the tab had no
way to know they had not saved it. The bar is read without scrolling, and it
only offers Discard when there is something to discard.

### A screen that derives facts gets a Summary tab first

A record that the Host reads back, such as which tools a machine actually
has against which it was asked for, or how its last run went, puts those
facts in a Summary tab ahead of Configuration, and the screen opens on it.
They are not configuration and do not belong under the form: the installed
versions used to sit under the machine's fifth step, past the Save, where
nobody scrolled to find them.

### A run is a tab, not a bar in the header

A machine's lifecycle action walks a dozen steps over several minutes. The
header row used to swap its badges for a progress bar while one ran, and a
failure left the Operator with a cause chain and a two-thousand-line log to
search. The Last run tab lists the steps with their timing, opens the failed
one on the output the guest printed just before it gave up, and offers the
retry. The header keeps its badges and adds one that names the action and
its fraction, so something still moves while the run goes; the screen opens
on the run when one starts.

## What this removed

The machine's screen had no heading: the breadcrumb said its name and the room
went to the action row. With the row grouped, the room is there, and a screen
with no heading was the only one of its kind.

The Application's Resources tab and the machine's metrics card are both gone.
Both restated what the glance already says, and the per-container breakdown
they carried belongs on the Overview, where containers are compared against
each other rather than read one Application at a time. ADR-0020's paragraph on
the detail screen is superseded by this file to that extent; everything it
says about collection, retention and how the numbers are drawn still holds.

## Keeping it

`.list-filters` in `console/src/console.css` is the filter row. `Lifecycle` in
`console/src/components/Lifecycle.tsx` is the detail header row, and `Glance`
in `console/src/components/Glance.tsx` is the line under it. Both take
slots rather than data, so a screen keeps its own conditions — which verbs are
disabled, whether the machine is mid-operation — without the component
learning about any of them. `SaveBar` in `console/src/components/SaveBar.tsx`
is the form's footer, on the same terms: a tone, a message and an actions
slot. `LastRun` in `console/src/components/LastRun.tsx` draws a run parsed
out of the machine's event log by `console/src/lib/runSteps.ts`.

The `/add-page` skill walks a new screen through these rules.
