import { failureOf } from "./api.js";

type Json = Record<string, unknown>;

export type AccountStatus = {
  exists: boolean;
  usable: boolean;
  canonical: boolean;
  console_url: string;
  root_key_retired: boolean;
};

export type Passkey = { id: string; name: string; created_at: string; last_used_at?: string };

export function fromBase64Url(text: string): ArrayBuffer {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64 + "=".repeat((4 - (base64.length % 4)) % 4));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

export function toBase64Url(buffer: ArrayBuffer | null | undefined): string | undefined {
  if (!buffer) return undefined;
  let binary = "";
  for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function withIds(list: unknown): PublicKeyCredentialDescriptor[] | undefined {
  if (!Array.isArray(list)) return undefined;
  return list.map((item: Json) => ({ ...item, id: fromBase64Url(item.id as string) }) as PublicKeyCredentialDescriptor);
}

export function creationOptions(options: { publicKey: Json }): PublicKeyCredentialCreationOptions {
  const key = options.publicKey;
  const user = key.user as Json;
  return {
    ...key,
    challenge: fromBase64Url(key.challenge as string),
    user: { ...user, id: fromBase64Url(user.id as string) },
    excludeCredentials: withIds(key.excludeCredentials),
  } as PublicKeyCredentialCreationOptions;
}

export function requestOptions(options: { publicKey: Json }): PublicKeyCredentialRequestOptions {
  const key = options.publicKey;
  return {
    ...key,
    challenge: fromBase64Url(key.challenge as string),
    allowCredentials: withIds(key.allowCredentials),
  } as PublicKeyCredentialRequestOptions;
}

export function registrationJson(credential: PublicKeyCredential): Json {
  const response = credential.response as AuthenticatorAttestationResponse;
  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: toBase64Url(response.attestationObject),
      clientDataJSON: toBase64Url(response.clientDataJSON),
      transports: response.getTransports?.() ?? undefined,
    },
    extensions: credential.getClientExtensionResults(),
  };
}

export function assertionJson(credential: PublicKeyCredential): Json {
  const response = credential.response as AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: toBase64Url(response.authenticatorData),
      clientDataJSON: toBase64Url(response.clientDataJSON),
      signature: toBase64Url(response.signature),
      userHandle: toBase64Url(response.userHandle),
    },
    extensions: credential.getClientExtensionResults(),
  };
}

export class PasskeyError extends Error {}

export function ceremonyMessage(cause: unknown): string {
  if (cause instanceof PasskeyError) return cause.message;
  const name = (cause as { name?: string } | null)?.name;
  if (name === "NotAllowedError") return "The passkey prompt was closed or timed out. Try again.";
  if (name === "InvalidStateError") return "This device already holds a passkey for this account. Use another device or password manager.";
  if (name === "SecurityError") return "The browser refused the passkey for this address. Open the console at its own address.";
  return "Could not reach the Host. Check your connection and try again.";
}

async function post(path: string, body: unknown, headers: Record<string, string> = {}): Promise<Json> {
  const res = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new PasskeyError((await failureOf(res)).error);
  const text = await res.text();
  return text ? (JSON.parse(text) as Json) : {};
}

async function create(options: unknown): Promise<Json> {
  const credential = (await navigator.credentials.create({
    publicKey: creationOptions(options as { publicKey: Json }),
  })) as PublicKeyCredential | null;
  if (!credential) throw new PasskeyError("No passkey was created. Try again.");
  return registrationJson(credential);
}

async function assert(options: unknown): Promise<Json> {
  const credential = (await navigator.credentials.get({
    publicKey: requestOptions(options as { publicKey: Json }),
  })) as PublicKeyCredential | null;
  if (!credential) throw new PasskeyError("No passkey answered. Try again.");
  return assertionJson(credential);
}

export async function accountStatus(): Promise<AccountStatus> {
  const res = await fetch("/account/status");
  if (!res.ok) throw new PasskeyError((await failureOf(res)).error);
  return (await res.json()) as AccountStatus;
}

export async function setUpPasskey(key: string, name: string): Promise<Passkey> {
  const headers = { Authorization: "Bearer " + key };
  const started = await post("/account/setup/start", { name }, headers);
  const credential = await create(started.options);
  return (await post("/account/setup/finish", { registration: started.registration, credential }, headers)) as Passkey;
}

export async function logIn(): Promise<{ retired_root_key: boolean }> {
  const started = await post("/account/login/start", undefined);
  const credential = await assert(started.options);
  return (await post("/account/login/finish", { login: started.login, credential })) as { retired_root_key: boolean };
}

export type StepUpProof = { id: string; credential: Json };

export async function stepUp(request: Json): Promise<StepUpProof> {
  const started = await post("/account/step-up", request);
  const credential = await assert(started.options);
  return { id: started.step_up as string, credential };
}

export async function addPasskey(name: string): Promise<Passkey> {
  const step_up = await stepUp({ purpose: "add_passkey", name });
  const started = await post("/account/passkeys/start", { name, step_up });
  const credential = await create(started.options);
  return (await post("/account/passkeys/finish", { registration: started.registration, credential })) as Passkey;
}

export async function createKey(request: Json, powerful: boolean): Promise<Json> {
  const step_up = powerful ? await stepUp({ purpose: "create_key", key: request }) : undefined;
  return post("/api-keys", step_up ? { ...request, step_up } : request);
}

export function needsPasskey(request: { kind: string; scopes?: string[] }): boolean {
  return request.kind === "human" || Boolean(request.scopes?.includes("host"));
}
