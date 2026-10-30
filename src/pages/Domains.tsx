import { useState } from "react";
import { CheckCircle2, ExternalLink, Globe2, KeyRound, ShieldCheck } from "lucide-react";
import { tunnelFor, useStore } from "../store";
import { api } from "../lib/api";
import { publishPill } from "../lib/publish";
import { Button, Card, Confirm, Empty, Field, IconButton, Modal, PageHeader, Pill, cx } from "../components/ui";

export const TOKEN_STEPS = [
  "Open dash.cloudflare.com/profile/api-tokens and choose Create Token.",
  "Choose Create Custom Token.",
  "Add three permissions: Account, Cloudflare Tunnel, Edit. Zone, DNS, Edit. Zone, Zone, Read.",
  "Under Zone Resources, include only the domain you want to host. Under Account Resources, include your account.",
  "Create the token and paste it below. Cloudflare shows it only once.",
];

export function ConnectCloudflare({ onClose, onDone }: { onClose: () => void; onDone?: () => void }) {
  const toast = useStore((s) => s.toast); const refresh = useStore((s) => s.refresh);
  const [token, setToken] = useState(""); const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>();
  async function connect() {
    setBusy(true); setErr(undefined);
    try { const a = await api.cfConnect(token.trim()); await refresh(); toast("ok", `Connected to ${a.accountName ?? "Cloudflare"}`); onDone?.(); onClose(); }
    catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  }
  return (
    <Modal title="Connect Cloudflare" onClose={onClose} footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" busy={busy} disabled={token.trim().length < 20} onClick={connect}>Connect Cloudflare</Button></>}>
      <p className="mb-3 text-ink2">Hostify uses a Cloudflare API token limited to your domain. It never asks for your Cloudflare password.</p>
      <ol className="mb-4 grid gap-2">
        {TOKEN_STEPS.map((t, i) => (<li key={i} className="flex gap-2.5"><span className="grid h-5 w-5 shrink-0 place-items-center rounded-full bg-panel2 text-[12px] font-semibold">{i + 1}</span><span>{t}</span></li>))}
      </ol>
      <Button size="sm" className="mb-4" onClick={() => api.openExternal("https://dash.cloudflare.com/profile/api-tokens")}><ExternalLink size={13} aria-hidden />Open Cloudflare tokens</Button>
      <Field label="API token" type="password" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)}
        error={err} hint="Stored in Windows Credential Manager, not in Hostify's database." />
    </Modal>
  );
}

export function CloudflareCard() {
  const cf = useStore((s) => s.cf); const refresh = useStore((s) => s.refresh); const toast = useStore((s) => s.toast);
  const [connecting, setConnecting] = useState(false); const [disconnecting, setDisconnecting] = useState(false);
  return (
    <Card className="flex flex-wrap items-center justify-between gap-4 p-4">
      <div className="flex min-w-0 items-center gap-3.5">
        <div className={cx("grid h-10 w-10 shrink-0 place-items-center rounded-full", cf.connected ? "bg-kelpsoft text-kelp" : "bg-panel2 text-ink2")}>{cf.connected ? <ShieldCheck size={19} aria-hidden /> : <KeyRound size={19} aria-hidden />}</div>
        <div className="min-w-0"><div className="truncate font-semibold">{cf.connected ? cf.accountName ?? "Cloudflare connected" : "Cloudflare isn't connected"}</div>
          <div className="text-[13px] text-ink2">{cf.connected ? `API token ending in ${cf.tokenHint ?? "unknown"}, kept in Windows Credential Manager` : "Connect an account to put a website on your own domain."}</div></div>
      </div>
      {cf.connected
        ? <Button variant="danger" onClick={() => setDisconnecting(true)}>Disconnect Cloudflare</Button>
        : <Button variant="primary" onClick={() => setConnecting(true)}>Connect Cloudflare</Button>}
      {connecting && <ConnectCloudflare onClose={() => setConnecting(false)} />}
      {disconnecting && (
        <Confirm title="Disconnect Cloudflare?" action="Disconnect Cloudflare" onClose={() => setDisconnecting(false)}
          onConfirm={async () => { await api.cfDisconnect(); await refresh(); toast("info", "Cloudflare disconnected. Published websites stay online."); }}>
          <p>Hostify deletes its copy of the API token. Websites that are already published stay online, but you can't publish, unpublish or check them until you reconnect.</p>
        </Confirm>)}
    </Card>
  );
}

const DNS_LABEL: Record<string, { text: string; tone: "ok" | "wait" | "bad" | "idle" }> = {
  configured: { text: "Record in place", tone: "ok" }, pending: { text: "Waiting for DNS", tone: "wait" }, conflict: { text: "Conflict", tone: "bad" },
  error: { text: "Error", tone: "bad" }, unconfigured: { text: "Not configured", tone: "idle" },
};

export function Domains() {
  const domains = useStore((s) => s.domains); const sites = useStore((s) => s.sites); const tunnels = useStore((s) => s.tunnels);
  const go = useStore((s) => s.go); const openWizard = useStore((s) => s.openWizard);
  return (
    <div className="mx-auto grid max-w-[1040px] gap-5">
      <PageHeader title="Domains" sub="Your Cloudflare connection and the domains your websites use." />
      <CloudflareCard />
      <Card className="overflow-x-auto">
        {domains.length === 0 ? (
          <Empty icon={<Globe2 size={20} />} title="No domains connected" body="Publish a website and choose a domain such as www.example.com. Its DNS, tunnel and HTTPS status appear here."
            action={sites.length > 0 ? <Button variant="primary" onClick={() => openWizard(sites[0].id)}>Publish {sites[0].name}</Button> : undefined} />
        ) : (
          <table className="w-full min-w-[640px] text-left">
            <thead className="border-b border-line text-[13px] text-ink2"><tr><th className="px-4 py-2.5 font-medium">Domain</th><th className="font-medium">Website</th><th className="font-medium">DNS</th><th className="font-medium">Tunnel</th><th className="font-medium">HTTPS</th><th className="w-12"><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>{domains.map((d) => {
              const site = sites.find((s) => s.id === d.siteId); const dns = DNS_LABEL[d.dnsStatus] ?? DNS_LABEL.unconfigured;
              const tunnel = site && tunnelFor(tunnels, site.id); const pp = publishPill(site?.publishState);
              return (<tr key={d.id} className="border-b border-line last:border-0">
                <td className="selectable px-4 py-3 font-medium">{d.hostname}</td>
                <td>{site ? <button type="button" className="press rounded text-left hover:text-kelp" onClick={() => go("websites", site.id)}>{site.name}</button> : <span className="text-ink3">None</span>}</td>
                <td><Pill tone={dns.tone}>{dns.text}</Pill></td>
                <td>{!site ? <span className="text-ink3">None</span> : tunnel ? <Pill tone={tunnel.connected ? "ok" : "bad"}>{tunnel.connected ? `${tunnel.connections} connections` : "Disconnected"}</Pill> : <span className="text-ink3">Not running</span>}</td>
                <td>{d.httpsVerified ? <span className="inline-flex items-center gap-1.5 text-kelp"><CheckCircle2 size={15} aria-hidden />Verified</span> : site?.publishState ? <Pill tone={pp.tone} busy={pp.busy}>{pp.text}</Pill> : <span className="text-ink3">Not verified</span>}</td>
                <td className="pr-2">{d.httpsVerified && <IconButton label={`Open ${d.hostname}`} onClick={() => api.openExternal(`https://${d.hostname}`)}><ExternalLink size={15} /></IconButton>}</td>
              </tr>);
            })}</tbody>
          </table>
        )}
      </Card>
    </div>
  );
}
