# Metrics are collected in-process and kept in memory

The Platform collects its own metrics and keeps them in memory, inside the
daemon. There is no Prometheus, no cAdvisor, no time-series database: no
Infra container comes back, no dependency is added, and nothing is written to
disk. The console reads what the daemon already holds.

This continues what DNS (ADR-0017), state (ADR-0018) and HTTP (ADR-0019)
already established: the binary serves itself. A monitoring stack idles at
hundreds of megabytes of RAM and writes to disk continuously, on a Host whose
whole point is to run the Operator's Applications. The Platform's own numbers
cost a lock and a counter on the request path, and one `docker stats` call
every ten seconds.

## What is collected

Two sources, because the Platform owns two kinds of traffic.

The proxy and DNS are in-process, so they are counted where they happen: a
request the proxy routes and a query DNS answers increment a counter under a
lock. Nanoseconds, no allocation, and exact — the daemon is the only one
holding the socket, so there is nothing to sample with.

Application containers belong to Docker, so they are sampled: one background
task runs `docker stats --no-stream` on a tick and joins containers to
Applications by the `sf.app.id` label. Use is summed across containers; a
ceiling is not. `docker stats` reports the Host's whole memory as the limit of
a container that has none, so adding limits would invent memory the Host does
not have — the Platform keeps the largest ceiling in play instead. The core
count comes from the process rather than from Docker, because a percentage
where one busy core reads 100 only becomes a fraction of the Host once there
is something to divide it by. The tick defaults to ten seconds, which
is what makes a chart a chart — a minute of resolution only draws a line after
the Operator has waited several minutes for it — and it is what one
`docker stats` costs: the call reads cgroup counters Docker already keeps, so
the Host pays for the process, not for the measurement. Each container is kept as it was read
and the Application is their sum, because a Compose Application is several
processes with different appetites: the total says the Host is busy, the
container says which service is doing it. The daemon reads what Docker
already tracks in cgroups; it does not instrument a container's process.

## What is kept

Two flags on `self-host serve`, because the ring is one divided by the other:

    --monitoring-collect-interval  how often it reads  (default 10s)
    --monitoring-max-age           how much it keeps   (default 5m)

Thirty samples per series by default, one series per Application plus one per
container. Five minutes is what an Operator watching a deploy is looking at,
and anything longer is a question for the monitoring stack this Platform does
not run. The flags are there because the right answer is the Operator's Host
to decide, and because the two trade against each other: a day of history at
ten seconds is 8640 samples, while the same day at one minute is 1440. Either
is kilobytes and neither touches the disk, so the cost worth naming is the
`docker stats` call, not the memory. A window shorter than the interval is
refused at startup rather than rounded up to one sample.

Counters are cumulative; each tick's series entry stores the delta since the
previous one. A restart starts the series over — metrics describe how the
Platform is running right now, and state that must survive a restart lives in
the store (ADR-0018).

An Application removed from the store drops out on the next tick. A container
does not, because `docker stats` names only running containers and a restart
is exactly what its chart is read around: its series stays until a whole
window passes with nothing in it, at which point the container is gone rather
than quiet. Docker being unreachable keeps what is there rather than erasing
it, because a Host whose Docker is down is exactly the moment its history is
worth reading.

## Reading them

`GET /metrics` on the Operator API answers the series per Application and per
container, the proxy's totals per Hostname, DNS query counts, the interval it
collected them on, and the Host's core count. The console polls at half that
interval — the daemon owns the cadence, so the flags are the only place it is
set.

How it draws them follows from what the numbers are. CPU and memory are
fractions of a known ceiling, so they are **meters**: a bar against its track,
because "203 MiB of 15.7 GiB" set as prose is a ratio nobody reads as 1.3%.
Network has no ceiling to measure against and keeps a line. The overview meters
the Host, summed across Applications; the detail meters one Application, and
its Resources tab lists the containers underneath — one table, because the
Services list and the per-container metrics are the same rows, and printing
them apart printed every one-container Application twice. The Application's
detail header carries a single line of numbers, so the panel below it keeps its
height.

The proxy's and DNS's own counts share one plot, because both are events
counted over the same interval and two y-scales on one chart invent a
correlation the data does not have. They are drawn as emphasis — the proxy in
the series colour, DNS in the de-emphasis gray — and that is a result rather
than a preference: kiso's ramp seats exactly one step inside the dark
surface's lightness band, so a second categorical hue is not something this
palette can do. Emphasis separates cleanly on both surfaces, and the legend
carries each series' current value, so identity is never colour alone.

Two more rules the drawing obeys. A series of one point draws nothing, because
a flat rule reads as a border rather than as a measurement. And a mark is not
`--color-primary`: that token is tuned for text contrast, and against the dark
surface it measures L 0.83 with chroma 0.082 — inside the contrast rule, below
the floor where a mark still reads as coloured. The series takes the nearest
step on the same ramp that passes as a mark.

Nothing pushes, nothing scrapes, and no Consumer-facing port exposes an
Operator's view of the LAN's traffic.

**Status:** accepted
**Context:** [ADR-0017](0017-host-native-dns.md),
[ADR-0018](0018-platform-state-in-files.md),
[ADR-0019](0019-embedded-http-proxy.md).
