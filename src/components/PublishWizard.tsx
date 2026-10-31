import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  Boxes,
  Check,
  CheckCircle2,
  Code2,
  ExternalLink,
  FileArchive,
  FolderOpen,
  Globe2,
  Layers,
  Loader2,
  Plus,
  Server,
  Trash2,
} from "lucide-react";
import { useStore } from "../store";
import { api, isTauri, onFileDrop } from "../lib/api";
import { isActive, holdsResources } from "../lib/publish";
import { Button, Card, Field, Modal, Pill, Select, Skeleton, cx, fmtBytes, fmtFiles } from "./ui";
import { PublishProgress } from "./PublishProgress";
import { ConnectCloudflare } from "../pages/Domains";
import { RuntimeInstall, RUNTIME_NAMES } from "../pages/Runtimes";
import { AppConfigForm } from "./AppConfigForm";
import { BuildPanel } from "./BuildPanel";
import type {
  AppConfig,
  DnsPlan,
  DomainCheck,
  EnvVar,
  ManagedDatabase,
  MysqlStatus,
  ProjectReport,
  RuntimeStatus,
  Site,
  SiteKind,
  SourceKind,
  SourceReport,
} from "../lib/types";

const HOST_RE = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const cleanHost = (v: string) => v.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");
const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

interface NewSiteState {
  path: string;
  sourceKind: SourceKind;
  sourceReport?: SourceReport;
  projectReport?: ProjectReport;
  name: string;
  siteKind: SiteKind;
  runtimeVersion: string;
  config: AppConfig;
  dbChoice: "none" | "new" | "existing";
  newDbName: string;
  selectedDb: string;
  envVars: EnvVar[];
}

