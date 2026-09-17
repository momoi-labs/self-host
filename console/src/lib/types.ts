import type { ImageDependency } from "./customImageTemplates.js";

/** Every failure reaches the console as this shape (ADR-0010). */
export type Report = {
  error: string;
  caused_by: string[];
};

export type ServiceState = {
  service: string;
  container: string;
  state: string;
  exit_code?: number;
  restarts?: number;
};

export type App = {
  id: string;
  name: string;
  image: string;
  hostname: string;
  status: string;
  source?: string;
  aliases?: string[];
  restarts?: number | null;
  last_error?: Report | string | null;
  compose?: string;
  web_service?: string;
  web_port?: number;
  development?: DevelopmentApplication;
  services?: ServiceState[];
  /** HTTP server readiness of the configured Web Target, separate from status. */
};

export type HttpReadiness = "responding" | "error" | "unreachable" | "unknown";

export type DevelopmentApplication = {
  image_id: string;
  tag: string;
  command: string;
  web_port: number;
  persist_data: boolean;
};

/** A saved custom image recipe and its latest build on this Host. A
 * `dockerfile` means the Operator owns the file and the builder is off
 * (ADR-0022). */
export type CustomImage = {
  id: string;
  name: string;
  template_id?: string | null;
  dependencies: { tool: string; version: string; allow_builds?: string[] }[];
  setup?: string[];
  build_checks?: string[];
  dockerfile?: string | null;
  image: string;
  status: "building" | "ready" | "failed";
  last_error: Report | null;
  log: string;
  in_use?: boolean | null;
};

export type ComposePort = { container: number | string };

export type ComposeVolume = {
  kind: "data" | "named" | "host" | "anonymous";
  target: string;
  source?: string;
  data_path?: string;
};

export type ComposeService = {
  name: string;
  ports: ComposePort[];
  volumes: ComposeVolume[];
};

export type Inspection = {
  services: ComposeService[];
  web_service?: string;
};

export type ApiKey = {
  id: string;
  label: string;
  created_at: string;
};

/** One minute of one scope: an Application, or one of its containers
 * (ADR-0020). */
export type AppSample = {
  at: number;
  cpu_percent: number;
  memory_bytes: number;
  memory_limit_bytes: number;
  rx_bytes: number;
  tx_bytes: number;
};

/** What the Platform's own proxy and DNS did during one interval. */
export type PlatformSample = {
  at: number;
  proxy_requests: number;
  proxy_errors: number;
  dns_queries: number;
};

export type HostTraffic = {
  hostname: string;
  requests: number;
  errors: number;
};

export type ContainerSeries = {
  container: string;
  samples: AppSample[];
};

export type AppSeries = {
  id: string;
  samples: AppSample[];
  containers: ContainerSeries[];
};

/** A Virtual machine measures itself from inside, so unlike an Application
 * there is nothing to break down into. */
export type MachineSeries = {
  id: string;
  samples: AppSample[];
};

export type Metrics = {
  interval_seconds: number;
  host_cpus: number;
  applications: AppSeries[];
  machines: MachineSeries[];
  platform: PlatformSample[];
  proxy: HostTraffic[];
  dns: { queries_total: number; by_name: { name: string; queries: number }[] };
};

/** A Virtual machine's saved settings. `applied_config` is what the Host has
 * actually installed; `config` is what the Operator last saved.
 *
 * The Platform omits an empty list instead of sending `[]`, so these arrays
 * are only guaranteed after `normalizeConfig` has put them back. */
export type EnvironmentConfig = {
  name: string;
  cpus: number;
  memory_gib: number;
  disk_gib: number;
  ssh_public_key: string;
  recipe: {
    name: string;
    template_id?: string | null;
    dependencies: ImageDependency[];
    setup: string[];
    build_checks: string[];
  };
  command: string;
  web_port: number;
};

/** A durable lifecycle action. It outlives the request that started it, so an
 * interrupted one is still here to retry. */
export type Operation = {
  action: string;
  status: string;
  step?: string | null;
  error?: Report | null;
} | null;

export type Environment = {
  id: string;
  config: EnvironmentConfig;
  applied_config?: EnvironmentConfig;
  state: string;
  service_ready: boolean;
  operation: Operation;
  log: string;
  ssh_command: string;
  tunnel_command?: string;
  web_url: string | null;
  hostname?: string;
  mac_address?: string | null;
  lan_address?: string | null;
  installed_versions?: unknown;
};
