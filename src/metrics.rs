//! Metrics the Platform collects about itself and its Applications, kept in
//! memory (ADR-0020).
//!
//! Two sources, two costs. The proxy and DNS are in-process, so they are
//! counted where they happen: a counter under a lock, no allocation.
//! Application containers belong to Docker, so they are sampled: one
//! `docker stats` call per tick, joined to Applications by label. What is
//! kept is a ring per Application and one per container, sized by the
//! [`Window`] the Operator asked for — a few kilobytes — and nothing is
//! written to disk.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::docker::{ContainerStats, DockerRuntime};
use crate::store::StateStore;

/// How often the collector samples Docker and folds counters into the
/// series, unless `--monitoring-collect-interval` says otherwise. Ten seconds
/// is what makes a chart a chart: a minute of resolution only draws a line
/// after the Operator has waited several minutes for it.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);

/// How much history the daemon keeps unless `--monitoring-max-age` asks for
/// more. Five minutes is what an Operator watching a deploy is looking at;
/// anything longer is a question for a monitoring stack this Platform does
/// not run (ADR-0020).
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(5 * 60);

/// Reads a duration flag: a plain number of seconds, or a number with an
/// `s`, `m` or `h` on it.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (digits, unit) = match text.strip_suffix(['s', 'm', 'h']) {
        Some(digits) => (digits, text.as_bytes()[text.len() - 1]),
        None => (text, b's'),
    };
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("'{text}' is not a duration like 30s, 5m or 2h"))?;
    Ok(Duration::from_secs(
        value
            * match unit {
                b'm' => 60,
                b'h' => 3600,
                _ => 1,
            },
    ))
}

/// What the collector runs on: how often it reads, and how much of the past
/// it keeps. The two only mean something together — the ring is one divided
/// by the other — so they are checked against each other once, here, before
/// the daemon starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    interval: Duration,
    max_age: Duration,
}

impl Default for Window {
    fn default() -> Self {
        Window {
            interval: DEFAULT_INTERVAL,
            max_age: DEFAULT_MAX_AGE,
        }
    }
}

