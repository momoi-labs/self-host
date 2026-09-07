/**
 * The console's own icons. kiso ships a BrandMark and a TerminalIcon; these
 * are the product's — an application, a chart, a key — and they travel as one
 * sprite so a page mounts them once.
 */
const paths: Record<string, React.ReactNode> = {
  plus: <path d="M8 3v10M3 8h10" />,
  x: <path d="M4 4l8 8M12 4l-8 8" />,
  alert: (
    <>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M8 5v3.5M8 11h.01" />
    </>
  ),
  info: (
    <>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M8 7.5V11M8 5h.01" />
    </>
  ),
  box: (
    <>
      <path d="M2.5 5L8 2l5.5 3v6L8 14l-5.5-3z" />
      <path d="M2.5 5L8 8l5.5-3M8 8v6" />
    </>
  ),
  chart: (
    <>
      <path d="M2 13.5h12" />
      <path d="M4 11V7M8 11V3M12 11V9" />
    </>
  ),
  check: <path d="M3 8.5l3.5 3.5L13 5" />,
  key: (
    <>
      <circle cx="5" cy="11" r="2.75" />
      <path d="M7 9l6-6M11 5l1.5 1.5M9.5 6.5L11 8" />
    </>
  ),
  globe: (
    <>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M1.8 8h12.4M8 1.75c3.2 3.4 3.2 9.1 0 12.5-3.2-3.4-3.2-9.1 0-12.5z" />
    </>
  ),
  lock: (
    <>
      <rect x="3.25" y="7" width="9.5" height="6.5" rx="1.25" />
      <path d="M5.5 7V5a2.5 2.5 0 015 0v2" />
    </>
  ),
};

export type IconName = keyof typeof paths;

export function Icon({
  name,
  size = "sm",
  className,
}: {
  name: IconName;
  size?: "sm" | "md" | "lg";
  className?: string;
}) {
  const sizing = size === "md" ? "icon" : `icon icon-${size}`;
  return (
    <svg
      className={className ? `${sizing} ${className}` : sizing}
      viewBox="0 0 16 16"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}
