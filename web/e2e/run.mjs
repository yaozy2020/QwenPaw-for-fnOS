import { spawn } from "node:child_process";
import { mkdtemp, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(web, "..");
const args = process.argv.slice(2);
if (args.includes("--help")) {
  console.log(`Local Playwright management E2E (not fnOS identity/session acceptance).
Run from repository root with Node on PATH:
  npm run test:e2e --prefix web                # Gateway UDS + Chromium, port 4176
  npm run test:e2e:gateway --prefix web        # same Gateway flow
  npm run test:e2e:gateway:webkit --prefix web # Gateway UDS + WebKit, local HTTPS
  node web/e2e/run.mjs --engine=webkit
  QWENPAW_BROWSER_ENGINE=webkit npm run test:e2e --prefix web
Engine: chromium (default) or webkit; Chromium can use PLAYWRIGHT_CHROMIUM_EXECUTABLE.
Other overrides: QWENPAW_BROWSER_PORT, QWENPAW_TEST_SUP_BIN,
  QWENPAW_TEST_RUNTIME_BIN.
Builds the console using the existing build.mjs in a temporary working directory;
web/dist and app/www are never changed. Builds current release binaries with Cargo.
Gateway uses a copied Python adapter running as UID 0 in an unprivileged user
namespace (unshare required for non-root); the adapter executable identity is
checked for fixture cleanup, not gateway authorization. Test-only X-Trim identity,
real Rust Origin/fetch-site and signed double-submit CSRF. No management TCP proxy.
Playwright arguments after -- are forwarded. Results and fixture roots are temporary.`);
  process.exit(0);
}

if (args.some(arg => arg.startsWith("--protocol="))) {
  throw new Error("Only the Gateway E2E protocol is supported");
}
const engineArg = args.find(arg => arg.startsWith("--engine="));
if (engineArg) args.splice(args.indexOf(engineArg), 1);
const engine = engineArg?.slice("--engine=".length) || process.env.QWENPAW_BROWSER_ENGINE || "chromium";
if (!["chromium", "webkit"].includes(engine)) throw new Error(`Invalid browser engine: ${engine}`);
const temporary = await mkdtemp(join(tmpdir(), "qwenpaw-e2e-gateway-"));
const env = { ...process.env, QWENPAW_BROWSER_ENGINE: engine };
for (const key of Object.keys(env)) {
  if (key.startsWith("QWENPAW_GATEWAY_") || key.startsWith("QWENPAW_TEST_GATEWAY_")
    || key === "QWENPAW_ENABLE_CRASH_POINT" || key === "QWENPAW_CRASH_POINT") delete env[key];
}

function run(command, parameters, cwd = web) {
  return new Promise((resolveCode, reject) => {
    const child = spawn(command, parameters, { cwd, env, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (signal) reject(new Error(`${command} exited by ${signal}`));
      else resolveCode(code ?? 1);
    });
  });
}

try {
  const { copyFile, readFile } = await import("node:fs/promises");
  await copyFile(join(web, "package.json"), join(temporary, "package.json"));
  await symlink(join(web, "node_modules"), join(temporary, "node_modules"));
  await symlink(join(web, "src"), join(temporary, "src"));
  await symlink(join(web, "build.mjs"), join(temporary, "build.mjs"));
  let code = await run(process.execPath, [join(temporary, "build.mjs")], temporary);
  if (code === 0) code = await run("cargo", ["build", "--locked", "--release", "--manifest-path", join(root, "supervisor/Cargo.toml")]);
  if (code === 0) {
    env.QWENPAW_TEST_WEB_HTML = join(temporary, "dist/index.html");
    env.QWENPAW_BROWSER_RESULTS = join(temporary, "results");
    env.QWENPAW_BROWSER_CLEANUP_REPORT = join(temporary, "gateway-cleanup.json");
    code = await run(process.execPath, [join(web, "node_modules/@playwright/test/cli.js"), "test", ...args]);
    let cleanup;
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        cleanup = JSON.parse(await readFile(env.QWENPAW_BROWSER_CLEANUP_REPORT, "utf8"));
        if (cleanup.finalized) break;
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    if (!cleanup?.ok || !cleanup.rootRemoved || !cleanup.finalized || !cleanup.peerRootRemoved) {
      console.error("Gateway cleanup NOT verified:", cleanup || "missing cleanup report");
      code ||= 1;
    } else {
      console.log("Gateway cleanup verified:", JSON.stringify(cleanup));
    }
  }
  process.exitCode = code;
} finally {
  await rm(temporary, { recursive: true, force: true });
}
