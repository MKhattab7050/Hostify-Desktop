import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, ChevronRight, CircleHelp, ClipboardList, Cloud, FolderOpen, Globe2, Loader2, ShieldCheck, SlidersHorizontal, Upload, Zap } from "lucide-react";
import { api } from "../lib/api";
import { useStore } from "../store";
import type { Deployment, ProjectReport, RuntimeStatus, Site, SiteKind, SourceKind, SourceReport } from "../lib/types";
import { Button, Card, Disclosure, Field, Pill } from "./ui";
import { PublishProgress } from "./PublishProgress";
import { ConnectCloudflare } from "../pages/Domains";

const HOST = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const clean = (value: string) => value.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");

type Phase = "project" | "review" | "working" | "tracking";
type Entry = { step: string; detail: string; status: "running" | "done" | "failed" };
type Project = { path: string; sourceKind: SourceKind; source: SourceReport; report: ProjectReport };

function recommendedRuntime(project: Project | undefined, site: Site | undefined, list: RuntimeStatus[]): RuntimeStatus | undefined {
  const kind = (site?.kind ?? project?.report.siteKind) as SiteKind | undefined;
  if (!kind || kind === "static") return undefined;
  const candidates = list.filter((r) => r.kind === kind);
  const exact = site?.runtimeVersion ?? project?.report.recommendedRuntime;
  const pinned = candidates.find((r) => r.version === exact);
  if (pinned) return pinned;
  // Never override a declared engines constraint with a speculative runtime.
  if (project?.report.engines || site?.runtimeVersion) return undefined;
  return candidates.find((r) => kind === "node" ? r.version.startsWith("24.") : r.version.startsWith("8.4."))
    ?? candidates[0];
}

async function readOnlyRetry<T>(task: () => Promise<T>, report: (text: string) => void): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try { return await task(); }
    catch (error) {
      const message = (error as Error).message.toLowerCase();
      // Provider write operations must never enter this helper. Two retries at most.
      const transient = /timed out|timeout|connection reset|temporarily unavailable/.test(message);
      if (!transient || attempt >= 2) throw error;
      report(`Read-only check temporarily unavailable. Rechecking (attempt ${attempt + 2} of 3).`);
      await new Promise((resolve) => setTimeout(resolve, 500 * 2 ** attempt));
    }
  }
}

async function waitForBuild(siteId: string, report: (text: string) => void) {
  const deadline = Date.now() + 20 * 60_000;
  while (Date.now() < deadline) {
    const status = await api.getBuildStatus(siteId);
    if (status?.state === "done") {
      const config = await api.getAppConfig(siteId);
      const release = config.builtRelease;
      if (!release || release !== config.trustedRelease ||
          (release !== config.pendingRelease && release !== config.activeRelease)) {
        throw new Error("Build ended without a trusted, verified release. Publishing stopped.");
      }
      return;
    }
    if (status?.state === "failed" || status?.state === "cancelled") {
      throw new Error(status.detail || "Application build did not complete.");
    }
    report("Waiting for the application build to finish.");
    await new Promise((resolve) => setTimeout(resolve, 1200));
  }
  throw new Error("Build timed out. The application was not published.");
}

function riskNotice(kind: string) {
  return kind === "static"
    ? "Static content is served locally. Hostify still verifies the local route, tunnel, DNS and public HTTPS."
    : "Node.js and PHP execute with your Windows account permissions. Only run projects you trust. Fast Publish does not sandbox application code.";
}

