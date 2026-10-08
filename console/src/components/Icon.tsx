import {
  Box, ChartColumn, Check, CircleAlert, CircleX, Clock, Database, Download, Ellipsis, Eye, EyeOff, File, GitBranch, Globe,
  Hammer, History, Info, KeyRound, Layers, Lock, Plus, Route, Settings, SquareTerminal, User, X, type LucideIcon,
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
} satisfies Record<string, LucideIcon>;

export type IconName = keyof typeof icons;

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
