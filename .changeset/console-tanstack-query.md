---
"self-host": patch
---

The console reads Platform state through TanStack Query. Screens that show
the same data share one cached copy, so a screen opened again shows the last
answer at once while it refreshes. Polling pauses while the console tab is
hidden and catches up when the tab is shown again. A failed read keeps the
last answer on screen beside the error, where some lists used to go empty.
