import { useState, type ReactNode } from "react";
import { Clipboard } from "lucide-react";
import { useStore } from "../store";
import { Button, Card, Field, PageHeader, Pill, Switch, cx, fmtBytes, fmtVersion } from "../components/ui";
import { CloudflareCard } from "./Domains";
import { redact } from "../lib/publish";
import type { AppSettings, ComponentName, ComponentStatus } from "../lib/types";

const SECTIONS = [["general", "General"], ["startup", "Startup"], ["engine", "Hosting engine"], ["components", "Components"], ["cloudflare", "Cloudflare"], ["storage", "Storage"], ["security", "Security"], ["diagnostics", "Diagnostics"], ["about", "About"]] as const;

function Section({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return <section id={id} aria-labelledby={`${id}-t`} className="scroll-mt-4"><h2 id={`${id}-t`} className="mb-2 text-[16px] font-semibold">{title}</h2>{children}</section>;
}
function Row({ k, children }: { k: string; children: ReactNode }) {
  return <><dt className="text-ink2">{k}</dt><dd className="selectable min-w-0 break-words">{children}</dd></>;
}

function ComponentRow({ name, label, status }: { name: ComponentName; label: string; status?: ComponentStatus }) {
  const install = useStore((s) => s.installComponents); const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const ready = !!status?.ready;
  return (
    <div className="flex items-center justify-between gap-4 py-3">
      <div className="min-w-0"><div className="flex items-center gap-2 font-medium">{label}<Pill tone={ready ? "ok" : "bad"}>{ready ? "Ready" : status?.installed ? "Can't run" : "Not installed"}</Pill></div>
        <div className="selectable break-all text-[13px] text-ink2">{ready ? `Version ${fmtVersion(status?.version)}` : status?.error ?? "Downloaded from the official release and checked against a pinned checksum."}</div></div>
      {!ready && <Button size="sm" busy={busy} onClick={async () => { setBusy(true); try { await install([name]); toast("ok", `${label} installed`); } catch (e) { toast("error", (e as Error).message); } finally { setBusy(false); } }}>Install {label}</Button>}
    </div>
  );
}

export function Settings() {
  const settings = useStore((s) => s.settings); const engine = useStore((s) => s.engine); const saveSettings = useStore((s) => s.saveSettings);
  const stats = useStore((s) => s.stats); const toast = useStore((s) => s.toast); const tunnels = useStore((s) => s.tunnels);
  const sites = useStore((s) => s.sites); const activity = useStore((s) => s.activity);
  const [port, setPort] = useState<string>();
  if (!settings) return <p className="text-ink2">Settings couldn't load because the hosting engine isn't reachable.</p>;
  const set = (p: Partial<AppSettings>) => saveSettings({ ...settings, ...p });
  const portText = port ?? String(settings.basePortStart); const portNum = Number(portText);
  const portOk = Number.isInteger(portNum) && portNum >= 1024 && portNum <= 65000;

  async function copyDiagnostics() {
    const text = redact(JSON.stringify({
      engine, tunnels, stats,
      sites: sites.map((s) => ({ id: s.id, status: s.status, detail: s.statusDetail, port: s.localPort, publishState: s.publishState, hostname: s.publishHostname })),
      recentActivity: activity.slice(0, 40).map((a) => `${a.ts} ${a.level} ${a.message}`),
    }, null, 2));
    try { await navigator.clipboard.writeText(text); toast("ok", "Diagnostics copied with secrets removed"); }
    catch { toast("error", "Windows blocked clipboard access."); }
  }

  return (
    <div className="mx-auto grid max-w-[920px] grid-cols-[160px_minmax(0,1fr)] gap-6 max-[1000px]:grid-cols-1">
      <nav aria-label="Settings sections" className="sticky top-0 grid content-start gap-0.5 self-start max-[1000px]:hidden">
        <h1 className="mb-3 text-[26px] font-bold">Settings</h1>
        {SECTIONS.map(([id, label]) => <a key={id} href={`#${id}`} className="press rounded-ctl px-2.5 py-1.5 text-ink2 hover:bg-panel2 hover:text-ink">{label}</a>)}
      </nav>
      <div className="grid gap-6">
        <div className="hidden max-[1000px]:block"><PageHeader title="Settings" /></div>
        <Section id="general" title="General">
          <Card className="p-4">
            <div id="theme-label" className="mb-2 font-medium">Appearance</div>
            <div className="inline-flex rounded-ctl border border-line p-0.5" role="radiogroup" aria-labelledby="theme-label">
              {(["system", "light", "dark"] as const).map((t) => (
                <button key={t} type="button" role="radio" aria-checked={settings.theme === t} onClick={() => set({ theme: t })}
                  className={cx("press h-7 rounded-[5px] px-3.5 capitalize", settings.theme === t ? "bg-kelp font-medium text-kelpink" : "text-ink2 hover:text-ink")}>{t}</button>))}
            </div>
          </Card>
        </Section>
        <Section id="startup" title="Startup">
          <Card className="divide-y divide-line px-4">
            <Switch label="Start hosting when I sign in to Windows" desc="Your websites come back online after a restart, once you sign in." on={settings.launchOnLogin} onChange={(v) => set({ launchOnLogin: v })} />
            <Switch label="Keep hosting when the window is closed" desc="Closing Hostify hides it to the tray. Your websites stay online." on={settings.closeToTray} onChange={(v) => set({ closeToTray: v })} />
          </Card>
        </Section>
        <Section id="engine" title="Hosting engine">
          <Card className="p-4">
            <dl className="grid grid-cols-[150px_minmax(0,1fr)] gap-y-1.5">
              <Row k="Status">{engine?.running ? `Running in ${engine.mode === "service" ? "service" : "user"} mode` : "Not running"}</Row>
              <Row k="Version">{engine?.version ?? "Unknown"}</Row>
            </dl>
            <div className="mt-4 max-w-[260px]">
              <Field label="First port for new websites" inputMode="numeric" value={portText} onChange={(e) => setPort(e.target.value)}
                error={!portOk ? "Enter a whole number from 1024 to 65000." : undefined} hint="Used only on this PC. Existing websites keep their port."
                onBlur={() => { if (portOk && portNum !== settings.basePortStart) void set({ basePortStart: portNum }); }} />
            </div>
          </Card>
        </Section>
        <Section id="components" title="Components">
          <Card className="divide-y divide-line px-4">
            <ComponentRow name="caddy" label="Caddy" status={engine?.caddy} />
            <ComponentRow name="cloudflared" label="cloudflared" status={engine?.cloudflared} />
          </Card>
        </Section>
        <Section id="cloudflare" title="Cloudflare"><CloudflareCard /></Section>
        <Section id="storage" title="Storage">
          <Card className="p-4"><dl className="grid grid-cols-[150px_minmax(0,1fr)] gap-y-1.5">
            <Row k="Used by Hostify">{stats ? fmtBytes(stats.hostifyDataBytes) : "Open the Dashboard to measure"}</Row>
            <Row k="Data folder">{engine?.dataDir ?? "Unknown"}</Row>
            <Row k="Components folder">{engine?.binDir ?? "Unknown"}</Row>
          </dl><p className="mt-3 text-[13px] text-ink2">Uninstalling Hostify keeps this data so you can reinstall without losing your websites.</p></Card>
        </Section>
        <Section id="security" title="Security">
          <Card className="p-4"><ul className="grid list-disc gap-1.5 pl-4 text-ink2">
            <li>Your Cloudflare token and tunnel credentials are kept in Windows Credential Manager, never in Hostify's database or logs.</li>
            <li>Websites listen only on this PC's loopback address. The web server's admin API is switched off.</li>
            <li>The tunnel makes outbound connections only, so no router ports are opened.</li>
            <li>Only your Windows account can talk to the hosting engine.</li>
          </ul></Card>
        </Section>
        <Section id="diagnostics" title="Diagnostics">
          <Card className="flex flex-wrap items-center justify-between gap-3 p-4">
            <p className="max-w-[440px] text-ink2">Copies engine status, website states and recent activity, with secrets removed, so you can share it when asking for help.</p>
            <Button onClick={copyDiagnostics}><Clipboard size={14} aria-hidden />Copy diagnostics</Button>
          </Card>
        </Section>
        <Section id="about" title="About">
          <Card className="p-4"><p className="font-medium">Hostify {engine?.version}</p>
            <p className="mt-1 text-ink2">Hosts static websites from your Windows PC. Uses Caddy and cloudflared, both under the Apache License 2.0. Their licence texts are included in the install folder.</p></Card>
        </Section>
      </div>
    </div>
  );
}
