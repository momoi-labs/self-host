import { useQuery } from "@tanstack/react-query";
import { readJson } from "./api.js";

type NativeCapabilities = {
  available: boolean;
  resource_limits: boolean;
  metrics: boolean;
  terminal: boolean;
  managed_postgres: boolean;
  reason: string | null;
};

/** The Host's native capabilities, read once per page and shared by every screen. */
export function useNativeCapabilities() {
  const { data, isError } = useQuery({
    queryKey: ["native", "capabilities"],
    queryFn: ({ signal }) => readJson<NativeCapabilities>("/native/capabilities", signal),
    staleTime: Infinity,
  });
  return { capabilities: data ?? null, error: isError };
}
