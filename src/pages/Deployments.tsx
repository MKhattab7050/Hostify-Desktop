import { useEffect, useState } from "react";
import { Rocket } from "lucide-react";
import { useStore } from "../store";
import { api, listen } from "../lib/api";
import { publishPill } from "../lib/publish";
import { Button, Card, Disclosure, Empty, PageHeader, Pill, StatusPill } from "../components/ui";
import { PublishProgress } from "../components/PublishProgress";
import { BuildPanel } from "../components/BuildPanel";
import type { AppConfig, BuildStatus, Site } from "../lib/types";

function SiteRow({ site }: { site: Site }) {
  const go = useStore((s) => s.go); const openWizard = useStore((s) => s.openWizard); const setDeployment = useStore((s) => s.setDeployment);
  const deployment = useStore((s) => (site.publishDeploymentId ? s.deployments[site.publishDeploymentId] : undefined));
  const [build, setBuild] = useState<BuildStatus | null>(); const [config, setConfig] = useState<AppConfig>(); const [log, setLog] = useState<string[]>();
  const isApp = !!site.kind && site.kind !== "static"; const pill = publishPill(site.publishState);

  useEffect(() => { if (site.publishDeploymentId && !deployment) api.getDeployment(site.publishDeploymentId).then(setDeployment).catch(() => {}); }, [site.publishDeploymentId, deployment, setDeployment]);
  useEffect(() => {
    if (!isApp) return;
    api.getBuildStatus(site.id).then(setBuild).catch(() => {}); api.getAppConfig(site.id).then(setConfig).catch(() => {});
    let off: (() => void) | undefined; let dead = false;
    listen<BuildStatus>("app-build", (b) => { if (b.siteId === site.id) setBuild(b); }).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  }, [isApp, site.id]);

  return (
    <Card className="grid gap-3 p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0"><h2 className="truncate text-[17px] font-semibold">{site.name}</h2>
          <div className="mt-1 flex flex-wrap items-center gap-1.5"><StatusPill status={site.status} /><Pill tone={pill.tone} busy={pill.busy}>{pill.text}</Pill></div></div>
        <div className="flex gap-1.5">
          <Button size="sm" onClick={() => go("websites", site.id)}>Open website</Button>
          {!site.publishState && <Button size="sm" onClick={() => openWizard(site.id)}>Connect a domain</Button>}
        </div>
      </div>
      {isApp && build !== undefined && config && <BuildPanel site={site} compact />}
      {isApp && <Disclosure title="Build log"><pre onFocus={() => {}} className="selectable mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-ctl bg-chalk p-3 font-mono text-[12px] leading-5 text-ink2">{log?.length ? log.join("\n") : "Nothing logged yet."}</pre>
        <div className="mt-2"><Button size="sm" variant="ghost" onClick={() => api.appLogs(site.id, "build", 400).then(setLog).catch(() => {})}>Load build log</Button></div></Disclosure>}
      {deployment
        ? <div><h3 className="mb-2 text-[14px] font-semibold">Public HTTPS checks{deployment.hostname ? ` for ${deployment.hostname}` : ""}</h3>
            <PublishProgress deployment={deployment} onRepublish={() => openWizard(site.id)} /></div>
        : !site.publishState && <p className="text-[13px] text-ink2">Not published. Public checks start when you connect a domain.</p>}
    </Card>
  );
}

export function Deployments() {
  const sites = useStore((s) => s.sites); const openWizard = useStore((s) => s.openWizard);
  return (
    <div className="mx-auto grid max-w-[1040px] gap-5">
      <PageHeader title="Deployments" sub="Builds, health checks and public HTTPS verification for every website." />
      {sites.length === 0
        ? <Card><Empty icon={<Rocket size={20} />} title="Nothing deployed yet" body="Add a website and its builds and checks appear here." action={<Button variant="primary" onClick={() => openWizard()}>Add website</Button>} /></Card>
        : sites.map((s) => <SiteRow key={s.id} site={s} />)}
    </div>
  );
}
