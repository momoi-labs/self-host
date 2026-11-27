---
"self-host": patch
---

The console has a new layout.

The sidebar now has groups. Workloads holds Apps, Databases, Machines and
Images. Network holds Routes and DNS. System holds Events, Host and
Settings. Apps, Databases and Machines show a red count when a workload has
failed.

The Overview is now a triage screen. A Needs attention card lists every
failed workload with its error and a button to open it. One card shows
workloads up, Host load, proxy traffic and DNS queries. Below them, Top
workloads ranks apps, databases and machines by resource use, and Recent
activity shows the latest events and running operations.

Apps and Machines have their own list pages, with search and a status
filter. A detail screen's breadcrumb leads back to its list.

A failed application or database says "Failed" once, in the status badge.
The loud alert stays while the workload is down. A failed deploy on a
workload that still runs gets a quiet alert.

The console moves to Kiso's default look with the stone accent: square
frames with corner ticks, a soft shadow on the outermost panel only, and a
quiet solid background. Tables have a single frame. A table inside a card or
a detail tab no longer draws a second border or shadow. A pane that holds
only a table shows its title above the table frame, with no card around it.

The diagram on an application or database summary inverts the theme: dark
on a light page and light on a dark page. It has small rounded corners, a
quiet dot grid and notched arrowheads.