impl Window {
    pub fn new(interval: Duration, max_age: Duration) -> Result<Self, String> {
        if interval.is_zero() {
            return Err("--monitoring-collect-interval cannot be zero".into());
        }
        if max_age < interval {
            return Err(format!(
                "--monitoring-max-age ({}s) is shorter than --monitoring-collect-interval ({}s), \
                 which would keep nothing to chart",
                max_age.as_secs(),
                interval.as_secs()
            ));
        }
        Ok(Window { interval, max_age })
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// How many samples one ring holds.
    fn retained(&self) -> usize {
        (self.max_age.as_secs() / self.interval.as_secs()).max(1) as usize
    }
}

/// One scope's resource usage at one tick, as `docker stats` reports it.
/// The scope is an Application — its containers summed — or one of those
/// containers on its own.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AppSample {
    /// Unix seconds.
    pub at: u64,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    /// Cumulative since the container started, not per-interval: the
    /// collector reads a counter Docker keeps, and a stopped Application
    /// shows a gap instead of a zero.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl AppSample {
    /// A zeroed reading to sum containers into.
    fn empty(at: u64) -> Self {
        AppSample {
            at,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_limit_bytes: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    fn add(&mut self, other: &Self) {
        self.cpu_percent += other.cpu_percent;
        self.memory_bytes += other.memory_bytes;
        self.rx_bytes += other.rx_bytes;
        self.tx_bytes += other.tx_bytes;
        // Use is additive; a ceiling is not. `docker stats` reports the Host's
        // whole memory as the limit of a container that has none, so summing
        // limits invents memory the Host does not have — two unconstrained
        // containers would claim twice the Host. The largest ceiling in play
        // is the one an Operator is measured against.
        self.memory_limit_bytes = self.memory_limit_bytes.max(other.memory_limit_bytes);
    }
}

/// One tick's reading of one Application: what each of its containers used,
/// and their sum. A Compose Application is several processes with different
/// appetites, so the console can chart the one that is misbehaving.
#[derive(Clone, Debug)]
pub struct AppTick {
    pub total: AppSample,
    /// Keyed by container name, the same key the Services table shows.
    pub containers: BTreeMap<String, AppSample>,
}

/// What the Platform's own servers did during one interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PlatformSample {
    /// Unix seconds.
    pub at: u64,
    pub proxy_requests: u64,
    pub proxy_errors: u64,
    pub dns_queries: u64,
}

#[derive(Default, Clone, Copy)]
struct HostTraffic {
    requests: u64,
    errors: u64,
}

/// The hot-path side: counters only, behind one lock. A request increments
/// and moves on; folding the counters into the series is the collector's
/// job, on its minute.
#[derive(Default, Clone)]
struct Counters {
    proxy_requests: u64,
    proxy_errors: u64,
    dns_queries: u64,
    proxy_by_hostname: HashMap<String, HostTraffic>,
    /// Local-zone names only, so the map stays the size of what the Platform
    /// published; a forwarded query counts and is not named.
    dns_by_name: HashMap<String, u64>,
}

/// One Application's history: the sum it reported each tick, and the same
/// window per container.
#[derive(Default)]
struct AppSeries {
    total: VecDeque<AppSample>,
    containers: BTreeMap<String, VecDeque<AppSample>>,
}

#[derive(Default)]
struct Series {
    applications: BTreeMap<String, AppSeries>,
    platform: VecDeque<PlatformSample>,
    /// The counter totals the previous platform sample was computed from.
    last_totals: (u64, u64, u64),
}

impl Default for Inner {
    fn default() -> Self {
        Inner::holding(Window::default())
    }
}

impl Inner {
    fn holding(window: Window) -> Self {
        Inner {
            counters: Mutex::default(),
            series: RwLock::new(Series::default()),
            retained: window.retained(),
            window,
            // Read once: the daemon runs on the Host it reports on, and a
            // core count does not change while it does.
            host_cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
        }
    }
}

/// Everything `GET /metrics` answers, built once per request.
#[derive(Debug, Serialize)]
pub struct MetricsSnapshot {
    pub interval_seconds: u64,
    /// Cores on this Host. `docker stats` reports 100% per fully used core, so
    /// a percentage only becomes a fraction of the Host once divided by this.
    pub host_cpus: usize,
    pub applications: Vec<AppSeriesSnapshot>,
    pub platform: Vec<PlatformSample>,
    pub proxy: Vec<HostTrafficSnapshot>,
    pub dns: DnsSnapshot,
}

#[derive(Debug, Serialize)]
pub struct AppSeriesSnapshot {
    pub id: String,
    /// Oldest first.
    pub samples: Vec<AppSample>,
    /// The same window, one series per container.
    pub containers: Vec<ContainerSeriesSnapshot>,
}

#[derive(Debug, Serialize)]
pub struct ContainerSeriesSnapshot {
    pub container: String,
    /// Oldest first.
    pub samples: Vec<AppSample>,
}

#[derive(Debug, Serialize)]
pub struct HostTrafficSnapshot {
    pub hostname: String,
    pub requests: u64,
    pub errors: u64,
}

#[derive(Debug, Serialize)]
pub struct DnsSnapshot {
    pub queries_total: u64,
    /// Busiest first.
    pub by_name: Vec<DnsNameSnapshot>,
}

#[derive(Debug, Serialize)]
pub struct DnsNameSnapshot {
    pub name: String,
    pub queries: u64,
}

/// The handle every part of the process shares: the proxy and DNS count into
/// it, the collector folds it, the API reads it. Cheap to clone, like the
/// `RouteTable` it outlives requests with.
#[derive(Clone, Default)]
pub struct Metrics {
    inner: Arc<Inner>,
}

struct Inner {
    counters: Mutex<Counters>,
    series: RwLock<Series>,
    window: Window,
    /// How many samples one ring holds: the window divided by the tick.
    retained: usize,
    host_cpus: usize,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Collects and keeps on the Operator's `window`.
    pub fn with_window(window: Window) -> Self {
        Metrics {
            inner: Arc::new(Inner::holding(window)),
        }
    }

