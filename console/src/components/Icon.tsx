import {
  Box, ChartColumn, Check, ChevronDown, ChevronUp, CircleAlert, CircleX, Clock, CornerUpRight, Database, Download, Ellipsis, Eye, EyeOff, File, GitBranch, Globe,
  Hammer, History, Info, KeyRound, Layers, Link, Lock, Plus, Route, Settings, Split, SquareTerminal, User, X, type LucideIcon,
} from "lucide-react";

/**
 * The console's icons by name. They are Lucide's, drawn the way kiso draws
 * its own (through `.icon`, at a 1.75 stroke), so a screen that mixes the two
 * shows one family.
 */
const icons = {
  plus: Plus,
  x: X,
  alert: CircleAlert,
  info: Info,
  box: Box,
  more: Ellipsis,
  database: Database,
  chart: ChartColumn,
  history: History,
  check: Check,
  clock: Clock,
  "x-circle": CircleX,
  key: KeyRound,
  globe: Globe,
  lock: Lock,
  download: Download,
  layers: Layers,
  file: File,
  branch: GitBranch,
  hammer: Hammer,
  user: User,
  terminal: SquareTerminal,
  eye: Eye,
  "eye-off": EyeOff,
  gear: Settings,
  route: Route,
  link: Link,
  split: Split,
  redirect: CornerUpRight,
  "chevron-down": ChevronDown,
  "chevron-up": ChevronUp,
} satisfies Record<string, LucideIcon>;

export type IconName = keyof typeof icons;

/** The drawing behind a name, for a component that takes Lucide's own, such as a diagram node. */
export function glyph(name: IconName): LucideIcon {
  return icons[name];
}

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
  const Glyph = icons[name];
  return <Glyph className={className ? `${sizing} ${className}` : sizing} strokeWidth={1.75} aria-hidden="true" focusable="false" />;
}
