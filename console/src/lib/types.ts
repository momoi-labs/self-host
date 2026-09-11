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

export type Metrics = {
  interval_seconds: number;
  host_cpus: number;
  applications: AppSeries[];
  platform: PlatformSample[];
  proxy: HostTraffic[];
  dns: { queries_total: number; by_name: { name: string; queries: number }[] };
};
