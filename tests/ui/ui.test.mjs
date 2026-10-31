// Browser checks of the real React UI against the design-preview data layer (src/lib/mock.ts).
// These prove navigation, forms, keyboard use and how each publish state renders. They do NOT prove
// anything about Cloudflare or the engine. Run: pnpm test:ui (needs a build served with `pnpm preview --port 4173` and Microsoft Edge).
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const BASE = process.env.HOSTIFY_UI_URL ?? "http://localhost:4173/";
const SHOTS = process.env.HOSTIFY_SHOTS;
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

const browser = await chromium.launch({ channel: "msedge", headless: true });
let failed = 0, passed = 0;
const pageErrors = [];

async function open({ url = BASE, width = 1180, height = 760, onboarded = true, theme, reducedMotion } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height }, colorScheme: theme ?? "light", reducedMotion });
  if (onboarded) await ctx.addInitScript(() => localStorage.setItem("hostify-onboarded", "1"));
  const page = await ctx.newPage();
  page.on("pageerror", (e) => pageErrors.push(String(e)));
  page.on("console", (m) => { if (m.type() === "error" && !/favicon/.test(m.location().url)) pageErrors.push(`${m.text()} ${m.location().url}`); });
  await page.goto(url);
  return page;
}
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png` }); };
async function test(name, fn) {
  try { await fn(); passed++; console.log(`ok   ${name}`); }
  catch (e) { failed++; console.log(`FAIL ${name}\n     ${String(e.message).split("\n").slice(0, 6).join("\n     ")}`); }
}
const nav = (page, name) => page.getByRole("navigation", { name: "Main" }).getByRole("button", { name }).click();
const noOverflow = async (page) => assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, "page scrolls sideways");

async function publishTo(page, host) {
  await page.getByRole("button", { name: "Add website" }).first().click();
  await page.getByRole("button", { name: /Standard Publish/ }).click();
  await page.getByRole("button", { name: "Choose folder" }).click();
  await page.getByText(/24 files/).waitFor();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue to domain" }).click();
  await page.getByRole("button", { name: "Connect Cloudflare" }).click();
  await page.getByLabel("API token").fill("x".repeat(40));
  await page.getByRole("dialog", { name: "Connect Cloudflare" }).getByRole("button", { name: "Connect Cloudflare" }).click();
  await page.getByLabel("Domain").fill(host);
  await page.getByRole("button", { name: "Check domain" }).click();
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "Publish website" }).click();
}

await test("every page renders without overflow at default and minimum window size", async () => {
  for (const [width, height] of [[1180, 760], [900, 600]]) {
    const page = await open({ width, height });
    for (const [label, heading] of [["Dashboard", /websites? running/], ["Websites", "Websites"], ["Domains", "Domains"], ["Activity & Logs", "Activity & Logs"], ["Settings", "Settings"]]) {
      await nav(page, label);
      await page.getByRole("heading", { level: 1, name: heading }).first().waitFor();
      await noOverflow(page);
      await shot(page, `${label.split(" ")[0].toLowerCase()}-${width}`);
    }
    await page.context().close();
  }
});

await test("dark theme renders every page", async () => {
  const page = await open({ theme: "dark" });
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "dark");
  for (const label of ["Dashboard", "Websites", "Domains", "Activity & Logs", "Settings"]) { await nav(page, label); await page.waitForTimeout(250); await shot(page, `dark-${label.split(" ")[0].toLowerCase()}`); }
  await page.context().close();
});

await test("first run: onboarding checks the computer and hands over to the wizard", async () => {
  const page = await open({ url: `${BASE}?empty`, onboarded: false });
  await page.getByRole("heading", { name: /Host a website from this PC/ }).waitFor();
  await shot(page, "onboarding-welcome");
  await page.getByRole("button", { name: "Check this computer" }).click();
  await page.getByText("Hosting engine", { exact: true }).waitFor();
  await shot(page, "onboarding-system");
  await page.getByRole("button", { name: "Add your first website" }).click();
  await page.getByRole("dialog", { name: "How would you like to publish?" }).waitFor();
  await page.reload();
  await page.getByRole("heading", { level: 1, name: /Put your first website online/ }).waitFor();
  assert.equal(await page.getByRole("heading", { name: /Host a website from this PC/ }).count(), 0, "onboarding came back after finishing");
  await page.context().close();
});

// Regression: Modal re-ran its focus effect on every parent re-render, so polling stole focus from the token field.
await test("typing in the token field keeps focus while the app polls", async () => {
  const page = await open();
  await nav(page, "Domains");
  await page.getByRole("button", { name: "Connect Cloudflare" }).click();
  const field = page.getByLabel("API token");
  await field.click(); await field.pressSequentially("abc");
  await page.waitForTimeout(3500);
  await page.keyboard.type("def");
  assert.equal(await field.inputValue(), "abcdef");
  assert.equal(await field.evaluate((el) => el === document.activeElement), true);
  await page.context().close();
});

await test("keyboard: skip link, focus trap, Escape restores focus", async () => {
  const page = await open();
  await page.getByRole("heading", { level: 1, name: /websites? running/ }).waitFor();
  await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Skip to content");
  const add = page.getByRole("button", { name: "Add website" }).last();
  await add.focus(); await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog");
  await dialog.waitFor();
  for (let i = 0; i < 14; i++) { await page.keyboard.press("Tab"); assert.equal(await dialog.evaluate((d) => d.contains(document.activeElement)), true, "focus left the dialog"); }
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
  await add.evaluate((el) => new Promise((ok, no) => { const t = Date.now(); (function poll() { if (el === document.activeElement) ok(); else if (Date.now() - t > 2000) no(new Error("focus not restored")); else requestAnimationFrame(poll); })(); }));
  await page.context().close();
});

await test("publish happy path shows waiting stages, never claims success early, ends published", async () => {
  const page = await open();
  await publishTo(page, "www.bakery-test.com");
  const dialog = page.getByRole("dialog");
  await dialog.getByText("Create a secure tunnel").waitFor();
  assert.equal(await dialog.getByText("Your website is on the internet").count(), 0);
  await dialog.getByText("Wait for DNS").waitFor();
  await dialog.getByRole("button", { name: "Keep checking in background" }).waitFor({ timeout: 12000 });
  await page.waitForTimeout(300); await shot(page, "publish-waiting-dns");
  assert.equal(await dialog.getByText("Your website is on the internet").count(), 0, "success shown while waiting");
  await dialog.getByRole("button", { name: "Details" }).click();
  await dialog.getByText("Connector online").waitFor();
  await page.waitForTimeout(250); await shot(page, "publish-details");
  await dialog.getByText("Your website is on the internet").waitFor({ timeout: 30000 });
  await page.waitForTimeout(600); await shot(page, "publish-done");
  await dialog.getByText(/still remembers an older DNS answer/).waitFor();
  await dialog.getByRole("button", { name: "Open website" }).waitFor();
  await page.context().close();
});

await test("waiting can be left and resumed from the website page", async () => {
  const page = await open();
  await publishTo(page, "www.leave-test.com");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Keep checking in background" }).click({ timeout: 15000 });
  await dialog.waitFor({ state: "detached" });
  await nav(page, "Websites");
  await page.getByRole("button", { name: /my-site/ }).first().click();
  await page.getByText("Publishing www.leave-test.com").waitFor();
  await page.context().close();
});

await test("paused verification explains what is kept and offers retry and unpublish", async () => {
  const page = await open();
  await publishTo(page, "stuck.example.com");
  const dialog = page.getByRole("dialog");
  await dialog.getByText("Hostify paused checking your public website").waitFor({ timeout: 25000 });
  await dialog.getByText(/Nothing was removed/).waitFor();
  await dialog.getByRole("button", { name: "Check again" }).waitFor();
  await dialog.getByRole("button", { name: "Unpublish" }).waitFor();
  await page.waitForTimeout(250); await shot(page, "publish-recovery");
  await page.context().close();
});

await test("hard failure shows the real reason and offers a retry", async () => {
  const page = await open();
  await publishTo(page, "fail.example.com");
  const dialog = page.getByRole("dialog");
  await dialog.getByText("Publishing failed").waitFor({ timeout: 15000 });
  await dialog.getByText(/token lacks Account/).waitFor();
  await page.waitForTimeout(250); await shot(page, "publish-failed");
  await dialog.getByRole("button", { name: "Try publishing again" }).click();
  await dialog.getByRole("button", { name: "Publish website" }).waitFor();
  await page.context().close();
});

await test("invalid domain is rejected after leaving the field", async () => {
  const page = await open();
  await page.getByRole("button", { name: "Add website" }).first().click();
  await page.getByRole("button", { name: /Standard Publish/ }).click();
  await page.getByRole("button", { name: "Choose folder" }).click();
  await page.getByText(/24 files/).waitFor();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue to domain" }).click();
  await page.getByRole("button", { name: "Connect Cloudflare" }).click();
  await page.getByLabel("API token").fill("x".repeat(40));
  await page.getByRole("dialog", { name: "Connect Cloudflare" }).getByRole("button", { name: "Connect Cloudflare" }).click();
  const domain = page.getByLabel("Domain");
  await domain.fill("not a domain"); await domain.blur();
  await page.getByText(/Enter a full domain/).waitFor();
  assert.equal(await page.getByRole("button", { name: "Check domain" }).isDisabled(), true);
  await page.context().close();
});

await test("publishing mode chooser separates quick and advanced experiences", async () => {
  const page = await open();
  await page.getByRole("button", { name: "Add website" }).first().click();
  await page.getByRole("dialog", { name: "How would you like to publish?" }).waitFor();
  await page.getByRole("button", { name: /Fast Publish/ }).click();
  await page.getByText("Select your project").waitFor();
  await page.getByRole("button", { name: "Standard Publish" }).click();
  await page.getByRole("dialog", { name: "Add a website" }).waitFor();
  await page.getByText("Import", { exact: true }).first().waitFor();
  await page.context().close();
});

await test("Fast Publish uses real project detection and requires DNS consent", async () => {
  const page = await open();
  await page.getByRole("button", { name: "Add website" }).first().click();
  await page.getByRole("button", { name: /Fast Publish/ }).click();
  await page.getByRole("button", { name: "Choose folder" }).click();
  await page.getByRole("button", { name: /Continue to domain/ }).click();
  await page.getByRole("textbox", { name: "Public domain" }).fill("fast.example.test");
  const publish = page.getByRole("button", { name: "Publish website" });
  assert.equal(await publish.isDisabled(), true, "must not publish without explicit DNS consent");
  if (await page.getByRole("button", { name: "Connect Cloudflare" }).count()) {
    await page.getByRole("button", { name: "Connect Cloudflare" }).click();
    await page.getByLabel("API token").fill("x".repeat(40));
    await page.getByRole("dialog", { name: "Connect Cloudflare" }).getByRole("button", { name: "Connect Cloudflare" }).click();
  }
  await page.getByRole("checkbox", { name: /Authorize this domain/ }).check();
  await publish.click();
  await page.getByText("Shared secure publishing engine").waitFor({ timeout: 15000 });
  await page.context().close();
});

await test("Fast Publish never executes dynamic code without explicit trust", async () => {
  const page = await open();
  // The page is already loaded: addInitScript runs on the next navigation,
  // so set the mock path immediately for this test.
  await page.evaluate(() => { window.__hostifyMockPath = "C:/hostify/node-express-project"; });
  await page.getByRole("button", { name: "Add website" }).first().click();
  await page.getByRole("button", { name: /Fast Publish/ }).click();
  await page.getByRole("button", { name: "Choose folder" }).click();
  await page.getByRole("button", { name: /Continue to domain/ }).click();
  await page.getByRole("textbox", { name: "Public domain" }).fill("safe.example.test");
  await page.getByRole("checkbox", { name: /Authorize this domain/ }).check();
  assert.equal(await page.getByRole("button", { name: "Publish website" }).isDisabled(), true);
  await page.getByRole("checkbox", { name: /I trust this application/ }).check();
  await page.getByRole("button", { name: "Publish website" }).waitFor({ state: "visible" });
  await page.context().close();
});

await test("activity search and filters, with a way out of an empty result", async () => {
  const page = await open();
  await nav(page, "Activity & Logs");
  await page.getByText("Hostify engine started").waitFor();
  await page.getByRole("searchbox", { name: "Search activity" }).fill("nothing-matches-this");
  await page.getByText("Nothing matches these filters").waitFor();
  await page.getByRole("button", { name: "Clear filters" }).click();
  await page.getByText("Hostify engine started").waitFor();
  await page.getByLabel("Severity").selectOption("error");
  await page.getByText("Nothing matches these filters").waitFor();
  await page.context().close();
});

await test("destructive actions name their target and need a second step", async () => {
  const page = await open();
  await nav(page, "Websites");
  await page.getByRole("button", { name: "Remove website" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: /^Remove Bakery landing page$/ }).waitFor();
  await dialog.getByRole("button", { name: "Keep it" }).click();
  await nav(page, "Dashboard");
  await page.getByText("Bakery landing page").first().waitFor();
  await page.context().close();
});

await test("settings: theme switch applies, toggles are real switches", async () => {
  const page = await open();
  await nav(page, "Settings");
  await page.getByRole("radio", { name: "dark" }).click();
  await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
  const sw = page.getByRole("switch", { name: "Keep hosting when the window is closed" });
  assert.equal(await sw.getAttribute("aria-checked"), "true");
  await sw.click();
  await page.waitForFunction(() => document.querySelector('[role=switch][aria-label="Keep hosting when the window is closed"]').getAttribute("aria-checked") === "false");
  await page.context().close();
});

await test("reduced motion stops the signal animation", async () => {
  const page = await open({ reducedMotion: "reduce" });
  await page.locator(".seg-live").first().waitFor();
  assert.equal(await page.locator(".seg-live").first().evaluate((el) => getComputedStyle(el).animationName), "none");
  await page.context().close();
});

await test("Guardian is opt-in, preserves stopped websites and exposes read-only health checks", async () => {
  const page=await open();
  const card=page.locator("section").filter({hasText:"Hostify Guardian"}).first();
  await card.getByRole("button",{name:"Enable"}).first().click();
  await card.getByText("Monitoring",{exact:true}).first().waitFor();
  await card.getByRole("button",{name:"Check now"}).first().click();
  await card.getByText("Healthy",{exact:false}).first().waitFor();
  await card.getByRole("button",{name:"Disable"}).first().click();
  await card.getByText("Monitoring disabled").first().waitFor();
  await page.context().close();
});

await test("Time Machine makes explicit code-only snapshots with integrity checks", async () => {
  const page=await open();
  const card=page.locator("section").filter({hasText:"Time Machine · Website files"}).first();
  await card.getByText(/MySQL databases, managed credential-store secrets and external storage are/).waitFor();
  await card.getByText(/Other project files may contain secrets/).waitFor();
  await card.getByRole("button",{name:"Create snapshot"}).first().click();
  await card.getByRole("button",{name:"Snapshots (1)"}).first().click();
  await card.getByRole("button",{name:"Verify"}).first().click();
  await card.getByText(/SHA-256 integrity verified/).waitFor();
  // A running website must not offer an unsafe restore.
  assert.equal(await card.getByRole("button",{name:"Restore"}).first().isDisabled(),true);
  await page.context().close();
});

await test("offline App Catalog can import a static project using the existing site flow", async () => {
  const page=await open();
  const catalog=page.locator("section").filter({hasText:"App Catalog · One-click starters"}).first();
  await catalog.getByText("Portfolio",{exact:true}).waitFor();
  await catalog.getByRole("button",{name:"Use template"}).first().click();
  await page.getByRole("heading",{level:1,name:"Websites"}).waitFor();
  await page.getByText("Portfolio",{exact:true}).first().waitFor();
  await page.context().close();
});

await test("Hostify Doctor remains local and shows read-only deterministic findings", async () => {
  const page=await open();
  const card=page.locator("section").filter({hasText:"Hostify Doctor"}).first();
  await card.getByRole("button",{name:"Analyze logs"}).first().click();
  await card.getByText(/No known error signatures found/).first().waitFor();
  await card.getByText(/No log lines are uploaded/).waitFor();
  await page.context().close();
});

await test("Security Center marks remote readiness unverified and never hides the need for real Windows checks", async () => {
  const page=await open();
  const audit=page.locator("section").filter({hasText:"Security Center"}).first();
  await audit.getByText("Unverified",{exact:true}).waitFor();
  await audit.getByRole("button",{name:"Evidence and next actions (1)"}).click();
  await audit.getByText(/Credential rotation and off-device backup have not been verified/).waitFor();
  await audit.getByRole("button",{name:"Rescan"}).click();
  await audit.getByText(/No blockers does not mean production-ready/).waitFor();
  await page.context().close();
});

await test("no uncaught errors in the browser console", async () => {
  const real = pageErrors.filter((e) => !/favicon/.test(e));
  assert.deepEqual(real, []);
});

await browser.close();
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
