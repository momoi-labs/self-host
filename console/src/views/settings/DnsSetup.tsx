import { useQuery } from "@tanstack/react-query";
import { KV, KVKey, KVValue, Link } from "@momoi-labs/kiso-react";

import { bootstrapStatusQuery } from "../../lib/queries.js";
import type { BootstrapStatus } from "../../lib/types.js";
import type { PaneProps } from "./General.js";
import { SettingCard } from "./SettingCard.js";

/** `undefined` while loading, `null` when the Platform did not answer. */
function useBootstrapStatus(): BootstrapStatus | null | undefined {
  const { data, isError } = useQuery(bootstrapStatusQuery);
  return data ?? (isError ? null : undefined);
}

/** The Host a device on the LAN points its DNS at. */
export function YourHost({ id }: PaneProps) {
  const settings = useBootstrapStatus();
  const suffix = settings?.dns_suffix;
  const hostIp = settings?.host_ip;
  const hostAddresses = settings?.host_addresses ?? [];

  return (
    <SettingCard id={id} title="Your Host">
      <p className="muted t-label">Use this Host IP as the DNS server on your LAN devices.</p>
      <KV>
        <KVKey>Host IP</KVKey>
        <KVValue>{hostIp || (settings === undefined ? "Loading…" : "Unavailable")}</KVValue>
        {hostAddresses.length > 1 ? (
          <>
            <KVKey>All host addresses</KVKey>
            <KVValue>{hostAddresses.join(", ")}</KVValue>
          </>
        ) : null}
        <KVKey>DNS Suffix</KVKey>
        <KVValue>{suffix || (settings === undefined ? "Loading…" : "Unavailable")}</KVValue>
      </KV>
      {hostIp ? (
        <p>
          <Link href={`http://${hostIp.includes(":") ? `[${hostIp}]` : hostIp}/setup`}>Connect a device</Link>
        </p>
      ) : null}
    </SettingCard>
  );
}
