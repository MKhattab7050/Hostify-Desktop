import { useState } from "react";
import { CheckCircle2, ExternalLink, Globe2, KeyRound, ShieldCheck } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { Button, Card, Empty, Field, Modal, cx } from "../components/ui";

export const TOKEN_STEPS = [
  "Open dash.cloudflare.com/profile/api-tokens and choose Create Token.",
  "Choose Create Custom Token.",
  "Add three permissions: Account · Cloudflare Tunnel · Edit, Zone · DNS · Edit, and Zone · Zone · Read.",
  "Under Zone Resources, include only the domain you want to host. Under Account Resources, include your account.",
  "Create the token and paste it below. Cloudflare shows it only once.",
];

export function ConnectCloudflare({ onClose, onDone }: { onClose: () => void; onDone?: () => void }) {
  const { toast, refresh } = useStore();
  const [token, setToken] = useState(""); const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>();
  async function connect() {
    setBusy(true); setErr(undefined);
    try { const a = await api.cfConnect(token.trim()); await refresh(); toast("ok", `Connected to ${a.accountName ?? "Cloudflare"}`); onDone?.(); onClose(); }
    catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  }
  return (
    <Modal title="Connect Cloudflare" onClose={onClose} footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" busy={busy} disabled={token.trim().length < 20} onClick={connect}>Connect Cloudflare</Button></>}>
      <p className="mb-3 text-ink2">Hostify uses a Cloudflare API token limited to your domain. It never asks for your Cloudflare password.</p>
      <ol className="mb-5 grid gap-2">
        {TOKEN_STEPS.map((t, i) => (<li key={i} className="flex gap-3"><span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-panel2 text-[12px] font-semibold">{i + 1}</span><span>{t}</span></li>))}
      </ol>
      <Button size="sm" className="mb-5" onClick={() => api.openExternal("https://dash.cloudflare.com/profile/api-tokens")}><ExternalLink size={13} aria-hidden />Open Cloudflare tokens</Button>
      <Field label="API token" type="password" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)}
        error={err} hint="Stored in Windows Credential Manager, not in Hostify's database." />
    </Modal>
  );
}

export function Domains() {
  const { cf, domains, sites, refresh, toast } = useStore();
  const [connecting, setConnecting] = useState(false);
  const label: Record<string, [string, string]> = {
    configured: ["DNS configured", "text-kelp"], pending: ["Waiting for DNS", "text-lantern"], conflict: ["DNS conflict", "text-buoy"],
    error: ["Error", "text-buoy"], unconfigured: ["Not configured", "text-ink3"],
  };
  return (
    <div className="mx-auto grid max-w-[1100px] gap-5">
      <h1 className="text-2xl font-bold">Domains</h1>
      <Card className="flex flex-wrap items-center justify-between gap-4 p-5">
        <div className="flex items-center gap-4">
          <div className={cx("grid h-11 w-11 place-items-center rounded-full", cf.connected ? "bg-kelpsoft text-kelp" : "bg-panel2 text-ink2")}>{cf.connected ? <ShieldCheck size={20} /> : <KeyRound size={20} />}</div>
          <div><div className="font-semibold">{cf.connected ? `Cloudflare: ${cf.accountName ?? "connected"}` : "Cloudflare is not connected"}</div>
            <div className="text-ink2">{cf.connected ? `Token ${cf.tokenHint ?? ""} · limited to the zones you chose` : "Connect an account to put a website on your own domain."}</div></div>
        </div>
        {cf.connected
          ? <Button variant="danger" onClick={async () => { try { await api.cfDisconnect(); await refresh(); toast("info", "Cloudflare disconnected. Published sites keep running until you remove them."); } catch (e) { toast("error", (e as Error).message); } }}>Disconnect Cloudflare</Button>
          : <Button variant="primary" onClick={() => setConnecting(true)}>Connect Cloudflare</Button>}
      </Card>

      <Card>
        {domains.length === 0 ? (
          <Empty icon={<Globe2 size={22} />} title="No domains connected" body="Publish a website and choose a domain such as www.example.com. It shows up here with its DNS and HTTPS status." />
        ) : (
          <table className="w-full text-left">
            <thead className="border-b border-line text-[13px] text-ink3"><tr><th className="px-5 py-3 font-medium">Domain</th><th className="font-medium">Website</th><th className="font-medium">DNS</th><th className="pr-5 font-medium">HTTPS</th></tr></thead>
            <tbody>{domains.map((d) => {
              const [t, c] = label[d.dnsStatus] ?? label.unconfigured;
              return (<tr key={d.id} className="border-b border-line last:border-0">
                <td className="selectable px-5 py-3.5 font-medium">{d.hostname}</td>
                <td>{sites.find((s) => s.id === d.siteId)?.name ?? "—"}</td>
                <td className={c}>{t}</td>
                <td className="pr-5">{d.httpsVerified ? <span className="inline-flex items-center gap-1.5 text-kelp"><CheckCircle2 size={15} />Verified</span> : <span className="text-ink3">Not verified</span>}</td>
              </tr>);
            })}</tbody>
          </table>
        )}
      </Card>
      {connecting && <ConnectCloudflare onClose={() => setConnecting(false)} />}
    </div>
  );
}
