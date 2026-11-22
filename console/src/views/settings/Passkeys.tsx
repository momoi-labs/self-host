import { useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Alert,
  AlertDescription,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  FormField,
  Table,
  TableBody,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../../components/Icon.js";
import { api, failureOf, readJson } from "../../lib/api.js";
import { formatAgo } from "../../lib/format.js";
import { addPasskey, ceremonyMessage, type Passkey } from "../../lib/passkey.js";
import type { PaneProps } from "./General.js";
import { SettingCard } from "./SettingCard.js";

type Account = { passkeys: Passkey[]; console_url: string };

export function Passkeys({ id }: PaneProps) {
  const { data, refetch } = useQuery({
    queryKey: ["account"],
    queryFn: ({ signal }) => readJson<Account>("/account", signal),
  });
  const passkeys = data?.passkeys ?? [];
  const last = passkeys.length === 1;
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  async function add(event: FormEvent) {
    event.preventDefault();
    if (pending) return;
    setError(null);
    setPending(true);
    try {
      await addPasskey(name.trim());
      setName("");
      setAdding(false);
      await refetch();
    } catch (cause) {
      setError(ceremonyMessage(cause));
    } finally {
      setPending(false);
    }
  }

  async function remove(passkey: Passkey) {
    if (!confirm(`Remove the passkey "${passkey.name}"? It can no longer log in.`)) return;
    setRemoveError(null);
    const res = await api(`/account/passkeys/${encodeURIComponent(passkey.id)}`, { method: "DELETE" });
    if (!res.ok) setRemoveError((await failureOf(res)).error);
    await refetch();
  }

  return (
    <SettingCard id={id} title="Passkeys" actions={
      <Button size="sm" variant="primary" onClick={() => setAdding(true)}>
        <Icon name="plus" />
        Add passkey
      </Button>
    }>
      <p className="muted t-label">Passkeys log you in to the console. Keep a second one as a backup.</p>
      {last ? (
        <Alert variant="info">
          <Icon name="info" size="md" />
          <AlertDescription>This is your only passkey, so it cannot be removed. Add another one first.</AlertDescription>
        </Alert>
      ) : null}
      {removeError ? (
        <Alert variant="error">
          <Icon name="alert" size="md" />
          <AlertDescription>{removeError}</AlertDescription>
        </Alert>
      ) : null}
      <TableFrame frame="none">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">Passkey</TableHead>
              <TableHead scope="col">Last used</TableHead>
              <TableHead scope="col">Created</TableHead>
              <TableHead scope="col" className="col-tight">Action</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {passkeys.map((passkey) => (
              <TableRow key={passkey.id}>
                <TableCell>{passkey.name}</TableCell>
                <TableCell>{passkey.last_used_at ? formatAgo(passkey.last_used_at) : <span className="muted">Never</span>}</TableCell>
                <TableCell className="key-date">{passkey.created_at.slice(0, 10)}</TableCell>
                <TableCell className="col-tight">
                  {last ? null : (
                    <Button size="sm" variant="ghost" className="btn-danger-ghost" onClick={() => void remove(passkey)}>
                      Remove
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableFrame>

      <Dialog open={adding} onOpenChange={(open) => { setAdding(open); if (!open) setError(null); }}>
        <DialogContent className="key-dialog">
          <DialogHeader>
            <DialogTitle>Add a passkey</DialogTitle>
            <DialogDescription>Confirm with a passkey you have, then create the new one.</DialogDescription>
          </DialogHeader>
          <form id="new-passkey" onSubmit={add}>
            <DialogBody>
              <FormField label="Name" id="passkey-name" placeholder="YubiKey" hint="Name it after where it lives." required autoFocus value={name} onChange={(event) => setName(event.target.value)} />
              <Alert variant="info">
                <Icon name="info" size="md" />
                <AlertDescription>Your browser asks twice: once to confirm with a passkey you have, once to create the new one.</AlertDescription>
              </Alert>
              {error ? (
                <Alert variant="error">
                  <Icon name="alert" size="md" />
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              ) : null}
            </DialogBody>
            <DialogFooter>
              <Button size="sm" type="button" onClick={() => setAdding(false)}>Cancel</Button>
              <Button size="sm" variant="primary" type="submit" disabled={pending}>Confirm with passkey</Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </SettingCard>
  );
}
