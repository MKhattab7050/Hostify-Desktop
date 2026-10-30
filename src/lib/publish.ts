import type { Deployment, DeploymentState, Site, TunnelHealth, VerifyClass } from "./types";
import type { Tone } from "../components/ui";

export const VERIFYING: DeploymentState[] = ["waiting_dns", "waiting_tls", "verifying_public"];
export const PROVISIONING: DeploymentState[] = ["validating", "starting_local", "provisioning_tunnel", "configuring_dns", "starting_connector"];

export const isVerifying = (s?: DeploymentState) => !!s && VERIFYING.includes(s);
export const isProvisioning = (s?: DeploymentState) => !!s && PROVISIONING.includes(s);
/** The engine is still doing work and will send more updates. */
export const isActive = (s?: DeploymentState) => isVerifying(s) || isProvisioning(s);
/** Cloudflare resources exist and belong to this website, whatever the public check says. */
export const holdsResources = (s?: DeploymentState) => isVerifying(s) || s === "published" || s === "recovery_required";

export function publishPill(state?: DeploymentState): { text: string; tone: Tone; busy?: boolean } {
  switch (state) {
    case "published": return { text: "Published", tone: "ok" };
    case "waiting_dns": return { text: "Waiting for DNS", tone: "wait", busy: true };
    case "waiting_tls": return { text: "Waiting for HTTPS", tone: "wait", busy: true };
    case "verifying_public": return { text: "Checking public site", tone: "wait", busy: true };
    case "recovery_required": return { text: "Needs your attention", tone: "bad" };
    case "failed": return { text: "Publishing failed", tone: "bad" };
    case undefined: case "rolled_back": return { text: "Local only", tone: "idle" };
    default: return { text: "Publishing", tone: "wait", busy: true };
  }
}

export interface Stage { key: string; label: string; explain: string; state: "pending" | "running" | "done" | "failed" | "waiting" }

const SETUP: { key: string; steps: string[]; label: string; explain: string }[] = [
  { key: "site", steps: ["validate_website", "start_local_server", "verify_local_site"], label: "Prepare your website", explain: "Hostify checks your files and starts the local web server." },
  { key: "tunnel", steps: ["create_tunnel"], label: "Create a secure tunnel", explain: "A private outbound link to Cloudflare, so no ports open on your router." },
  { key: "dns", steps: ["add_dns_record"], label: "Point your domain at the tunnel", explain: "Hostify adds the one DNS record you approved." },
  { key: "connector", steps: ["start_connector"], label: "Connect this PC", explain: "The connector on this computer joins the tunnel." },
];

/** Turns engine steps and the deployment state into the stages a person reads. */
export function stagesOf(d: Deployment): Stage[] {
  const out: Stage[] = SETUP.map((g) => {
    const steps = d.steps.filter((s) => g.steps.includes(s.key));
    const state: Stage["state"] = steps.some((s) => s.state === "failed") ? "failed"
      : steps.length > 0 && steps.every((s) => s.state === "done" || s.state === "skipped") ? "done"
      : steps.some((s) => s.state === "running" || s.state === "done") ? "running" : "pending";
    return { key: g.key, label: g.label, explain: g.explain, state };
  });
  const v = d.state; const setupDone = out.every((s) => s.state === "done");
  const published = v === "published";
  const stuck = v === "recovery_required";
  const stale = v === "failed" || v === "rolled_back";
  const pos = v === "waiting_dns" ? 0 : v === "waiting_tls" ? 1 : v === "verifying_public" ? 2 : -1;
  const cls = d.verification?.errorClass;
  const stuckAt = cls === "tls_pending" ? 1 : cls === "zone_not_active" || cls === "dns_record_missing" || cls === "dns_not_visible" ? 0 : 2;
  const tail = [
    { key: "wait_dns", label: "Wait for DNS", explain: "DNS answers can stay cached around the world for a while. This is normal and needs nothing from you." },
    { key: "wait_tls", label: "Wait for the HTTPS certificate", explain: "Cloudflare issues the certificate for your domain automatically." },
    { key: "verify", label: "Check your public website", explain: "Hostify requests a private one-time marker over HTTPS to prove this deployment answers at your domain." },
  ];
  tail.forEach((t, i) => {
    let state: Stage["state"] = "pending";
    if (published) state = "done";
    else if (pos >= 0) state = i < pos ? "done" : i === pos ? "waiting" : "pending";
    else if (stuck) state = i < stuckAt ? "done" : i === stuckAt ? "failed" : "pending";
    else if (stale && setupDone) state = i === 0 ? "failed" : "pending";
    out.push({ ...t, state });
  });
  return out;
}

