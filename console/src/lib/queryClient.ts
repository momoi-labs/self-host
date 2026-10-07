import { QueryClient } from "@tanstack/react-query";

/**
 * What the console has read from the Platform, shared by every screen that
 * reads the same thing. A failed read shows at once and is asked again on the
 * screen's next poll, so nothing retries in between. The Host is on the LAN,
 * and the browser's online flag says nothing about whether it answers.
 */
export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, networkMode: "always" } },
});
