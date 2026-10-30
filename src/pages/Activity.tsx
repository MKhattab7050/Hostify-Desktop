import { useEffect, useMemo, useState } from "react";
import { Clipboard, ScrollText, Search } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { redact } from "../lib/publish";
import { Button, Card, Empty, PageHeader, Select, Skeleton, cx, fmtTime } from "../components/ui";
import type { ActivityEntry } from "../lib/types";

const LEVELS = [["all", "All severities"], ["error", "Errors"], ["warn", "Warnings"], ["info", "Information"]] as const;

export function Activity() {
  const sites = useStore((s) => s.sites); const toast = useStore((s) => s.toast);
  const pushed = useStore((s) => s.activity);
  const [history, setHistory] = useState<ActivityEntry[]>();
  const [site, setSite] = useState("all"); const [level, setLevel] = useState("all"); const [q, setQ] = useState("");
  const [logs, setLogs] = useState<string[]>();

  useEffect(() => { let live = true; api.listActivity(1000).then((r) => live && setHistory(r)).catch(() => live && setHistory([])); return () => { live = false; }; }, []);
  useEffect(() => {
    setLogs(undefined); if (site === "all") return;
    let live = true; const load = () => { if (!document.hidden) api.siteLogs(site, 300).then((l) => live && setLogs(l)).catch(() => live && setLogs([])); };
    load(); const t = setInterval(load, 5000); return () => { live = false; clearInterval(t); };
  }, [site]);

  // Entries pushed since the page opened sit on top of the history it loaded.
  const all = useMemo(() => { const seen = new Set<number>(); return [...pushed, ...(history ?? [])].filter((a) => !seen.has(a.id) && !!seen.add(a.id)).sort((a, b) => b.id - a.id); }, [pushed, history]);
  const rows = useMemo(() => { const n = q.trim().toLowerCase(); return all.filter((a) => (site === "all" || a.siteId === site) && (level === "all" || a.level === level) && (!n || a.message.toLowerCase().includes(n))).slice(0, 500); }, [all, site, level, q]);
  const nameOf = (id?: string) => sites.find((s) => s.id === id)?.name;

  async function copy() {
    const text = redact(rows.map((a) => `${a.ts} ${a.level} ${nameOf(a.siteId) ?? "-"} ${a.message}`).join("\n"));
    try { await navigator.clipboard.writeText(text); toast("ok", `Copied ${rows.length} entries with secrets removed`); }
    catch { toast("error", "Windows blocked clipboard access. Select the entries and copy them."); }
  }
  const filtered = site !== "all" || level !== "all" || !!q;

  return (
    <div className="mx-auto grid max-w-[1040px] gap-4">
      <PageHeader title="Activity & Logs" sub="What Hostify did and what went wrong." />
      <div className="flex flex-wrap items-center gap-3">
        <label className="relative min-w-[200px] flex-1"><span className="sr-only">Search activity</span>
          <Search size={14} className="pointer-events-none absolute left-2.5 top-[9px] text-ink3" aria-hidden />
          <input type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" className="h-8 w-full rounded-ctl border border-line bg-panel pl-8 pr-2 text-ink placeholder:text-ink3 focus:border-kelp" /></label>
        <Select label="Website" value={site} onChange={(e) => setSite(e.target.value)}><option value="all">All websites</option>{sites.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}</Select>
        <Select label="Severity" value={level} onChange={(e) => setLevel(e.target.value)}>{LEVELS.map(([v, t]) => <option key={v} value={v}>{t}</option>)}</Select>
        <Button size="sm" disabled={rows.length === 0} onClick={copy}><Clipboard size={13} aria-hidden />Copy diagnostics</Button>
      </div>
      <Card>
        {!history ? <div className="grid gap-3 p-4" role="status" aria-label="Loading activity"><Skeleton className="h-5" /><Skeleton className="h-5" /><Skeleton className="h-5" /></div>
          : rows.length === 0 ? (filtered
            ? <Empty icon={<Search size={20} />} title="Nothing matches these filters" body="Try another search, or show all severities and websites." action={<Button onClick={() => { setSite("all"); setLevel("all"); setQ(""); }}>Clear filters</Button>} />
            : <Empty icon={<ScrollText size={20} />} title="No activity yet" body="Starting, stopping, publishing and errors are recorded here." />)
          : (
          <ul className="max-h-[56vh] overflow-y-auto">{rows.map((a) => (
            <li key={a.id} className="flex gap-3 border-b border-line px-4 py-2.5 last:border-0">
              <span className={cx("mt-[7px] h-1.5 w-1.5 shrink-0 rounded-full", a.level === "error" ? "bg-buoy" : a.level === "warn" ? "bg-lantern" : "bg-kelp")} role="img" aria-label={a.level} />
              <p className="selectable min-w-0 flex-1 break-words">{a.message}{site === "all" && nameOf(a.siteId) && <span className="ml-2 text-[13px] text-ink3">{nameOf(a.siteId)}</span>}</p>
              <time className="tnum shrink-0 text-[13px] text-ink3" dateTime={a.ts}>{fmtTime(a.ts)}</time>
            </li>))}</ul>
        )}
      </Card>
      {site !== "all" && (
        <Card><h2 className="border-b border-line px-4 py-2.5 text-[16px] font-semibold">Server log</h2>
          {logs ? <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-all p-4 font-mono text-[12px] leading-5 text-ink2">{redact(logs.join("\n")) || "No requests logged yet."}</pre> : <div className="p-4"><Skeleton className="h-24" /></div>}</Card>
      )}
    </div>
  );
}
