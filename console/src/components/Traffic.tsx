import type { PlatformSample } from "../lib/types.js";

const WIDTH = 480;
const HEIGHT = 72;

/**
 * What the Platform's own servers answered, proxy and DNS on one plot
 * (ADR-0020). One axis, because both are events counted over the same
 * interval — two y-scales on one chart invent a correlation the data does
 * not have.
 *
 * The proxy carries the colour and DNS the de-emphasis gray, which is a
 * decision the validator made rather than a preference: kiso's ramp seats
 * exactly one step inside the dark surface's lightness band, so two hues is
 * not something this palette can do. Emphasis separates cleanly on both
 * surfaces (ΔE 24.7 light, 16.7 dark for normal vision), and the legend
 * carries each series' current value so identity is never colour alone.
 */
export function Traffic({
  platform,
  intervalSeconds,
}: {
  platform: PlatformSample[];
  intervalSeconds: number;
}) {
  // The collector counts per tick; a reader thinks per minute.
  const perMinute = (count: number) => Math.round((count * 60) / Math.max(1, intervalSeconds));
  const proxy = platform.map((sample) => perMinute(sample.proxy_requests));
  const dns = platform.map((sample) => perMinute(sample.dns_queries));
  const latest = platform[platform.length - 1];

  // One scale for both: counts of the same thing over the same interval, so
  // they are comparable, and zero is the floor a count is read against.
  const ceiling = Math.max(1, ...proxy, ...dns);

  return (
    <div className="traffic">
      <p className="t-caps">Platform traffic</p>
      <div className="legend">
        <span className="legend-item">
          <span className="legend-dot series-proxy" />
          Proxy{" "}
          <b>
            {latest ? perMinute(latest.proxy_requests) : 0}
            <span className="muted">/min</span>
          </b>
        </span>
        <span className="legend-item">
          <span className="legend-dot series-dns" />
          DNS{" "}
          <b>
            {latest ? perMinute(latest.dns_queries) : 0}
            <span className="muted">/min</span>
          </b>
        </span>
        {latest?.proxy_errors ? (
          <span className="legend-item danger">{perMinute(latest.proxy_errors)} errors/min</span>
        ) : null}
      </div>
      {platform.length > 1 ? (
        <svg
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          width="100%"
          height={HEIGHT}
          preserveAspectRatio="none"
          role="img"
          aria-label={`Proxied requests and DNS queries per minute over the last ${platform.length} samples`}
          className="plot"
        >
          {[0.5, 1].map((fraction) => (
            <line
              key={fraction}
              className="grid"
              x1="0"
              x2={WIDTH}
              y1={HEIGHT * fraction - 0.5}
              y2={HEIGHT * fraction - 0.5}
            />
          ))}
          <polyline className="series-dns" points={points(dns, ceiling)} />
          <polyline className="series-proxy" points={points(proxy, ceiling)} />
        </svg>
      ) : (
        <p className="muted">Collecting.</p>
      )}
    </div>
  );
}

function points(values: number[], ceiling: number): string {
  return values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * WIDTH;
      const y = HEIGHT - 1 - (value / ceiling) * (HEIGHT - 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}