    /// How often the collector should tick. The daemon reads its cadence off
    /// the metrics rather than carrying it separately, and so does the
    /// console, through the snapshot.
    pub fn interval(&self) -> Duration {
        self.inner.window.interval()
    }

    /// One proxied request. `error` is a `5xx`, ours or the Application's —
    /// both mean the Consumer did not get what they asked for.
    pub fn count_proxy_request(&self, hostname: &str, error: bool) {
        let mut counters = self.inner.counters.lock().unwrap();
        counters.proxy_requests += 1;
        counters.proxy_errors += error as u64;
        let traffic = counters
            .proxy_by_hostname
            .entry(hostname.to_ascii_lowercase())
            .or_default();
        traffic.requests += 1;
        traffic.errors += error as u64;
    }

    /// One query the local zone answered. The name is what the client asked
    /// for, trailing dot and all; only local-zone names reach here, so the
    /// per-name map stays bounded by what the Platform published.
    pub fn count_dns_query(&self, name: &str) {
        let mut counters = self.inner.counters.lock().unwrap();
        counters.dns_queries += 1;
        *counters.dns_by_name.entry(name.to_string()).or_insert(0) += 1;
    }

    /// One query the forwarder took, for a name outside the local zone.
    pub fn count_dns_forwarded(&self) {
        self.inner.counters.lock().unwrap().dns_queries += 1;
    }

    /// One Application's tick, its containers with it. A gap is left when
    /// there is nothing to record: a stopped Application reads as silence,
    /// not zero.
    pub fn record_app_tick(&self, id: &str, tick: &AppTick) {
        let retained = self.inner.retained;
        let mut series = self.inner.series.write().unwrap();
        let entry = series.applications.entry(id.to_string()).or_default();
        push(&mut entry.total, tick.total, retained);
        for (container, sample) in &tick.containers {
            push(
                entry.containers.entry(container.clone()).or_default(),
                *sample,
                retained,
            );
        }
        // A container that stopped keeps its history, because a restart is
        // exactly what the chart is read around. One that has said nothing
        // for the whole window is gone rather than quiet.
        let cutoff = tick
            .total
            .at
            .saturating_sub(retained as u64 * self.inner.window.interval().as_secs());
        entry
            .containers
            .retain(|_, samples| samples.back().is_some_and(|last| last.at >= cutoff));
    }

    /// Drops the series of Applications the store no longer has. The
    /// collector calls it every tick with the live ids.
    pub fn retain_applications(&self, live: &[String]) {
        self.inner
            .series
            .write()
            .unwrap()
            .applications
            .retain(|id, _| live.contains(id));
    }

