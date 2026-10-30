import { AlertTriangle } from "lucide-react";
import { Disclosure, Field, Select } from "./ui";
import type { AppConfig, SiteKind } from "../lib/types";

/** Tokens are split on spaces only for display; the engine receives an argument list and never a command string. */
const startText = (c: AppConfig) => (c.start.type === "npmScript" ? `npm run ${c.start.name}` : `node ${c.start.args.join(" ")}`);
function parseStart(text: string): AppConfig["start"] | undefined {
  const parts = text.trim().split(/\s+/).filter(Boolean);
  if (parts[0] === "npm" && parts[1] === "run" && parts[2] && parts.length === 3) return { type: "npmScript", name: parts[2] };
  if (parts[0] === "node" && parts.length > 1) return { type: "node", args: parts.slice(1) };
  return undefined;
}
const PHP_EXTENSIONS = ["mbstring", "openssl", "curl", "fileinfo", "mysqli", "pdo_mysql", "gd", "intl", "exif", "zip", "soap", "sodium", "pdo_sqlite", "sqlite3"];

export function StartField({ config, onChange }: { config: AppConfig; onChange: (c: AppConfig) => void }) {
  return (
    <Field key={startText(config)} label="Start command" defaultValue={startText(config)} spellCheck={false} autoCapitalize="off"
      hint="npm run <script> or node <file>. Use {PORT} for the port, for example node app.js --port {PORT}. Hostify never runs this through a shell."
      onBlur={(e) => { const start = parseStart(e.target.value); if (start) onChange({ ...config, start }); else e.target.value = startText(config); }} />
  );
}

export function AppConfigForm({ kind, config, scripts, installScripts, onChange }: {
  kind: SiteKind; config: AppConfig; scripts?: Record<string, string>; installScripts?: string[]; onChange: (c: AppConfig) => void;
}) {
  const set = (p: Partial<AppConfig>) => onChange({ ...config, ...p });
  const paths = (v: string) => v.split(",").map((x) => x.trim()).filter(Boolean);
  const setting = (key: string, value: string) => { const phpSettings = { ...config.phpSettings }; if (value) phpSettings[key] = value; else delete phpSettings[key]; set({ phpSettings }); };
  return (
    <div className="grid gap-4">
      {kind === "node" && <>
        <div className="grid gap-4 sm:grid-cols-2">
          <Select label="Install dependencies" value={config.install} onChange={(e) => set({ install: e.target.value as AppConfig["install"] })}>
            <option value="ci">npm ci (uses your lockfile)</option><option value="install">npm install</option><option value="none">Don't install</option></Select>
          <Select label="Build script" value={config.buildScript ?? ""} onChange={(e) => set({ buildScript: e.target.value || null })}>
            <option value="">No build</option>{Object.keys(scripts ?? {}).concat(config.buildScript && !scripts?.[config.buildScript] ? [config.buildScript] : []).map((s) => <option key={s} value={s}>{s}</option>)}</Select>
        </div>
        <StartField config={config} onChange={onChange} />
      </>}
      {kind === "php" && <Field label="Website folder" value={config.documentRoot} spellCheck={false} onChange={(e) => set({ documentRoot: e.target.value })} hint="The folder holding index.php, relative to your project." />}

      <Disclosure title="Advanced settings">
        <div className="grid gap-4 pt-2">
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Health check path" value={config.healthPath} spellCheck={false} onChange={(e) => set({ healthPath: e.target.value })} hint="Hostify asks this address to confirm the app is alive." />
            <Select label="If the app stops" value={config.restart} onChange={(e) => set({ restart: e.target.value as AppConfig["restart"] })}>
              <option value="always">Always restart</option><option value="onFailure">Restart only after a crash</option><option value="never">Leave it stopped</option></Select>
          </div>
          <Field label="Keep across updates" value={config.persistentPaths.join(", ")} spellCheck={false} onChange={(e) => set({ persistentPaths: paths(e.target.value) })} hint="Folders your app writes to, such as uploads. Separate with commas." />
          {kind === "node" && <>
            <Field label="Working folder" value={config.workingDir} spellCheck={false} onChange={(e) => set({ workingDir: e.target.value })} hint="Relative to your project." />
            <Field label="Memory limit in MB" type="number" min={128} max={65536} value={config.memoryLimitMb ?? ""} onChange={(e) => set({ memoryLimitMb: e.target.value ? Number(e.target.value) : null })} hint="Leave empty for no limit. Windows refuses memory beyond it, which can make the app fail." />
            <label className="flex items-start gap-2.5"><input type="checkbox" className="mt-1" checked={config.allowInstallScripts} onChange={(e) => set({ allowInstallScripts: e.target.checked })} />
              <span>Allow install scripts<span className="block text-[13px] text-ink2">Packages can run their own programs on this PC during install. Some packages with native code need this. Secret variables are withheld from this step.</span></span></label>
            {installScripts && installScripts.length > 0 && !config.allowInstallScripts && (
              <p className="flex gap-2 text-[13px] text-lantern"><AlertTriangle size={15} className="mt-0.5 shrink-0" aria-hidden />These want to run install scripts and may not work without them: {installScripts.join(", ")}.</p>)}
          </>}
          {kind === "php" && <>
            <Field label="Simultaneous requests" type="number" min={1} max={16} value={config.phpWorkers} onChange={(e) => set({ phpWorkers: Number(e.target.value) })} hint="One PHP process runs per request, and each one uses memory." />
            <div className="grid gap-4 sm:grid-cols-2">
              {[["memory_limit", "Memory per request", "256M"], ["upload_max_filesize", "Largest upload", "64M"], ["post_max_size", "Largest form post", "64M"], ["max_execution_time", "Time limit in seconds", "60"]].map(([key, label, ph]) => (
                <Field key={key} label={label} value={config.phpSettings[key] ?? ""} placeholder={ph} spellCheck={false} onChange={(e) => setting(key, e.target.value)} />))}
            </div>
            <fieldset className="grid gap-1.5"><legend className="mb-1 text-[13px] font-medium">PHP extensions</legend>
              <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-4">{PHP_EXTENSIONS.map((x) => (
                <label key={x} className="flex items-center gap-2 text-[13px]"><input type="checkbox" checked={config.phpExtensions.includes(x)}
                  onChange={(e) => set({ phpExtensions: e.target.checked ? [...config.phpExtensions, x] : config.phpExtensions.filter((v) => v !== x) })} />{x}</label>))}</div>
            </fieldset>
          </>}
        </div>
      </Disclosure>
    </div>
  );
}