export function FastPublish({ initialSite, onDone, onStandard }: { initialSite?: Site; onDone: () => void; onStandard: () => void }) {
  const setDeployment = useStore((s) => s.setDeployment);
  const refresh = useStore((s) => s.refresh);
  const toast = useStore((s) => s.toast);
  const [phase, setPhase] = useState<Phase>(initialSite ? "review" : "project");
  const [project, setProject] = useState<Project>();
  const [site, setSite] = useState<Site | undefined>(initialSite);
  const [hostname, setHostname] = useState(initialSite?.publishHostname ?? "");
  const [runtimes, setRuntimes] = useState<RuntimeStatus[]>([]);
  const [connected, setConnected] = useState(false);
  const [connect, setConnect] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [history, setHistory] = useState<Entry[]>([]);
  const [deployment, setLocalDeployment] = useState<Deployment>();
  const [trust, setTrust] = useState(false);
  const [allowComponents, setAllowComponents] = useState(false);
  const [allowDatabase, setAllowDatabase] = useState(false);
  const [externalDatabase, setExternalDatabase] = useState(false);
  const [consentDns, setConsentDns] = useState(false);
  const kind = site?.kind ?? project?.report.siteKind ?? "static";
  const runtime = recommendedRuntime(project, site, runtimes);
  const requiresDatabase = !initialSite && !!project?.report.usesDatabase;
  const active = deployment && useStore.getState().deployments[deployment.id] || deployment;

  useEffect(() => {
    let mounted = true;
    void Promise.all([api.listRuntimes(), api.cfStatus()])
      .then(([versions, cf]) => { if (mounted) { setRuntimes(versions); setConnected(cf.connected); } })
      .catch((e) => { if (mounted) setError((e as Error).message); });
    return () => { mounted = false; };
  }, []);

  // A previously started deployment keeps running independently of this window.
  useEffect(() => {
    const id = initialSite?.publishDeploymentId;
    if (!id) return;
    let alive = true;
    void api.getDeployment(id).then((d) => {
      if (alive && d && d.state !== "rolled_back") { setLocalDeployment(d); setDeployment(d); setPhase("tracking"); }
    }).catch(() => {});
    return () => { alive = false; };
  }, [initialSite?.publishDeploymentId, setDeployment]);

  const note = (step: string, detail: string, status: Entry["status"] = "done") =>
    setHistory((h) => [...h.filter((x) => x.step !== step), { step, detail, status }]);

  async function choose(sourceKind: SourceKind) {
    setBusy(true); setError(undefined);
    try {
      const path = await api.pickPath(sourceKind);
      if (!path) return;
      const [source, report] = await Promise.all([
        api.validateSource(path, sourceKind),
        api.inspectProject(path, sourceKind),
      ]);
      if (!source.ok) throw new Error(source.errors.join(" ") || "This project cannot be imported safely.");
      if (report.siteKind === "unknown") {
        throw new Error("Project type is unclear. Use Standard Publish to choose and validate its runtime.");
      }
      setProject({ path, sourceKind, source, report });
      note("project", `${report.siteKind.toUpperCase()} project detected: ${report.framework ?? "standard project"}`);
    } catch (e) { setError((e as Error).message); }
    finally { setBusy(false); }
  }

  const canContinue = !!initialSite || !!project;
  const domain = clean(hostname);
  const domainValid = HOST.test(domain);
  const needsRuntime = kind !== "static" && !runtime?.ready;
  const canPublish = connected && domainValid && consentDns &&
    (kind === "static" || (trust && !!runtime)) &&
    (!needsRuntime || allowComponents) &&
    (!requiresDatabase || (allowDatabase !== externalDatabase)) &&
    !busy;

  async function publish() {
    if (!canPublish) return;
    setBusy(true); setPhase("working"); setError(undefined);
    let current = site;
    let currentStep = "preflight";
    try {
      const cf = await api.cfStatus();
      if (!cf.connected) throw new Error("Connect Cloudflare before publishing.");
      note("domain", "Checking domain and Cloudflare permissions.", "running");
      const check = await readOnlyRetry(() => api.cfCheckDomain(domain), (message) => note("domain", message, "running"));
      if (!check.canManageDns || check.problems.length) {
        throw new Error(check.problems.join(" ") || "Cloudflare DNS permission is unavailable.");
      }
      note("domain", `Domain ${domain} belongs to zone ${check.zoneName}.`);

      if (!current) {
        if (!project) throw new Error("Choose a project first.");
        currentStep = "import";
        note("import", "Importing a separate release copy.", "running");
        current = await api.addSite(
          project.report.name?.trim() || project.path.split(/[\\/]/).filter(Boolean).pop()?.replace(/\.zip$/i, "") || "My website",
          project.path, project.sourceKind, project.report.siteKind, project.report.suggested
        );
        setSite(current);
        await refresh();
        note("import", "Project imported. Your original folder has not been changed.");
      }

      // Dynamic code never runs without this user's explicit acknowledgment.
      if (kind !== "static") {
        if (!trust) throw new Error("Explicit trust is required to execute this project's code.");
        if (!runtime) throw new Error("No compatible, supported runtime was detected. Use Standard Publish.");
        currentStep = "runtime";
        const version = current.runtimeVersion;
        if (version !== runtime.version) {
          if (initialSite) throw new Error("Change the existing website runtime in Standard Publish and rebuild it first.");
          await api.setAppConfig(current.id, project!.report.suggested,
            kind === "php" ? { phpVersion: runtime.version } : { nodeVersion: runtime.version });
          note("runtime", `Selected ${kind} runtime ${runtime.version} with explicit approval.`);
        }
        if (!runtime.ready) {
          if (!allowComponents) throw new Error("Runtime installation requires authorization.");
          note("runtime", `Installing verified ${kind} runtime ${runtime.version}.`, "running");
          await api.installRuntime(kind, runtime.version);
          note("runtime", `Verified runtime ${runtime.version} installed.`);
        }
      }

      currentStep = "components";
      const engine = await api.engineStatus();
      if (!engine.componentsReady) {
        if (!allowComponents) throw new Error("Approve installation of Caddy and cloudflared before publishing.");
        note("components", "Installing verified Caddy and cloudflared components.", "running");
        await useStore.getState().installComponents();
        note("components", "Required hosting components installed.");
      }

      if (requiresDatabase) {
        if (!allowDatabase && !externalDatabase) throw new Error("Choose a database arrangement.");
        if (allowDatabase) {
          currentStep = "database";
          note("database", "Preparing a new database assigned only to this website.", "running");
          const mysql = await api.mysqlStatus();
          if (!mysql.setUp) {
            const installed = (await api.listRuntimes()).find((r) => r.kind === "mysql" && r.version === "8.4.10");
            if (!installed) throw new Error("Supported MySQL runtime unavailable. Use Standard Publish.");
            if (!installed.ready) {
              if (!allowComponents) throw new Error("Approve MySQL installation before continuing.");
              await api.installRuntime("mysql", installed.version);
            }
            await api.mysqlSetup(installed.version);
          } else if (!mysql.running) {
            // Restarting an already-owned service is safe; this does not import a schema.
            await api.mysqlStart();
          }
          const unique = `site_${current.id.replace(/-/g, "").slice(0, 12).toLowerCase()}`;
          const existing = (await api.listDatabases()).find((d) => d.name === unique);
          if (existing && existing.siteId !== current.id) {
            throw new Error("This database name belongs to another website. Nothing was reassigned.");
          }
          if (!existing) await api.createDatabase(unique, current.id, true, false);
          note("database", "Dedicated database is ready. No SQL schema was automatically executed.");
        }
      }

      if (kind !== "static") {
        currentStep = "build";
        const c = await api.getAppConfig(current.id);
        const candidate = c.pendingRelease ?? c.activeRelease;
        const ready = !!candidate && candidate === c.builtRelease && candidate === c.trustedRelease;
        if (!ready) {
          note("build", "Installing dependencies and building a trusted release.", "running");
          await api.buildApp(current.id, trust);
          await waitForBuild(current.id, (detail) => note("build", detail, "running"));
          note("build", "Build completed and trust verified.");
        } else {
          note("build", "Reusing an already trusted and successfully built release.");
        }
      }

      currentStep = "start";
      note("start", "Starting and verifying the local application.", "running");
      await api.startSite(current.id);
      const preflight = await readOnlyRetry(() => api.publishPreflight(current!.id),
        (message) => note("start", message, "running"));
      if (!preflight.ready) throw new Error(preflight.blocked.join(" ") || "Local publish preflight failed.");
      note("start", "Local process, runtime, trust and Caddy checks passed.");

      currentStep = "dns-plan";
      note("dns-plan", "Checking current DNS state without changing anything.", "running");
      const plan = await readOnlyRetry(() => api.cfPlanDns(current!.id, domain),
        (message) => note("dns-plan", message, "running"));
      if (plan.action === "conflict") throw new Error(plan.message || "Existing DNS record cannot be changed.");
      note("dns-plan", plan.message);

      currentStep = "publish";
      note("publish", "Starting the shared, journaled Cloudflare deployment.", "running");
      const d = await api.publishSite(current.id, domain, consentDns);
      setLocalDeployment(d);
      setDeployment(d);
      note("publish", "The engine accepted the deployment. Public HTTPS must still be verified.");
      await refresh();
      setPhase("tracking");
    } catch (e) {
      const message = (e as Error).message;
      note(currentStep, message, "failed");
      setError(message);
      // Failure never triggers another publish request or a DNS/database write.
      if (current) {
        try {
          const diagnosis = await api.publishDiagnosis(current.id);
          note("diagnosis", `${diagnosis.class}: ${diagnosis.nextAction}`);
        } catch { /* keep the original error */ }
      }
      setPhase("review");
    } finally { setBusy(false); }
  }

  if (phase === "tracking" && active) {
    return (
      <div className="grid gap-4">
        <div className="flex items-center justify-between">
          <Pill tone="ok">Shared secure publishing engine</Pill>
          <Button size="sm" onClick={onStandard}><SlidersHorizontal size={14} />Advanced settings</Button>
        </div>
        <PublishProgress deployment={active} onDone={onDone} onRepublish={() => {
          setDeployment({ ...active, state: "rolled_back" }); setLocalDeployment(undefined); setPhase("review");
        }} />
        <Disclosure title="Fast Publish preparation history">
          <ol className="mt-2 grid gap-2 text-[13px]">
            {history.map((entry, index) => <li key={index} className="flex gap-2">
              {entry.status === "failed" ? <AlertTriangle size={14} className="mt-0.5 shrink-0 text-buoy" /> :
                <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-kelp" />}
              <span><strong>{entry.step}:</strong> {entry.detail}</span>
            </li>)}
          </ol>
        </Disclosure>
      </div>
    );
  }

  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2 text-[13px]">
          <Pill tone="ok">Fast Publish</Pill>
          <span className="text-ink3">Project <ChevronRight size={12} className="inline" /> Domain <ChevronRight size={12} className="inline" /> Publish</span>
        </div>
        <Button size="sm" variant="ghost" disabled={busy} onClick={onStandard}>
          <SlidersHorizontal size={14} />Standard Publish
        </Button>
      </div>
      {phase === "project" && (
        <>
          <Card className="grid gap-4 p-5">
            <div><div className="mb-1 flex items-center gap-2 text-lg font-bold"><Zap className="text-kelp" size={19} />Select your project</div>
              <p className="text-[13px] text-ink2">Hostify detects the framework, runtime and dependencies. Your original project is never modified.</p></div>
            <div className="grid gap-2 sm:grid-cols-2">
              <Button busy={busy} onClick={() => void choose("directory")}><FolderOpen size={16} />Choose folder</Button>
              <Button busy={busy} onClick={() => void choose("zip")}><Upload size={16} />Choose ZIP</Button>
            </div>
            {project && <div className="rounded-ctl border border-kelp bg-kelpsoft p-3">
              <div className="font-semibold">{project.report.name || project.path.split(/[\\/]/).pop()}</div>
              <p className="break-all text-[12px] text-ink2">{project.path}</p>
              <div className="mt-2 flex flex-wrap gap-2"><Pill tone="ok">{project.report.siteKind}</Pill>
                {project.report.framework && <Pill tone="idle">{project.report.framework}</Pill>}
                {project.report.usesDatabase && <Pill tone="wait">Database detected</Pill>}</div>
            </div>}
            {project?.report.warnings.map((warning, i) => <p key={i} className="flex gap-2 text-[13px] text-lantern"><AlertTriangle size={15} />{warning}</p>)}
            {error && <p role="alert" className="text-buoy">{error}</p>}
          </Card>
          <div className="flex justify-end">
            <Button variant="primary" disabled={!canContinue || busy} onClick={() => { setError(undefined); setPhase("review"); }}>Continue to domain <ChevronRight size={14} /></Button>
          </div>
        </>
      )}
      {(phase === "review" || phase === "working") && (
        <>
          <Card className="grid gap-4 p-5">
            <div className="flex items-center gap-3"><Globe2 className="text-kelp" size={22} /><div>
              <h2 className="text-lg font-semibold">Domain and safety checks</h2>
              <p className="text-[13px] text-ink2">{site?.name || project?.report.name || "Your website"} · {kind.toUpperCase()}</p>
            </div></div>
            <Field label="Public domain" placeholder="app.example.com" value={hostname} disabled={busy}
              onChange={(e) => { setHostname(e.target.value); setConsentDns(false); }}
              error={hostname && !domainValid ? "Enter a valid full hostname." : undefined}
              hint="Cloudflare ownership and DNS conflicts are checked before publishing." />
            {!connected && <div className="rounded-ctl border border-lantern bg-lanternsoft p-3 text-[13px]">
              <p className="font-medium">Cloudflare is not connected.</p>
              <Button size="sm" onClick={() => setConnect(true)}>Connect Cloudflare</Button>
              {connect && <ConnectCloudflare onClose={() => { setConnect(false); void api.cfStatus().then((cf) => setConnected(cf.connected)); }} />}
            </div>}
            {kind !== "static" && <div className="rounded-ctl border border-line p-3 text-[13px]">
              <p className="font-semibold">Application runtime</p>
              <p className="mt-1 text-ink2">{runtime ? `${kind === "node" ? "Node.js" : "PHP"} ${runtime.version}${runtime.ready ? " · Installed" : " · Installation required"}` : "No compatible pinned runtime. Use Standard Publish."}</p>
              {project?.report.engines && <p className="mt-1 text-ink3">Declared engine requirement: {project.report.engines}</p>}
            </div>}
            <p className="rounded-ctl bg-chalk p-3 text-[13px] text-ink2"><ShieldCheck size={15} className="mr-2 inline" />{riskNotice(kind)}</p>
            {kind !== "static" && <label className="flex gap-3 text-[13px]">
              <input type="checkbox" checked={trust} disabled={busy} onChange={(e) => setTrust(e.target.checked)} className="mt-1" />
              <span><strong>I trust this application</strong> and explicitly authorize Hostify to execute its dependencies, build and start commands.</span>
            </label>}
            <label className="flex gap-3 text-[13px]">
              <input type="checkbox" checked={allowComponents} disabled={busy} onChange={(e) => setAllowComponents(e.target.checked)} className="mt-1" />
              <span>Allow installation of official, integrity-checked Hostify components and selected runtime when missing.</span>
            </label>
            {requiresDatabase && <fieldset className="grid gap-2 rounded-ctl border border-line p-3">
              <legend className="px-1 font-medium">Database detected</legend>
              <label className="flex gap-2 text-[13px]"><input type="checkbox" checked={allowDatabase} disabled={busy}
                onChange={(e) => { setAllowDatabase(e.target.checked); if (e.target.checked) setExternalDatabase(false); }} />
                Create a dedicated MySQL database for this site (no automatic schema import)</label>
              <label className="flex gap-2 text-[13px]"><input type="checkbox" checked={externalDatabase} disabled={busy}
                onChange={(e) => { setExternalDatabase(e.target.checked); if (e.target.checked) setAllowDatabase(false); }} />
                I will configure an external or existing database myself</label>
            </fieldset>}
            <label className="flex gap-3 rounded-ctl border border-kelp bg-kelpsoft p-3 text-[13px]">
              <input type="checkbox" checked={consentDns} disabled={busy} onChange={(e) => setConsentDns(e.target.checked)} className="mt-1" />
              <span><strong>Authorize this domain's Cloudflare DNS/tunnel changes.</strong> Hostify will plan them first and will not overwrite a conflicting unmanaged record.</span>
            </label>
          </Card>
          {error && <div role="alert" className="rounded-ctl border border-buoy bg-buoysoft p-4">
            <p className="flex items-center gap-2 font-semibold text-buoy"><AlertTriangle size={16} />Publishing paused safely</p>
            <p className="mt-1 break-words text-[13px]">{error}</p>
            <p className="mt-1 text-[12px] text-ink3">No error was bypassed. Existing sites and Cloudflare resources remain preserved.</p>
          </div>}
          {!!history.length && <Disclosure title="Diagnosis and preparation history">
            <ol className="mt-2 grid gap-2 text-[13px]">
              {history.map((entry, i) => <li key={i} className="flex gap-2">
                {entry.status === "running" ? <Loader2 size={14} className="spin mt-0.5 shrink-0" /> :
                  entry.status === "failed" ? <AlertTriangle size={14} className="mt-0.5 shrink-0 text-buoy" /> : <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-kelp" />}
                <span><strong>{entry.step}:</strong> {entry.detail}</span></li>)}
            </ol>
          </Disclosure>}
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Button disabled={busy} onClick={() => { setError(undefined); setPhase(initialSite ? "review" : "project"); }}>Back</Button>
            <Button variant="primary" busy={busy} disabled={!canPublish} onClick={() => void publish()}>
              {busy ? <Loader2 size={15} className="spin" /> : <Zap size={15} />} {busy ? "Preparing safely..." : "Publish website"}
            </Button>
          </div>
          <p className="flex gap-2 text-[12px] text-ink3"><CircleHelp size={14} className="shrink-0" />DNS propagation may continue after the local setup. The deployment remains journaled and resumes checking when Hostify restarts.</p>
        </>
      )}
      {phase === "working" && !site && <p className="text-[13px] text-ink3"><ClipboardList size={14} className="mr-1 inline" />Validating and creating your local release.</p>}
      {phase === "tracking" && !active && <div className="flex items-center gap-2 text-[13px] text-ink2"><Cloud size={15} />Loading deployment status...</div>}
    </div>
  );
}