    /// Folds the counters into one platform sample. Deltas, not totals: a
    /// restart of the Platform is not a spike in traffic.
    pub fn record_platform_tick(&self) {
        let totals = {
            let counters = self.inner.counters.lock().unwrap();
            (
                counters.proxy_requests,
                counters.proxy_errors,
                counters.dns_queries,
            )
        };
        let mut series = self.inner.series.write().unwrap();
        let (requests, errors, queries) = (
            totals.0.saturating_sub(series.last_totals.0),
            totals.1.saturating_sub(series.last_totals.1),
            totals.2.saturating_sub(series.last_totals.2),
        );
        series.last_totals = totals;
        series.platform.push_back(PlatformSample {
            at: now(),
            proxy_requests: requests,
            proxy_errors: errors,
            dns_queries: queries,
        });
        while series.platform.len() > self.inner.retained {
            series.platform.pop_front();
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let counters = self.inner.counters.lock().unwrap().clone();
        let series = self.inner.series.read().unwrap();

        let mut proxy: Vec<HostTrafficSnapshot> = counters
            .proxy_by_hostname
            .iter()
            .map(|(hostname, traffic)| HostTrafficSnapshot {
                hostname: hostname.clone(),
                requests: traffic.requests,
                errors: traffic.errors,
            })
            .collect();
        proxy.sort_by(|a, b| {
            b.requests
                .cmp(&a.requests)
                .then_with(|| a.hostname.cmp(&b.hostname))
        });

        let mut by_name: Vec<DnsNameSnapshot> = counters
            .dns_by_name
            .iter()
            .map(|(name, queries)| DnsNameSnapshot {
                name: name.clone(),
                queries: *queries,
            })
            .collect();
        by_name.sort_by(|a, b| b.queries.cmp(&a.queries).then_with(|| a.name.cmp(&b.name)));

        MetricsSnapshot {
            interval_seconds: self.inner.window.interval().as_secs(),
            host_cpus: self.inner.host_cpus,
            applications: series
                .applications
                .iter()
                .map(|(id, app)| AppSeriesSnapshot {
                    id: id.clone(),
                    samples: app.total.iter().copied().collect(),
                    containers: app
                        .containers
                        .iter()
                        .map(|(container, samples)| ContainerSeriesSnapshot {
                            container: container.clone(),
                            samples: samples.iter().copied().collect(),
                        })
                        .collect(),
                })
                .collect(),
            platform: series.platform.iter().copied().collect(),
            proxy,
            dns: DnsSnapshot {
                queries_total: counters.dns_queries,
                by_name,
            },
        }
    }
}

/// Appends one sample and drops whatever falls out of the retained window.
fn push(samples: &mut VecDeque<AppSample>, sample: AppSample, retained: usize) {
    samples.push_back(sample);
    while samples.len() > retained {
        samples.pop_front();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Groups one tick's container stats by Application, keeping each container
/// and summing them, all stamped with the same tick.
fn aggregate(stats: Vec<ContainerStats>) -> BTreeMap<String, AppTick> {
    let at = now();
    let mut applications: BTreeMap<String, AppTick> = BTreeMap::new();
    for stat in stats {
        let sample = AppSample {
            at,
            cpu_percent: stat.cpu_percent,
            memory_bytes: stat.memory_bytes,
            memory_limit_bytes: stat.memory_limit_bytes,
            rx_bytes: stat.rx_bytes,
            tx_bytes: stat.tx_bytes,
        };
        let tick = applications
            .entry(stat.application.clone())
            .or_insert_with(|| AppTick {
                total: AppSample::empty(at),
                containers: BTreeMap::new(),
            });
        tick.total.add(&sample);
        tick.containers.insert(stat.container, sample);
    }
    applications
}

/// Samples Docker once and folds the counters. One tick of the collector:
/// a Docker that cannot be reached keeps the series it has, because a Host
/// whose Docker is down is exactly the moment its history is worth reading.
pub async fn collect_once<S: StateStore>(store: &S, docker: &dyn DockerRuntime, metrics: &Metrics) {
    match store.list_applications().await {
        Ok(applications) => {
            let live: Vec<String> = applications.into_iter().map(|app| app.id).collect();
            match docker.container_stats().await {
                Ok(stats) => {
                    for (id, tick) in aggregate(stats) {
                        metrics.record_app_tick(&id, &tick);
                    }
                }
                Err(e) => tracing::warn!("could not sample Application stats: {e}"),
            }
            metrics.retain_applications(&live);
        }
        // Without the store there is no honest live list to prune by either.
        Err(e) => tracing::warn!("could not list Applications for metrics: {e}"),
    }
    metrics.record_platform_tick();
}

/// Runs [`collect_once`] once per tick, for as long as the daemon does.
pub fn spawn_collector<S: StateStore>(store: S, docker: Arc<dyn DockerRuntime>, metrics: Metrics) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(metrics.interval());
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` ticks immediately; the counters are already live and the
        // first Docker sample lands one interval out.
        timer.tick().await;
        loop {
            timer.tick().await;
            collect_once(&store, docker.as_ref(), &metrics).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: u64, memory: u64) -> AppSample {
        AppSample {
            at,
            cpu_percent: 1.0,
            memory_bytes: memory,
            memory_limit_bytes: memory * 10,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    /// One tick of an Application whose only container is `web`.
    fn tick(at: u64, memory: u64) -> AppTick {
        AppTick {
            total: sample(at, memory),
            containers: BTreeMap::from([("web".to_string(), sample(at, memory))]),
        }
    }

    /// How many samples fit in the default window.
    const RETAINED: usize = (DEFAULT_MAX_AGE.as_secs() / DEFAULT_INTERVAL.as_secs()) as usize;

    #[test]
    fn a_series_keeps_only_what_fits_in_its_window() {
        let metrics = Metrics::new();
        for at in 0..(RETAINED + 10) {
            metrics.record_app_tick("app", &tick(at as u64, 100));
        }
        let series = metrics.snapshot().applications;
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].samples.len(), RETAINED);
        assert_eq!(series[0].samples[0].at, 10, "the oldest samples dropped");
        assert_eq!(
            series[0].samples.last().unwrap().at as usize,
            RETAINED + 9,
            "the newest sample stayed"
        );
        assert_eq!(series[0].containers.len(), 1);
        assert_eq!(series[0].containers[0].container, "web");
        assert_eq!(series[0].containers[0].samples.len(), RETAINED);
    }

    #[test]
    fn a_longer_window_is_a_longer_ring() {
        let window = Window::new(Duration::from_secs(10), Duration::from_secs(3600)).unwrap();
        let metrics = Metrics::with_window(window);
        for at in 0..400 {
            metrics.record_app_tick("app", &tick(at, 100));
            metrics.record_platform_tick();
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.applications[0].samples.len(), 360);
        assert_eq!(snapshot.platform.len(), 360);
    }

    #[test]
    fn a_coarser_interval_is_a_shorter_ring_over_the_same_window() {
        let window = Window::new(Duration::from_secs(60), Duration::from_secs(3600)).unwrap();
        let metrics = Metrics::with_window(window);
        for at in 0..100 {
            metrics.record_app_tick("app", &tick(at, 100));
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.applications[0].samples.len(), 60);
        assert_eq!(
            snapshot.interval_seconds, 60,
            "the console polls on what the daemon collects on"
        );
    }

    #[test]
    fn a_duration_reads_as_seconds_minutes_or_hours() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration(" 5m "), Ok(Duration::from_secs(300)));
        assert_eq!(parse_duration("2h"), Ok(Duration::from_secs(7200)));
        assert!(parse_duration("later").is_err());
    }

    #[test]
    fn a_window_that_keeps_less_than_it_collects_is_refused() {
        let ten = Duration::from_secs(10);
        assert!(Window::new(ten, Duration::from_secs(5)).is_err());
        assert!(Window::new(Duration::ZERO, Duration::from_secs(300)).is_err());
        assert_eq!(Window::new(ten, ten).unwrap().retained(), 1);
    }

    #[test]
    fn a_container_that_stopped_keeps_its_history_until_the_window_runs_out() {
        let metrics = Metrics::new();
        let step = DEFAULT_INTERVAL.as_secs();
        metrics.record_app_tick(
            "app",
            &AppTick {
                total: sample(step, 200),
                containers: BTreeMap::from([
                    ("web".to_string(), sample(step, 100)),
                    ("db".to_string(), sample(step, 100)),
                ]),
            },
        );

        // `db` stops: `docker stats` stops naming it on the next tick.
        metrics.record_app_tick("app", &tick(step * 2, 100));
        let containers = &metrics.snapshot().applications[0].containers;
        assert_eq!(containers.len(), 2, "a stopped container is still charted");

        // And a full window later it is gone rather than quiet.
        metrics.record_app_tick("app", &tick(step * (RETAINED as u64 + 3), 100));
        let containers = &metrics.snapshot().applications[0].containers;
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].container, "web");
    }

    #[test]
    fn a_removed_application_drops_out_on_the_next_tick() {
        let metrics = Metrics::new();
        metrics.record_app_tick("gone", &tick(1, 100));
        metrics.record_app_tick("kept", &tick(1, 100));
        metrics.retain_applications(&["kept".into()]);
        let series = metrics.snapshot().applications;
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].id, "kept");
    }

    #[test]
    fn platform_samples_are_deltas_between_counter_readings() {
        let metrics = Metrics::new();
        metrics.count_proxy_request("blog.home.lan", false);
        metrics.count_proxy_request("blog.home.lan", true);
        metrics.count_dns_query("blog.home.lan.");
        metrics.record_platform_tick();

        metrics.count_proxy_request("blog.home.lan", false);
        metrics.count_dns_forwarded();
        metrics.record_platform_tick();

        let platform = metrics.snapshot().platform;
        assert_eq!(platform.len(), 2);
        assert_eq!(
            (
                platform[0].proxy_requests,
                platform[0].proxy_errors,
                platform[0].dns_queries
            ),
            (2, 1, 1)
        );
        assert_eq!(
            (
                platform[1].proxy_requests,
                platform[1].proxy_errors,
                platform[1].dns_queries
            ),
            (1, 0, 1)
        );
    }

    #[test]
    fn the_snapshot_sorts_traffic_busiest_first_and_lowercases_hosts() {
        let metrics = Metrics::new();
        metrics.count_proxy_request("quiet.home.lan", false);
        for _ in 0..3 {
            metrics.count_proxy_request("BLOG.home.lan", true);
        }
        metrics.count_dns_query("blog.home.lan.");
        metrics.count_dns_query("blog.home.lan.");

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.proxy[0].hostname, "blog.home.lan");
        assert_eq!(
            (snapshot.proxy[0].requests, snapshot.proxy[0].errors),
            (3, 3)
        );
        assert_eq!(snapshot.dns.queries_total, 2);
        assert_eq!(snapshot.dns.by_name[0].name, "blog.home.lan.");
    }

    #[test]
    fn container_stats_are_kept_apart_and_summed_per_application() {
        let stats = vec![
            ContainerStats {
                container: "a-web".into(),
                application: "a".into(),
                cpu_percent: 0.25,
                memory_bytes: 100,
                memory_limit_bytes: 1000,
                rx_bytes: 10,
                tx_bytes: 20,
            },
            ContainerStats {
                container: "a-db".into(),
                application: "a".into(),
                cpu_percent: 0.75,
                memory_bytes: 50,
                memory_limit_bytes: 1000,
                rx_bytes: 1,
                tx_bytes: 2,
            },
            ContainerStats {
                container: "b-web".into(),
                application: "b".into(),
                cpu_percent: 2.0,
                memory_bytes: 7,
                memory_limit_bytes: 70,
                rx_bytes: 0,
                tx_bytes: 0,
            },
        ];

        let aggregated = aggregate(stats);
        assert_eq!(aggregated.len(), 2);
        let a = &aggregated["a"];
        assert_eq!(a.total.cpu_percent, 1.0);
        assert_eq!(a.total.memory_bytes, 150);
        assert_eq!(
            a.total.memory_limit_bytes, 1000,
            "the ceiling is the largest in play, not the sum of ceilings"
        );
        assert_eq!((a.total.rx_bytes, a.total.tx_bytes), (11, 22));
        assert_eq!(aggregated["b"].total.memory_bytes, 7);

        // And each container is kept apart, so the console can chart the one
        // service that is eating the Host.
        assert_eq!(a.containers.len(), 2);
        assert_eq!(a.containers["a-db"].cpu_percent, 0.75);
        assert_eq!(a.containers["a-web"].memory_bytes, 100);
        assert_eq!(aggregated["b"].containers.len(), 1);
    }
}
