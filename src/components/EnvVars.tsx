import { useCallback, useEffect, useState } from "react";
import { Copy, Eye, EyeOff, KeyRound, Plus, Trash2 } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { Button, Confirm, Field, IconButton, Modal, Pill, Skeleton } from "./ui";
import type { EnvVar, Site } from "../lib/types";

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;
const LOOKS_SECRET = /SECRET|TOKEN|PASSWORD|PASS|KEY|PRIVATE|CREDENTIAL/i;
const RESTART = "Restart the website to use this change.";

/** A secret reaches this screen only after an explicit Show or Copy, and is dropped again on Hide. */
function Row({ site, v, onChanged, onRestart }: { site: Site; v: EnvVar; onChanged: () => void; onRestart: () => void }) {
  const toast = useStore((s) => s.toast);
  const [shown, setShown] = useState<string>(); const [editing, setEditing] = useState(false); const [removing, setRemoving] = useState(false);
  async function copy() {
    try { await navigator.clipboard.writeText(v.secret ? await api.revealEnvVar(site.id, v.name) : v.value ?? ""); toast("info", `${v.name} copied to the clipboard`); }
    catch (e) { toast("error", (e as Error).message); }
  }
  async function toggle() {
    if (shown !== undefined) { setShown(undefined); return; }
    try { setShown(await api.revealEnvVar(site.id, v.name)); } catch (e) { toast("error", (e as Error).message); }
  }
  const text = v.secret ? shown ?? "••••••••" : v.value;
  return (
    <li className="flex flex-wrap items-center gap-2 py-2">
      <span className="selectable min-w-0 break-all font-mono text-[13px] font-semibold">{v.name}</span>
      {v.secret && <Pill tone="idle">Secret</Pill>}
      <span className="selectable min-w-0 flex-1 break-all font-mono text-[13px] text-ink2">{text}</span>
      <div className="flex items-center">
        {v.secret && <IconButton label={`${shown !== undefined ? "Hide" : "Show"} ${v.name}`} onClick={toggle}>{shown !== undefined ? <EyeOff size={15} /> : <Eye size={15} />}</IconButton>}
        <IconButton label={`Copy ${v.name}`} onClick={copy}><Copy size={15} /></IconButton>
        <Button size="sm" variant="ghost" onClick={() => setEditing(true)}>Edit</Button>
        <IconButton label={`Delete ${v.name}`} onClick={() => setRemoving(true)}><Trash2 size={15} /></IconButton>
      </div>
      {editing && <VarForm site={site} initial={v} onClose={() => setEditing(false)} onSaved={(restart) => { setShown(undefined); onChanged(); if (restart) onRestart(); }} />}
      {removing && (
        <Confirm title={`Delete ${v.name}?`} action={`Delete ${v.name}`} onClose={() => setRemoving(false)}
          onConfirm={async () => { const r = await api.deleteEnvVar(site.id, v.name); onChanged(); if (r.restartRequired) onRestart(); }}>
          <p>The website stops receiving this variable the next time it starts.</p>
        </Confirm>)}
    </li>
  );
}

function VarForm({ site, initial, onClose, onSaved }: { site: Site; initial?: EnvVar; onClose: () => void; onSaved: (restart: boolean) => void }) {
  const [name, setName] = useState(initial?.name ?? ""); const [value, setValue] = useState(initial?.secret ? "" : initial?.value ?? "");
  const [secret, setSecret] = useState(initial?.secret ?? false); const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>();
  const nameOk = NAME_RE.test(name);
  async function save(e: React.FormEvent) {
    e.preventDefault(); setBusy(true); setErr(undefined);
    try { const r = await api.setEnvVar(site.id, name, value, secret); onSaved(r.restartRequired); onClose(); }
    catch (x) { setErr((x as Error).message); setBusy(false); }
  }
  return (
    <Modal title={initial ? `Edit ${initial.name}` : "Add a variable"} onClose={onClose}>
      <form className="grid gap-4" onSubmit={save}>
        <Field label="Name" value={name} disabled={!!initial} spellCheck={false} autoCapitalize="off" placeholder="API_URL"
          onChange={(e) => { setName(e.target.value); if (!initial && LOOKS_SECRET.test(e.target.value)) setSecret(true); }} onBlur={() => setTouched(true)}
          error={touched && name && !nameOk ? "Use letters, numbers and underscores, starting with a letter or underscore." : undefined} />
        <Field label="Value" value={value} type={secret ? "password" : "text"} spellCheck={false} autoComplete="off" onChange={(e) => setValue(e.target.value)}
          hint={initial?.secret ? "The current value stays hidden. Enter a new value to replace it." : undefined} />
        <label className="flex items-start gap-2.5"><input type="checkbox" className="mt-1" checked={secret} onChange={(e) => setSecret(e.target.checked)} />
          <span>Mark as secret<span className="block text-[13px] text-ink2">Encrypted for your Windows account, hidden in Hostify, and removed from captured logs.</span></span></label>
        {err && <p role="alert" className="selectable text-buoy">{err}</p>}
        <div className="flex justify-end gap-2"><Button onClick={onClose}>Cancel</Button><Button type="submit" variant="primary" busy={busy} disabled={!nameOk}>Save variable</Button></div>
      </form>
    </Modal>
  );
}