export function PublishWizard() {
  const closeWizard = useStore((s) => s.closeWizard);
  const refresh = useStore((s) => s.refresh);
  const toast = useStore((s) => s.toast);
  const wizardSiteId = useStore((s) => s.wizardSiteId);
  const existing = useStore((s) => s.sites.find((x) => x.id === wizardSiteId));

  const [isExistingPublish] = useState(!!existing);
  const [createdSite, setCreatedSite] = useState<Site | undefined>(existing);
  const [hostname, setHostname] = useState(existing?.publishHostname ?? "");

  const [step, setStep] = useState(existing ? 8 : 1);

  const [state, setState] = useState<NewSiteState>({
    path: "",
    sourceKind: "directory",
    name: "",
    siteKind: "static",
    runtimeVersion: "",
    config: {
      install: "ci",
      buildScript: null,
      start: { type: "node", args: ["server.js"] },
      workingDir: "",
      healthPath: "/",
      restart: "always",
      memoryLimitMb: null,
      allowInstallScripts: false,
      documentRoot: "",
      phpWorkers: 4,
      phpExtensions: ["pdo_mysql", "mysqli", "mbstring", "openssl", "curl"],
      phpSettings: { memory_limit: "256M", max_execution_time: "60" },
      persistentPaths: [],
      internalPort: null,
      activeRelease: null,
      previousRelease: null,
      pendingRelease: null,
      builtRelease: null,
      trustedRelease: null,
    },
    dbChoice: "none",
    newDbName: "",
    selectedDb: "",
    envVars: [],
  });

  const stepLabels = isExistingPublish
    ? ["Domain", "Publish"]
    : state.siteKind === "static"
      ? ["Import", "Project type", "Local preview", "Domain", "Publish"]
      : [
          "Import",
          "Project type",
          "Runtime",
          "Config",
          "Database",
          "Environment",
          "Local preview",
          "Domain",
          "Publish",
        ];

  const currentStepIndex = isExistingPublish
    ? step === 8
      ? 0
      : 1
    : state.siteKind === "static"
      ? step === 1
        ? 0
        : step === 2
          ? 1
          : step === 7
            ? 2
            : step === 8
              ? 3
              : 4
      : step - 1;

  function goNextFromStep2() {
    if (state.siteKind === "static") {
      setStep(7);
    } else {
      setStep(3);
    }
  }

  function goBackFromStep7() {
    if (state.siteKind === "static") {
      setStep(2);
    } else {
      setStep(6);
    }
  }

  return (
    <Modal
      title={
        isExistingPublish
          ? `Publish ${existing?.name}`
          : createdSite
            ? `Publish ${createdSite.name}`
            : "Add a website"
      }
      wide
      onClose={closeWizard}
    >
      <ol className="mb-5 flex items-center gap-2 overflow-x-auto pb-1" aria-label="Progress">
        {stepLabels.map((s, i) => (
          <li
            key={s}
            className={cx("flex items-center gap-2", i < stepLabels.length - 1 && "flex-1")}
            aria-current={i === currentStepIndex ? "step" : undefined}
          >
            <span
              className={cx(
                "grid h-6 w-6 shrink-0 place-items-center rounded-full text-[12px] font-semibold transition-colors",
                i < currentStepIndex
                  ? "bg-kelp text-kelpink"
                  : i === currentStepIndex
                    ? "border-2 border-kelp text-kelp"
                    : "border border-line text-ink3"
              )}
            >
              {i < currentStepIndex ? <Check size={13} strokeWidth={3} aria-label="Done" /> : i + 1}
            </span>
            <span className={cx("whitespace-nowrap text-[13px]", i === currentStepIndex ? "font-semibold" : "text-ink3")}>
              {s}
            </span>
            {i < stepLabels.length - 1 && (
              <span
                className={cx(
                  "h-px min-w-3 flex-1 transition-colors",
                  i < currentStepIndex ? "bg-kelp" : "bg-line"
                )}
              />
            )}
          </li>
        ))}
      </ol>

      {step === 1 && (
        <Step1Import
          state={state}
          onChange={setState}
          onNext={() => setStep(2)}
        />
      )}

      {step === 2 && (
        <Step2Detect
          state={state}
          onChange={setState}
          onBack={() => setStep(1)}
          onNext={goNextFromStep2}
        />
      )}

      {step === 3 && (
        <Step3Runtime
          state={state}
          onChange={setState}
          onBack={() => setStep(2)}
          onNext={() => setStep(4)}
        />
      )}

      {step === 4 && (
        <Step4AppConfig
          state={state}
          onChange={setState}
          onBack={() => setStep(3)}
          onNext={() => setStep(5)}
        />
      )}

      {step === 5 && (
        <Step5Database
          state={state}
          onChange={setState}
          onBack={() => setStep(4)}
          onNext={() => setStep(6)}
        />
      )}

      {step === 6 && (
        <Step6EnvVars
          state={state}
          onChange={setState}
          onBack={() => setStep(5)}
          onNext={() => setStep(7)}
        />
      )}

      {step === 7 && (
        <Step7LocalPreview
          state={state}
          site={createdSite}
          onSiteReady={async (s) => {
            setCreatedSite(s);
            await refresh();
          }}
          onBack={createdSite ? undefined : goBackFromStep7}
          onLocalOnly={() => {
            toast("ok", `${createdSite?.name ?? state.name} is running on this computer`);
            closeWizard();
          }}
          onNext={() => setStep(8)}
        />
      )}

      {step === 8 && createdSite && (
        <Step8Domain
          site={createdSite}
          hostname={hostname}
          setHostname={setHostname}
          onBack={isExistingPublish ? undefined : () => setStep(7)}
          onNext={() => setStep(9)}
        />
      )}

      {step === 9 && createdSite && (
        <Step9Publish
          site={createdSite}
          hostname={hostname}
          onDone={closeWizard}
          onBack={() => setStep(8)}
        />
      )}
    </Modal>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 1: Import Project
// -------------------------------------------------------------------------------------------------
function Step1Import({
  state,
  onChange,
  onNext,
}: {
  state: NewSiteState;
  onChange: React.Dispatch<React.SetStateAction<NewSiteState>>;
  onNext: () => void;
}) {
  const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);

  const inspect = useCallback(
    async (p: string, k: SourceKind) => {
      setBusy(true);
      const base = p.split(/[\\/]/).filter(Boolean).pop()?.replace(/\.zip$/i, "") ?? "";
      try {
        const [srcRep, projRep] = await Promise.all([
          api.validateSource(p, k),
          api.inspectProject(p, k).catch(() => undefined),
        ]);

        const detectedKind: SiteKind =
          projRep?.siteKind && projRep.siteKind !== "unknown" ? projRep.siteKind : "static";

        const initialEnv: EnvVar[] = (projRep?.dotenvNames ?? []).map((name) => ({
          name,
          secret: /SECRET|KEY|TOKEN|PASS/i.test(name),
          value: "",
        }));

        onChange((prev) => ({
          ...prev,
          path: p,
          sourceKind: k,
          sourceReport: srcRep,
          projectReport: projRep,
          name: prev.name || projRep?.name || base,
          siteKind: detectedKind,
          runtimeVersion: projRep?.recommendedRuntime ?? "",
          config: projRep?.suggested
            ? {
                ...prev.config,
                ...projRep.suggested,
              }
            : prev.config,
          newDbName: base.toLowerCase().replace(/[^a-z0-9_]/g, "_").slice(0, 32),
          envVars: prev.envVars.length ? prev.envVars : initialEnv,
        }));
      } catch (e) {
        toast("error", (e as Error).message);
      } finally {
        setBusy(false);
      }
    },
    [onChange, toast]
  );

  useEffect(() => {
    let off: (() => void) | undefined;
    let dead = false;
    onFileDrop((o, paths) => {
      setOver(o);
      const p = paths[0];
      if (p) void inspect(p, /\.zip$/i.test(p) ? "zip" : "directory");
    }).then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
    };
  }, [inspect]);

  async function pick(k: SourceKind) {
    try {
      const p = await api.pickPath(k);
      if (p) await inspect(p, k);
    } catch (e) {
      toast("error", (e as Error).message);
    }
  }

  const ok = state.path && state.sourceReport && state.sourceReport.ok;

  return (
    <div className="grid gap-4">
      <div
        className={cx(
          "grid place-items-center gap-3 rounded-card border-2 border-dashed px-6 py-8 text-center transition-colors",
          over ? "border-kelp bg-kelpsoft" : "border-line"
        )}
      >
        <p className="selectable break-all text-[15px] font-semibold">
          {state.path || "Drop your project folder or ZIP archive here"}
        </p>
        <p className="max-w-md text-[13px] text-ink2">
          Supports Static HTML/CSS/JS, Node.js applications (Express, Socket.IO, Fastify), or PHP projects.
        </p>
        <div className="flex gap-2">
          <Button onClick={() => pick("directory")}>
            <FolderOpen size={15} aria-hidden />
            Choose folder
          </Button>
          <Button onClick={() => pick("zip")}>
            <FileArchive size={15} aria-hidden />
            Choose ZIP
          </Button>
        </div>
        {!isTauri && <p className="text-[12px] text-lantern">Design preview: file dialogs return sample paths.</p>}
      </div>

      {busy && (
        <p className="flex items-center gap-2 text-ink2" role="status">
          <Loader2 size={15} className="spin" aria-hidden />
          Analyzing project structure and dependencies...
        </p>
      )}

      {state.sourceReport && (
        <div
          className={cx(
            "rise rounded-card border p-3.5",
            state.sourceReport.ok ? "border-line" : "border-buoy bg-buoysoft"
          )}
          role={state.sourceReport.ok ? "status" : "alert"}
        >
          {state.sourceReport.ok ? (
            <p className="flex items-center gap-2 font-medium">
              <CheckCircle2 size={16} className="text-kelp" aria-hidden />
              {fmtFiles(state.sourceReport.fileCount)}, {fmtBytes(state.sourceReport.sizeBytes)}
              {state.sourceReport.hasIndex ? "" : ". Note: no index.html or index.php found at root"}
            </p>
          ) : (
            <p className="font-medium text-buoy">These files cannot be imported</p>
          )}
          {[...state.sourceReport.errors, ...state.sourceReport.warnings].map((m, i) => (
            <p
              key={i}
              className={cx("mt-1.5 flex gap-2 text-[13px]", i < state.sourceReport!.errors.length ? "text-buoy" : "text-lantern")}
            >
              <AlertTriangle size={15} className="mt-0.5 shrink-0" aria-hidden />
              {m}
            </p>
          ))}
        </div>
      )}

      <div className="flex justify-end">
        <Button variant="primary" disabled={!ok} onClick={onNext}>
          Continue
        </Button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 2: Detect Project Type & Manual Override
// -------------------------------------------------------------------------------------------------
function Step2Detect({
  state,
  onChange,
  onBack,
  onNext,
}: {
  state: NewSiteState;
  onChange: React.Dispatch<React.SetStateAction<NewSiteState>>;
  onBack: () => void;
  onNext: () => void;
}) {
  const rep = state.projectReport;

  return (
    <div className="grid gap-4">
      <Card className="grid gap-3 p-4">
        <h3 className="text-[16px] font-semibold">Detected project characteristics</h3>
        <div className="grid gap-2 text-[13px]">
          <div className="flex items-center justify-between border-b border-line pb-2">
            <span className="text-ink3">Detected kind</span>
            <span className="font-semibold uppercase">{state.siteKind}</span>
          </div>
          {rep?.framework && (
            <div className="flex items-center justify-between border-b border-line pb-2">
              <span className="text-ink3">Framework / library</span>
              <span className="font-medium">{rep.framework}</span>
            </div>
          )}
          {rep?.lockfile && (
            <div className="flex items-center justify-between border-b border-line pb-2">
              <span className="text-ink3">Package lockfile</span>
              <span className="font-medium">{rep.lockfile}</span>
            </div>
          )}
          {rep?.hasServerEntry !== undefined && (
            <div className="flex items-center justify-between border-b border-line pb-2">
              <span className="text-ink3">Server entry file</span>
              <span className="font-medium">{rep.hasServerEntry ? "Found" : "None detected"}</span>
            </div>
          )}
          {rep?.usesDatabase && (
            <div className="flex items-center justify-between border-b border-line pb-2">
              <span className="text-ink3">Database usage detected</span>
              <span className="font-medium text-lantern">Database driver or references found</span>
            </div>
          )}
        </div>
      </Card>

      <div>
        <label className="mb-2 block text-[13px] font-medium">Select or override website type</label>
        <div className="grid grid-cols-3 gap-2">
          {(["static", "node", "php"] as SiteKind[]).map((k) => (
            <button
              key={k}
              type="button"
              onClick={() => onChange((s) => ({ ...s, siteKind: k }))}
              className={cx(
                "press flex flex-col items-center gap-1.5 rounded-card border p-3 text-center transition-colors",
                state.siteKind === k
                  ? "border-kelp bg-kelpsoft font-semibold text-kelp"
                  : "border-line bg-panel text-ink2 hover:border-linestrong"
              )}
            >
              {k === "static" ? <Server size={18} /> : k === "node" ? <Code2 size={18} /> : <Layers size={18} />}
              <span className="capitalize">{k === "static" ? "Static HTML/JS" : k === "node" ? "Node.js" : "PHP"}</span>
            </button>
          ))}
        </div>
      </div>

      <Field
        label="Website name"
        value={state.name}
        onChange={(e) => onChange((s) => ({ ...s, name: e.target.value }))}
        placeholder="My website"
        maxLength={60}
        hint="Only shown inside Hostify on this computer."
      />

      <div className="flex justify-between border-t border-line pt-4">
        <Button onClick={onBack}>Back</Button>
        <Button variant="primary" disabled={!state.name.trim()} onClick={onNext}>
          Continue
        </Button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 3: Configure Runtime (Node / PHP)
// -------------------------------------------------------------------------------------------------
function Step3Runtime({
  state,
  onChange,
  onBack,
  onNext,
}: {
  state: NewSiteState;
  onChange: React.Dispatch<React.SetStateAction<NewSiteState>>;
  onBack: () => void;
  onNext: () => void;
}) {
  const [runtimes, setRuntimes] = useState<RuntimeStatus[]>();
  const [err, setErr] = useState<string>();

  const kind = state.siteKind === "php" ? "php" : "node";
  const load = useCallback(() => {
    api.listRuntimes()
      .then((l) => {
        const filtered = l.filter((r) => r.kind === kind);
        setRuntimes(filtered);
        if (!state.runtimeVersion && filtered.length > 0) {
          onChange((s) => ({ ...s, runtimeVersion: filtered[0].version }));
        }
      })
      .catch((e) => setErr((e as Error).message));
  }, [kind, state.runtimeVersion, onChange]);

  useEffect(() => {
    load();
  }, [load]);

  const activeRuntime = runtimes?.find((r) => r.version === state.runtimeVersion);

  return (
    <div className="grid gap-4">
      <div>
        <label className="mb-1 block text-[13px] font-medium">{RUNTIME_NAMES[kind]} version</label>
        {runtimes ? (
          <select
            value={state.runtimeVersion}
            onChange={(e) => onChange((s) => ({ ...s, runtimeVersion: e.target.value }))}
            className="h-9 w-full rounded-ctl border border-line bg-panel px-2"
          >
            {runtimes.map((r) => (
              <option key={r.version} value={r.version}>
                {r.version} {r.ready ? "(Installed)" : "(Download required)"}
              </option>
            ))}
          </select>
        ) : (
          <Skeleton className="h-9" />
        )}
      </div>

      {activeRuntime && !activeRuntime.ready && (
        <Card className="border-lantern bg-lanternsoft p-3.5">
          <p className="flex items-center gap-2 font-medium">
            <Boxes size={16} className="text-lantern" />
            {RUNTIME_NAMES[kind]} {activeRuntime.version} needs to be installed
          </p>
          <p className="mt-1 text-[13px] text-ink2">
            Hostify will automatically download and verify the official hash-pinned runtime package.
          </p>
          <div className="mt-2">
            <RuntimeInstall kind={kind} version={activeRuntime.version} onDone={load} label="Install now" />
          </div>
        </Card>
      )}

      {kind === "node" && (
        <Card className="grid gap-3 p-4">
          <h4 className="text-[14px] font-semibold">Node.js Scripts & Dependencies</h4>
          <div className="grid gap-3 sm:grid-cols-2">
            <Select
              label="Install dependencies"
              value={state.config.install}
              onChange={(e) =>
                onChange((s) => ({
                  ...s,
                  config: { ...s.config, install: e.target.value as AppConfig["install"] },
                }))
              }
            >
              <option value="ci">npm ci (uses package-lock.json)</option>
              <option value="install">npm install</option>
              <option value="none">Don't run install</option>
            </Select>

            <Select
              label="Build script"
              value={state.config.buildScript ?? ""}
              onChange={(e) =>
                onChange((s) => ({
                  ...s,
                  config: { ...s.config, buildScript: e.target.value || null },
                }))
              }
            >
              <option value="">No build script</option>
              {Object.keys(state.projectReport?.scripts ?? {}).map((sc) => (
                <option key={sc} value={sc}>
                  npm run {sc}
                </option>
              ))}
            </Select>
          </div>
        </Card>
      )}

      {kind === "php" && (
        <Card className="grid gap-3 p-4">
          <h4 className="text-[14px] font-semibold">PHP Environment & Document Root</h4>
          <Field
            label="Document root folder"
            value={state.config.documentRoot}
            spellCheck={false}
            onChange={(e) =>
              onChange((s) => ({
                ...s,
                config: { ...s.config, documentRoot: e.target.value },
              }))
            }
            hint="Subdirectory holding index.php, e.g. public or dist. Leave blank if in the project root."
          />
        </Card>
      )}

      {err && <p role="alert" className="selectable text-[13px] text-buoy">{err}</p>}

      <div className="flex justify-between border-t border-line pt-4">
        <Button onClick={onBack}>Back</Button>
        <Button variant="primary" disabled={!state.runtimeVersion} onClick={onNext}>
          Continue to configuration
        </Button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 4: Application Configuration
// -------------------------------------------------------------------------------------------------
function Step4AppConfig({
  state,
  onChange,
  onBack,
  onNext,
}: {
  state: NewSiteState;
  onChange: React.Dispatch<React.SetStateAction<NewSiteState>>;
  onBack: () => void;
  onNext: () => void;
}) {
  return (
    <div className="grid gap-4">
      <AppConfigForm
        kind={state.siteKind}
        config={state.config}
        scripts={state.projectReport?.scripts}
        installScripts={state.projectReport?.dependenciesWithInstallScripts}
        onChange={(cfg) => onChange((s) => ({ ...s, config: cfg }))}
      />

      <div className="flex justify-between border-t border-line pt-4">
        <Button onClick={onBack}>Back</Button>
        <Button variant="primary" onClick={onNext}>
          Continue to database
        </Button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 5: Database Setup
// -------------------------------------------------------------------------------------------------
function Step5Database({
  state,
  onChange,
  onBack,
  onNext,
}: {
  state: NewSiteState;
  onChange: React.Dispatch<React.SetStateAction<NewSiteState>>;
  onBack: () => void;
  onNext: () => void;
}) {
  const toast = useStore((s) => s.toast);
  const [mysql, setMysql] = useState<MysqlStatus>();
  const [dbs, setDbs] = useState<ManagedDatabase[]>([]);
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string>();

  const load = useCallback(async () => {
    try {
      const s = await api.mysqlStatus();
      setMysql(s);
      if (s.setUp) {
        setDbs(await api.listDatabases());
      }
    } catch {
      // ignore
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  async function setupMysql() {
    setBusy(true);
    try {
      await api.mysqlSetup("8.4.10");
      toast("ok", "MySQL initialized successfully");
      await load();
    } catch (e) {
      toast("error", (e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function testSelected(dbName: string) {
    setTesting(true);
    setTestResult(undefined);
    try {
      const r = await api.testConnection(dbName);
      setTestResult(r.ok ? `Connected successfully (${r.elapsedMs}ms)` : `Failed: ${r.detail}`);
    } catch (e) {
      setTestResult((e as Error).message);
    } finally {
      setTesting(false);
    }
  }

  return (
    <div className="grid gap-4">
      <div>
        <label className="mb-2 block text-[13px] font-medium">Database configuration</label>
        <div className="grid gap-2">
          <label className="flex items-center gap-2.5 rounded-card border border-line p-3 hover:border-linestrong">
            <input
              type="radio"
              name="dbChoice"
              checked={state.dbChoice === "none"}
              onChange={() => onChange((s) => ({ ...s, dbChoice: "none" }))}
            />
            <div>
              <span className="font-semibold">No database</span>
              <p className="text-[13px] text-ink2">This project does not require a database connection.</p>
            </div>
          </label>

          <label className="flex items-center gap-2.5 rounded-card border border-line p-3 hover:border-linestrong">
            <input
              type="radio"
              name="dbChoice"
              checked={state.dbChoice === "new"}
              onChange={() => onChange((s) => ({ ...s, dbChoice: "new" }))}
            />
            <div>
              <span className="font-semibold">Create a new MySQL database</span>
              <p className="text-[13px] text-ink2">
                Hostify will create an isolated database and least-privilege user, injecting DB_* environment variables.
              </p>
            </div>
          </label>

          <label className="flex items-center gap-2.5 rounded-card border border-line p-3 hover:border-linestrong">
            <input
              type="radio"
              name="dbChoice"
              checked={state.dbChoice === "existing"}
              onChange={() => onChange((s) => ({ ...s, dbChoice: "existing" }))}
            />
            <div>
              <span className="font-semibold">Connect to existing local database</span>
              <p className="text-[13px] text-ink2">Link one of your existing managed databases to this website.</p>
            </div>
          </label>
        </div>
      </div>

      {state.dbChoice === "new" && (
        <Card className="grid gap-3 p-4">
          <h4 className="text-[14px] font-semibold">New database details</h4>
          {mysql && !mysql.setUp && (
            <div className="rounded-card border border-lantern bg-lanternsoft p-3">
              <p className="font-medium text-lantern">MySQL needs initial setup</p>
              <div className="mt-2">
                <Button size="sm" variant="primary" busy={busy} onClick={setupMysql}>
                  Initialize MySQL now
                </Button>
              </div>
            </div>
          )}
          <Field
            label="Database name"
            value={state.newDbName}
            spellCheck={false}
            autoCapitalize="off"
            onChange={(e) => onChange((s) => ({ ...s, newDbName: e.target.value.toLowerCase().replace(/[^a-z0-9_]/g, "") }))}
            hint="Letters, numbers and underscores only (max 48 chars). User will be named <name>_app."
          />
        </Card>
      )}

      {state.dbChoice === "existing" && (
        <Card className="grid gap-3 p-4">
          <h4 className="text-[14px] font-semibold">Choose an existing database</h4>
          {dbs.length === 0 ? (
            <p className="text-[13px] text-ink2">No managed databases exist yet. Switch to "Create new" above.</p>
          ) : (
            <div className="grid gap-2">
              <select
                value={state.selectedDb}
                onChange={(e) => onChange((s) => ({ ...s, selectedDb: e.target.value }))}
                className="h-9 rounded-ctl border border-line bg-panel px-2"
              >
                <option value="">Select a database...</option>
                {dbs.map((d) => (
                  <option key={d.name} value={d.name}>
                    {d.name} {d.siteName ? `(Used by ${d.siteName})` : "(Unlinked)"}
                  </option>
                ))}
              </select>
              {state.selectedDb && (
                <div className="flex items-center gap-2 pt-1">
                  <Button size="sm" busy={testing} onClick={() => testSelected(state.selectedDb)}>
                    Test connection
                  </Button>
                  {testResult && <span className="text-[13px] text-ink2">{testResult}</span>}
                </div>
              )}
            </div>
          )}
        </Card>
      )}

      <div className="flex justify-between border-t border-line pt-4">
        <Button onClick={onBack}>Back</Button>
        <Button
          variant="primary"
          disabled={
            state.dbChoice === "new"
              ? !state.newDbName || (mysql && !mysql.setUp)
              : state.dbChoice === "existing"
                ? !state.selectedDb
                : false
          }
          onClick={onNext}
        >
          Continue to environment variables
        </Button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 6: Environment Variables
// -------------------------------------------------------------------------------------------------
function Step6EnvVars({
  state,
  onChange,
  onBack,
  onNext,
}: {
  state: NewSiteState;
  onChange: React.Dispatch<React.SetStateAction<NewSiteState>>;
  onBack: () => void;
  onNext: () => void;
}) {
  const [key, setKey] = useState("");
  const [val, setVal] = useState("");
  const [secret, setSecret] = useState(false);
  const [envPaste, setEnvPaste] = useState("");
  const [showPaste, setShowPaste] = useState(false);

  function addVar() {
    if (!key.trim() || !NAME_RE.test(key.trim())) return;
    onChange((s) => ({
      ...s,
      envVars: [...s.envVars.filter((v) => v.name !== key.trim()), { name: key.trim(), value: val, secret }],
    }));
    setKey("");
    setVal("");
    setSecret(false);
  }

  function removeVar(name: string) {
    onChange((s) => ({ ...s, envVars: s.envVars.filter((v) => v.name !== name) }));
  }

  function parsePaste() {
    const lines = envPaste.split("\n");
    const parsed: EnvVar[] = [];
    for (const l of lines) {
      const trimmed = l.trim();
      if (!trimmed || trimmed.startsWith("#")) continue;
      const idx = trimmed.indexOf("=");
      if (idx > 0) {
        const k = trimmed.slice(0, idx).trim();
        let v = trimmed.slice(idx + 1).trim();
        if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) {
          v = v.slice(1, -1);
        }
        if (NAME_RE.test(k)) {
          parsed.push({ name: k, value: v, secret: /SECRET|KEY|TOKEN|PASS/i.test(k) });
        }
      }
    }
    onChange((s) => ({
      ...s,
      envVars: [
        ...s.envVars.filter((ev) => !parsed.some((p) => p.name === ev.name)),
        ...parsed,
      ],
    }));
    setEnvPaste("");
    setShowPaste(false);
  }

  return (
    <div className="grid gap-4">
      <div className="flex items-center justify-between">
        <h3 className="text-[16px] font-semibold">Environment Variables & Secrets</h3>
        <Button size="sm" onClick={() => setShowPaste(!showPaste)}>
          {showPaste ? "Hide .env paste" : "Import .env text"}
        </Button>
      </div>

      {showPaste && (
        <Card className="grid gap-2 p-3">
          <textarea
            className="h-28 w-full rounded-ctl border border-line bg-panel p-2 font-mono text-[12px]"
            placeholder="PORT=3000&#10;API_KEY=xyz&#10;DEBUG=false"
            value={envPaste}
            onChange={(e) => setEnvPaste(e.target.value)}
          />
          <div className="flex justify-end">
            <Button size="sm" variant="primary" onClick={parsePaste}>
              Parse and add variables
            </Button>
          </div>
        </Card>
      )}

      <Card className="p-3">
        <div className="grid gap-2 sm:grid-cols-[1fr_1.5fr_auto_auto]">
          <Field
            label="Name"
            value={key}
            placeholder="API_KEY"
            spellCheck={false}
            autoCapitalize="off"
            onChange={(e) => {
              setKey(e.target.value);
              if (/SECRET|TOKEN|PASS|KEY/i.test(e.target.value)) setSecret(true);
            }}
          />
          <Field
            label="Value"
            value={val}
            placeholder="value"
            spellCheck={false}
            autoCapitalize="off"
            type={secret ? "password" : "text"}
            onChange={(e) => setVal(e.target.value)}
          />
          <label className="flex items-center gap-1.5 pt-6 text-[13px]">
            <input type="checkbox" checked={secret} onChange={(e) => setSecret(e.target.checked)} />
            <span>Secret</span>
          </label>
          <div className="pt-5">
            <Button size="sm" onClick={addVar} disabled={!key.trim() || !NAME_RE.test(key.trim())}>
              <Plus size={14} /> Add
            </Button>
          </div>
        </div>
      </Card>

      {state.envVars.length > 0 ? (
        <ul className="divide-y divide-line rounded-card border border-line p-2">
          {state.envVars.map((v) => (
            <li key={v.name} className="flex items-center justify-between py-1.5 text-[13px]">
              <div className="flex items-center gap-2">
                <span className="font-mono font-semibold">{v.name}</span>
                {v.secret && <Pill tone="idle">Secret (DPAPI encrypted)</Pill>}
              </div>
              <div className="flex items-center gap-3">
                <span className="font-mono text-ink2">{v.secret ? "••••••••" : v.value}</span>
                <button
                  type="button"
                  onClick={() => removeVar(v.name)}
                  className="press text-ink3 hover:text-buoy"
                  aria-label={`Remove ${v.name}`}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-[13px] text-ink2">No variables configured yet.</p>
      )}

      <div className="flex justify-between border-t border-line pt-4">
        <Button onClick={onBack}>Back</Button>
        <Button variant="primary" onClick={onNext}>
          Create and verify preview
        </Button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 7: Local Preview & Build
// -------------------------------------------------------------------------------------------------
function Step7LocalPreview({
  state,
  site,
  onSiteReady,
  onBack,
  onLocalOnly,
  onNext,
}: {
  state: NewSiteState;
  site?: Site;
  onSiteReady: (s: Site) => Promise<void>;
  onBack?: () => void;
  onLocalOnly: () => void;
  onNext: () => void;
}) {
  const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const [buildDone, setBuildDone] = useState(false);
  const [created, setCreated] = useState<Site | undefined>(site);
  const [opened, setOpened] = useState(false);
  const onSiteReadyRef = useRef(onSiteReady);
  onSiteReadyRef.current = onSiteReady;
  const didInit = useRef(false);

  useEffect(() => {
    if (created || didInit.current) return;
    didInit.current = true;
    let dead = false;

    async function init() {
      setBusy(true);
      try {
        const s = await api.addSite(
          state.name.trim(),
          state.path,
          state.sourceKind,
          state.siteKind,
          state.siteKind !== "static" ? state.config : undefined
        );
        if (dead) return;
        setCreated(s);
        await onSiteReadyRef.current(s);

        for (const ev of state.envVars) {
          if (ev.value) {
            await api.setEnvVar(s.id, ev.name, ev.value, ev.secret);
          }
        }

        if (state.dbChoice === "new" && state.newDbName) {
          await api.createDatabase(state.newDbName, s.id, true, true);
        } else if (state.dbChoice === "existing" && state.selectedDb) {
          await api.linkDatabase(state.selectedDb, s.id, true, true);
        }

        if (s.kind === "static" || !s.kind) {
          await api.startSite(s.id);
          setBuildDone(true);
        }
      } catch (e) {
        if (!dead) toast("error", (e as Error).message);
      } finally {
        setBusy(false);
      }
    }

    void init();
    return () => {
      dead = true;
    };
  }, [created, state, toast]);

  async function openPreview() {
    if (!created) return;
    try {
      if (created.status !== "running") {
        await api.startSite(created.id);
      }
      await api.openExternal(created.localUrl);
      setOpened(true);
    } catch (e) {
      toast("error", (e as Error).message);
    }
  }

  if (busy || !created) {
    return (
      <div className="grid place-items-center gap-3 py-12" role="status">
        <Loader2 size={24} className="spin text-kelp" />
        <p className="font-semibold">Setting up and initializing {state.name}...</p>
      </div>
    );
  }

  const isDynamic = created.kind && created.kind !== "static";

  return (
    <div className="grid gap-4">
      <div className="flex items-center gap-3">
        <CheckCircle2 className="shrink-0 text-kelp" size={20} aria-hidden />
        <div className="min-w-0">
          <p className="font-semibold">{created.name} is installed locally</p>
          <p className="selectable truncate font-mono text-[13px] text-ink2">{created.localUrl}</p>
        </div>
      </div>

      {isDynamic && (
        <Card className="p-4">
          <h4 className="mb-2 text-[14px] font-semibold">Application build & verification</h4>
          <BuildPanel
            site={created}
            lifecycle={state.projectReport?.lifecycleScripts}
            onBuilt={() => setBuildDone(true)}
            compact={false}
          />
        </Card>
      )}

      <div>
        <Button variant={opened ? "secondary" : "primary"} onClick={openPreview}>
          <ExternalLink size={15} aria-hidden />
          Preview in local browser
        </Button>
      </div>

      <div className="flex justify-between border-t border-line pt-4">
        {onBack ? <Button onClick={onBack}>Back</Button> : <span />}
        <div className="flex gap-2">
          <Button onClick={onLocalOnly}>Keep local only</Button>
          <Button
            variant={opened || buildDone ? "primary" : "secondary"}
            onClick={onNext}
          >
            Continue to domain
          </Button>
        </div>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 8: Domain Selection
// -------------------------------------------------------------------------------------------------
function Step8Domain({
  site,
  hostname,
  setHostname,
  onBack,
  onNext,
}: {
  site: Site;
  hostname: string;
  setHostname: (v: string) => void;
  onBack?: () => void;
  onNext: () => void;
}) {
  const cf = useStore((s) => s.cf);
  const [connect, setConnect] = useState(false);
  const [check, setCheck] = useState<DomainCheck>();
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [touched, setTouched] = useState(false);

  const host = cleanHost(hostname);
  const valid = HOST_RE.test(host);

  async function verify() {
    setBusy(true);
    setErr(undefined);
    setCheck(undefined);
    try {
      const c = await api.cfCheckDomain(host);
      setCheck(c);
      if (c.problems.length === 0 && c.canManageDns) onNext();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  if (!cf.connected) {
    return (
      <div className="grid gap-4">
        <div className="flex gap-3">
          <Globe2 className="mt-0.5 shrink-0 text-ink2" size={18} aria-hidden />
          <div>
            <p className="font-medium">Connect your Cloudflare account</p>
            <p className="mt-0.5 text-[13px] text-ink2">
              Your domain's DNS needs to be on Cloudflare. Hostify uses an encrypted scoped API token from Windows Credential Manager.
            </p>
          </div>
        </div>
        <div className="flex justify-between">
          {onBack ? <Button onClick={onBack}>Back</Button> : <span />}
          <Button variant="primary" onClick={() => setConnect(true)}>
            Connect Cloudflare
          </Button>
        </div>
        {connect && <ConnectCloudflare onClose={() => setConnect(false)} />}
      </div>
    );
  }

  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (valid && !busy) void verify();
      }}
    >
      <div className="text-[13px] text-ink2">
        Connecting domain for <span className="font-semibold text-ink">{site.name}</span>
      </div>
      <Field
        label="Domain"
        value={hostname}
        onChange={(e) => {
          setHostname(e.target.value);
          setCheck(undefined);
          setErr(undefined);
        }}
        onBlur={() => setTouched(true)}
        placeholder="app.example.com"
        autoCapitalize="off"
        spellCheck={false}
        error={touched && hostname && !valid ? "Enter a full domain, like www.example.com." : err}
        hint="Hostify validates DNS zone permissions with your Cloudflare token."
      />

      {check && check.problems.length > 0 && (
        <div className="rounded-card border border-buoy bg-buoysoft p-3.5" role="alert">
          <p className="font-medium text-buoy">This domain cannot be connected yet</p>
          <ul className="mt-1.5 grid gap-1 text-[13px]">
            {check.problems.map((p, i) => (
              <li key={i}>{p}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex justify-between border-t border-line pt-4">
        {onBack ? <Button onClick={onBack}>Back</Button> : <span />}
        <Button type="submit" variant="primary" busy={busy} disabled={!valid}>
          Check domain
        </Button>
      </div>
    </form>
  );
}

// -------------------------------------------------------------------------------------------------
// Step 9: Publish & Public Verification
// -------------------------------------------------------------------------------------------------
function Step9Publish({
  site,
  hostname,
  onDone,
  onBack,
}: {
  site: Site;
  hostname: string;
  onDone: () => void;
  onBack: () => void;
}) {
  const setDeployment = useStore((s) => s.setDeployment);
  const live = useStore((s) => s.sites.find((x) => x.id === site.id));
  const host = cleanHost(hostname);
  const [plan, setPlan] = useState<DnsPlan>();
  const [consent, setConsent] = useState(false);

  const resumable =
    live?.publishDeploymentId && (isActive(live.publishState) || holdsResources(live.publishState))
      ? live.publishDeploymentId
      : undefined;

  const [depId, setDepId] = useState<string | undefined>(resumable);
  const [starting, setStarting] = useState<string>();
  const [err, setErr] = useState<string>();
  const dep = useStore((s) => (depId ? s.deployments[depId] : undefined));

  useEffect(() => {
    if (depId && !dep) api.getDeployment(depId).then(setDeployment).catch((e) => setErr((e as Error).message));
  }, [depId, dep, setDeployment]);

  useEffect(() => {
    if (!depId) api.cfPlanDns(site.id, host).then(setPlan).catch((e) => setErr((e as Error).message));
  }, [site.id, host, depId]);

  async function go() {
    setErr(undefined);
    try {
      if (!useStore.getState().engine?.componentsReady) {
        setStarting("Installing components");
        await useStore.getState().installComponents();
      }
      setStarting("Starting tunnel");
      const d = await api.publishSite(site.id, host, consent);
      setDeployment(d);
      setDepId(d.id);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setStarting(undefined);
    }
  }

  if (dep) {
    return (
      <div className="rise">
        <PublishProgress
          deployment={dep}
          onDone={onDone}
          onRepublish={() => {
            setDepId(undefined);
            setConsent(false);
          }}
        />
      </div>
    );
  }

  return (
    <div className="grid gap-4">
      <div className="rounded-card border border-line p-3.5">
        <div className="text-[13px] text-ink3">Target website</div>
        <div className="font-semibold">{site.name}</div>
        <div className="mt-2 text-[13px] text-ink3">Public HTTPS URL</div>
        <div className="selectable break-all font-semibold text-kelp">https://{host}</div>
      </div>

      {plan ? (
        <div
          className={cx(
            "rise rounded-card border p-3.5",
            plan.action === "conflict" ? "border-buoy bg-buoysoft" : "border-line"
          )}
          role={plan.action === "conflict" ? "alert" : undefined}
        >
          <p className="font-medium">
            {plan.action === "conflict"
              ? "A DNS record already exists for this domain"
              : "Planned DNS record modification"}
          </p>
          <p className="mt-1 text-[13px] text-ink2">{plan.message}</p>
          {plan.existing && (
            <p className="selectable mt-1.5 font-mono text-[12px]">
              {plan.existing.type}: {plan.existing.content}
            </p>
          )}
          {plan.action === "conflict" && (
            <p className="mt-1.5 text-[13px] text-ink2">
              Hostify will never overwrite existing unmanaged records. Please choose a different subdomain.
            </p>
          )}
        </div>
      ) : !err ? (
        <p className="flex items-center gap-2 text-ink2" role="status">
          <Loader2 size={15} className="spin" aria-hidden />
          Analyzing Cloudflare DNS records...
        </p>
      ) : null}

      {plan && plan.action !== "conflict" && (
        <label className="flex items-start gap-3 text-[13px]">
          <input
            type="checkbox"
            className="mt-1"
            checked={consent}
            onChange={(e) => setConsent(e.target.checked)}
          />
          <span>
            I authorize Hostify to add the required CNAME tunnel record in my Cloudflare DNS zone.
          </span>
        </label>
      )}

      {err && <p className="selectable text-[13px] text-buoy" role="alert">{err}</p>}

      <div className="flex justify-between border-t border-line pt-4">
        <Button onClick={onBack}>Back</Button>
        <Button
          variant="primary"
          busy={!!starting}
          disabled={!consent || !plan || plan.action === "conflict"}
          onClick={go}
        >
          {starting ?? "Publish website"}
        </Button>
      </div>
    </div>
  );
}
