/**
 * A line, not a chart: the shape of one series over the collected window.
 * Rendered as inline SVG with no library, because there is nothing to
 * interact with — the numbers beside it carry the precision.
 *
 * One point is not a shape, so it draws nothing rather than a flat rule the
 * eye reads as a border. The collector ticks on the minute (ADR-0020), so
 * every series starts there.
 */
export function Sparkline({
  values,
  width = 120,
  height = 32,
  stretch = false,
  label,
}: {
  values: number[];
  width?: number;
  height?: number;
  /** Fills the width it is given instead of keeping its own. The viewBox
   * still sets the drawing's proportions, so the stroke is pinned to the
   * screen rather than scaled with the box. */
  stretch?: boolean;
  label: string;
}) {
  if (values.length < 2) return null;

  // The extremes come out of the loop: a day of samples is 1440 points, and
  // the Overview draws one line per Application on every poll.
  const min = Math.min(...values);
  const span = Math.max(...values) - min || 1;
  const points = values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * width;
      const y = height - 1 - ((value - min) / span) * (height - 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      width={stretch ? "100%" : width}
      height={height}
      preserveAspectRatio={stretch ? "none" : undefined}
      role="img"
      aria-label={label}
      className="sparkline"
    >
      <polyline
        points={points}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
