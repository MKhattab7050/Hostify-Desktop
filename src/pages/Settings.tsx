import { useStore } from "../store";
import { Card, cx } from "../components/ui";
import type { AppSettings } from "../lib/types";

function Toggle({ label, desc, on, onChange }: { label: string; desc: string; on: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between gap-6 py-3.5">
      <div><div className="font-medium">{label}</div><div className="text-ink2">{desc}</div></div>
      <button role="switch" aria-checked={on} aria-label={label} onClick={() => onChange(!on)}
        className={cx("relative h-6 w-11 shrink-0 rounded-full transition-colors", on ? "bg-kelp" : "bg-line")}>
        <span className={cx("absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-[left]", on ? "left-[22px]" : "left-0.5")} />
      </button>
    </div>
  );
}

export function Settings() {
  const { settings, engine, saveSettings } = useStore();
  if (!settings) return <p className="text-ink2">Loading settings…</p>;
  const set = (p: Partial<AppSettings>) => saveSettings({ ...settings, ...p });
  return (
    <div className="mx-auto grid max-w-[760px] gap-5">
      <h1 className="text-2xl font-bold">Settings</h1>
      <Card className="divide-y divide-line px-5">
        <Toggle label="Start hosting when I sign in to Windows" desc="Your websites come back online automatically after a restart." on={settings.launchOnLogin} onChange={(v) => set({ launchOnLogin: v })} />
        <Toggle label="Keep hosting when the window is closed" desc="Closing Hostify hides it to the tray. Your websites stay online." on={settings.closeToTray} onChange={(v) => set({ closeToTray: v })} />
      </Card>
      <Card className="p-5">
        <div className="mb-3 font-medium">Appearance</div>
        <div className="inline-flex rounded-ctl border border-line p-0.5" role="radiogroup" aria-label="Theme">
          {(["system", "light", "dark"] as const).map((t) => (
            <button key={t} role="radio" aria-checked={settings.theme === t} onClick={() => set({ theme: t })}
              className={cx("h-8 rounded-md px-4 capitalize", settings.theme === t ? "bg-kelp text-kelpink" : "text-ink2 hover:text-ink")}>{t}</button>))}
        </div>
      </Card>
      <Card className="p-5">
        <div className="mb-2 font-medium">About this installation</div>
        <dl className="grid grid-cols-[160px_1fr] gap-y-1.5 text-ink2">
          <dt>Hostify engine</dt><dd className="selectable">{engine?.running ? `Running (${engine.mode} mode) · v${engine.version}` : "Not running"}</dd>
          <dt>Web server</dt><dd className="selectable">{engine?.caddyVersion ?? "Not installed yet"}</dd>
          <dt>Tunnel connector</dt><dd className="selectable">{engine?.cloudflaredVersion ?? "Not installed yet"}</dd>
        </dl>
      </Card>
    </div>
  );
}
