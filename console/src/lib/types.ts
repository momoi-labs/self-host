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
  services?: ServiceState[];
};

export type SystemContainer = {
  role: string;
  name: string;
  image: string;
  status: string;
  restarts?: number | null;
};

/** A service on the Overview is an Application or a Platform Infra container. */
export type Service = (App | SystemContainer) & { role?: string };

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
