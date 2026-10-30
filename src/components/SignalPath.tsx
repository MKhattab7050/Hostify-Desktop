import { Cloud, Globe, Laptop, Waypoints } from "lucide-react";
import type { ReactNode } from "react";
import { cx } from "./ui";
import { isVerifying } from "../lib/publish";
import type { Site, TunnelHealth } from "../lib/types";

type Hop = "ok" | "idle" | "bad" | "wait";

/** Derive each hop's state from real data only. Anything we cannot know stays "idle". */
export function hopStates(site: Site, tunnel?: TunnelHealth): { pc: Hop; tunnel: Hop; edge: Hop; domain: Hop } {
  const pc: Hop = site.status === "running" ? "ok" : site.status === "error" ? "bad" : "idle";
  const published = !!site.publicUrl;
  const waiting = isVerifying(site.publishState) || site.publishState === "recovery_required";
  const tun: Hop = !published && !waiting ? "idle" : tunnel ? (tunnel.connected ? "ok" : "bad") : "idle";
  if (waiting) return { pc, tunnel: tun, edge: tun === "ok" ? "wait" : "idle", domain: site.publishState === "recovery_required" ? "bad" : "wait" };
  const edge: Hop = tun === "ok" ? "ok" : tun === "bad" ? "bad" : "idle";
  const domain: Hop = published && pc === "ok" && tun === "ok" ? "ok" : published && (tun === "bad" || pc !== "ok") ? "bad" : "idle";
  return { pc, tunnel: tun, edge, domain };
}

const WORD: Record<Hop, string> = { ok: "working", bad: "problem", idle: "not active", wait: "waiting" };

function Node({ icon, label, state }: { icon: ReactNode; label: string; state: Hop }) {
  const tone = state === "ok" ? "border-kelp text-kelp" : state === "bad" ? "border-buoy text-buoy" : state === "wait" ? "border-lantern text-lantern" : "border-line text-ink3";
  return (
    <div className="flex w-[84px] shrink-0 flex-col items-center gap-1.5" role="img" aria-label={`${label}: ${WORD[state]}`}>
      <div className={cx("grid h-9 w-9 place-items-center rounded-full border-2 bg-panel transition-colors", tone)}>{icon}</div>
      <span className={cx("max-w-full truncate text-center text-[12px] leading-tight", state === "idle" ? "text-ink3" : "text-ink2")} title={label}>{label}</span>
    </div>
  );
}
function Seg({ state }: { state: Hop }) {
  return <div className="mb-5 h-[3px] min-w-4 flex-1 overflow-hidden rounded-full"><div className={cx("h-full w-full", state === "ok" ? "seg-live" : state === "bad" ? "seg-bad" : state === "wait" ? "seg-wait" : "seg-idle")} /></div>;
}

export function SignalPath({ site, tunnel, domain }: { site: Site; tunnel?: TunnelHealth; domain?: string }) {
  const h = hopStates(site, tunnel);
  return (
    <div className="flex items-start" role="group" aria-label="Connection from your PC to the internet">
      <Node icon={<Laptop size={17} />} label="This PC" state={h.pc} />
      <Seg state={h.tunnel} />
      <Node icon={<Waypoints size={17} />} label="Tunnel" state={h.tunnel} />
      <Seg state={h.edge} />
      <Node icon={<Cloud size={17} />} label="Cloudflare" state={h.edge} />
      <Seg state={h.domain} />
      <Node icon={<Globe size={17} />} label={domain ?? site.publishHostname ?? "Not published"} state={h.domain} />
    </div>
  );
}
