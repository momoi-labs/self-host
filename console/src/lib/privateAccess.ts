import type { App, NetworkPolicy } from "./types.js";

/** The Applications granted access to this one; none for a shared or unset policy. */
export function consumersOf(app: App): string[] {
  return app.network_policy?.kind === "private" ? app.network_policy.consumers : [];
}

/**
 * Whether this Application can grant others access: an unpublished Compose
 * Application in a container, as the API requires (src/connectivity.rs).
 */
export function canGrant(app: App): boolean {
  return app.publication?.kind === "unpublished"
    && app.runtime?.kind !== "native"
    && (!!app.compose || !!app.git?.compose_path);
}

/** Who may be granted access: any other container Application. The API refuses native ones. */
export function grantable(provider: App, apps: App[]): App[] {
  return apps.filter((app) => app.id !== provider.id && app.runtime?.kind !== "native");
}

/** The Applications that granted this one access. */
export function providersOf(app: App, apps: App[]): App[] {
  return apps.filter((provider) => provider.id !== app.id && consumersOf(provider).includes(app.id));
}

/**
 * The address a consumer uses for each service of a provider: the service's
 * container name, `sf-app-<provider-id>-<service>`, on the provider's private
 * network. Known once the provider has run.
 */
export function addressesOf(provider: App): { service: string; address: string }[] {
  return (provider.services ?? []).map((service) => ({ service: service.service, address: service.container }));
}

/** The policy an edit should send, or nothing when the grants did not change. */
export function policyChange(app: App, consumers: string[]): NetworkPolicy | undefined {
  const before = consumersOf(app);
  const same = before.length === consumers.length && before.every((id, index) => id === consumers[index]);
  return same ? undefined : { kind: "private", consumers };
}