function ImportDialog({ site, onClose, onDone }: { site: Site; onClose: () => void; onDone: (restart: boolean) => void }) {
  const toast = useStore((s) => s.toast);
  const [text, setText] = useState(""); const [busy, setBusy] = useState(false); const [errors, setErrors] = useState<{ line: number; message: string }[]>(); const [err, setErr] = useState<string>();
  async function run() {
    setBusy(true); setErr(undefined); setErrors(undefined);
    try {
      const r = await api.importEnv(site.id, text); setText("");
      toast(r.imported ? "ok" : "info", `${r.imported} ${r.imported === 1 ? "variable" : "variables"} imported`);
      if (r.errors.length) { setErrors(r.errors); onDone(r.restartRequired); } else { onDone(r.restartRequired); onClose(); }
    } catch (x) { setErr((x as Error).message); } finally { setBusy(false); }
  }
  return (
    <Modal title="Import a .env file" onClose={onClose}>
      <div className="grid gap-3">
        <label className="grid gap-1.5"><span className="text-[13px] font-medium">Paste the contents of your .env file</span>
          <textarea value={text} onChange={(e) => setText(e.target.value)} rows={8} spellCheck={false} autoComplete="off"
            className="selectable rounded-ctl border border-line bg-chalk p-3 font-mono text-[13px] text-ink focus:border-kelp" /></label>
        <p className="text-[13px] text-ink2">Names containing words like KEY, TOKEN or PASSWORD are saved as secrets.</p>
        {errors && <div role="alert" className="rounded-ctl border border-buoy bg-buoysoft p-3 text-[13px]"><p className="font-medium">These lines weren't imported</p>
          <ul className="mt-1">{errors.map((e) => <li key={e.line}>Line {e.line}: {e.message}</li>)}</ul></div>}
        {err && <p role="alert" className="selectable text-buoy">{err}</p>}
        <div className="flex justify-end gap-2"><Button onClick={onClose}>Close</Button><Button variant="primary" busy={busy} disabled={!text.trim()} onClick={run}>Import variables</Button></div>
      </div>
    </Modal>
  );
}

export function EnvVars({ site, onRestartNeeded }: { site: Site; onRestartNeeded?: () => void }) {
  const toast = useStore((s) => s.toast);
  const [vars, setVars] = useState<EnvVar[]>(); const [err, setErr] = useState<string>(); const [adding, setAdding] = useState(false); const [importing, setImporting] = useState(false); const [restart, setRestart] = useState(false);
  const load = useCallback(() => api.listEnvVars(site.id).then((v) => { setVars(v); setErr(undefined); }).catch((e) => setErr((e as Error).message)), [site.id]);
  useEffect(() => { void load(); }, [load]);
  const needRestart = () => { setRestart(true); onRestartNeeded?.(); };
  async function exportFile() {
    try { await navigator.clipboard.writeText(await api.exportEnv(site.id)); toast("info", "Non-secret variables copied to the clipboard. Secrets are left out."); }
    catch (e) { toast("error", (e as Error).message); }
  }
  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center justify-end gap-1.5">
        <Button size="sm" variant="ghost" onClick={exportFile} disabled={!vars?.length}>Export non-secret</Button>
        <Button size="sm" variant="ghost" onClick={() => setImporting(true)}>Import .env</Button>
        <Button size="sm" onClick={() => setAdding(true)}><Plus size={13} aria-hidden />Add variable</Button>
      </div>
      {restart && <p role="status" className="rounded-ctl bg-lanternsoft p-2.5 text-[13px] text-lantern">{RESTART}</p>}
      {err && <p role="alert" className="selectable text-[13px] text-buoy">{err}</p>}
      {!vars && !err && <Skeleton className="h-16" />}
      {vars && vars.length === 0 && <p className="flex items-center gap-2 py-3 text-ink2"><KeyRound size={15} aria-hidden />No variables yet. Your app reads these as environment variables.</p>}
      {vars && vars.length > 0 && <ul className="divide-y divide-line">{vars.map((v) => <Row key={v.name} site={site} v={v} onChanged={load} onRestart={needRestart} />)}</ul>}
      {adding && <VarForm site={site} onClose={() => setAdding(false)} onSaved={(r) => { void load(); if (r) needRestart(); }} />}
      {importing && <ImportDialog site={site} onClose={() => setImporting(false)} onDone={(r) => { void load(); if (r) needRestart(); }} />}
    </div>
  );
}