const NEXT: Record<VerifyClass, string> = {
  zone_not_active: "Change the nameservers at your domain registrar to the ones below. Registrars can take up to a day to apply this.",
  dns_record_missing: "The DNS record Hostify added was removed or changed in Cloudflare. Hostify won't overwrite it. Restore it, or unpublish and publish again.",
  dns_not_visible: "Nothing to do. Hostify keeps checking.",
  tls_pending: "Nothing to do. If this lasts more than an hour, check that SSL/TLS is enabled for this domain in Cloudflare.",
  connector_down: "Check that this PC is online and that a firewall isn't blocking cloudflared.",
  origin_unreachable: "Check that the website is running on the Websites page. Hostify keeps checking.",
  routing_mismatch: "Your domain answered, but not with this website. Look in Cloudflare for a redirect, Worker or page rule on this domain.",
  network_error: "Check this PC's internet connection. Hostify keeps checking.",
  cloudflare_api: "Hostify couldn't reach the Cloudflare API. Check the connection and that your token is still valid.",
};
// Automatic checks have stopped by the time these show, so none of them may promise more checking.
const PAUSED: Partial<Record<VerifyClass, string>> = {
  dns_not_visible: "DNS usually appears well within 30 minutes. Check that the record exists in your Cloudflare DNS settings, then check again.",
  tls_pending: "Check that SSL/TLS is enabled for this domain in Cloudflare, then check again.",
  origin_unreachable: "Check that the website is running on the Websites page, then check again.",
  network_error: "Check this PC's internet connection, then check again.",
};
export const nextAction = (c?: VerifyClass, paused = false) => (c ? (paused && PAUSED[c]) || NEXT[c] : undefined);

export function diagnosticsText(d: Deployment, tunnel?: TunnelHealth): string {
  const v = d.verification;
  return [
    `Deployment ${d.id}`, `Website ${d.siteId}`, `Hostname ${d.hostname ?? "unknown"}`, `State ${d.state}`,
    `Started ${d.startedAt}`, d.finishedAt && `Finished ${d.finishedAt}`, d.error && `Error ${d.error}`,
    ...d.steps.map((s) => `Step ${s.key}: ${s.state}${s.detail ? ` (${s.detail})` : ""}`),
    v && `Attempt ${v.attempt}, class ${v.errorClass ?? "none"}, last checked ${v.lastCheckedAt ?? "never"}, next ${v.nextRetryAt ?? "none"}`,
    v?.message && `Message ${v.message}`, v?.visibleFromThisPc !== undefined && `Visible from this PC: ${v.visibleFromThisPc}`,
    ...(v?.checks ?? []).map((c) => `Check ${c.key}: ${c.state}${c.detail ? ` (${c.detail})` : ""}`),
    tunnel && `Tunnel connected ${tunnel.connected}, connections ${tunnel.connections}`,
  ].filter(Boolean).join("\n");
}

export const siteById = (sites: Site[], id?: string) => sites.find((s) => s.id === id);

/** Never lets anything shaped like a credential reach the clipboard. */
export function redact(text: string): string {
  return text
    .replace(/eyJ[\w-]{10,}(\.[\w-]+)*/g, "[redacted]")
    .replace(/(token|authorization|bearer|secret|password|api[_-]?key)(["'\s:=]+)[^\s"',;]+/gi, "$1$2[redacted]")
    .replace(/\b[A-Za-z0-9_-]{40,}\b/g, "[redacted]");
}
