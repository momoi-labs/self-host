export type LoginFailure = { kind: "key" | "host"; message: string };
type LoginResult = { kind: "accepted"; key: string } | LoginFailure;

export async function checkApiKey(value: string, request: typeof fetch = fetch): Promise<LoginResult> {
  const key = value.trim();
  if (!key) return { kind: "key", message: "Paste your API key to continue." };
  if (!/^[\x21-\x7e]+$/.test(key)) {
    return { kind: "key", message: "The API key must contain only visible ASCII characters, without spaces or line breaks." };
  }
  let response: Response;
  try {
    response = await request("/health", { headers: { Authorization: "Bearer " + key } });
  } catch {
    return { kind: "host", message: "Could not reach the Host. Check your LAN connection and try again." };
  }
  if (response.ok) return { kind: "accepted", key };
  if (response.status === 401 || response.status === 403) {
    return { kind: "key", message: "This key was not accepted. Check that it belongs to this Host and try again." };
  }
  return { kind: "host", message: "The Host could not check your key. Wait a moment and try again." };
}
