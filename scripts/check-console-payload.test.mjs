import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { brotliCompressSync, gzipSync } from "node:zlib";
import { checkConsolePayload } from "./check-console-payload.mjs";

const hash = bytes => createHash("sha256").update(bytes).digest("hex");

function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), "qwenpaw-payload-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const payload = path.join(directory, "payload");
  const manifest = { schema_version: 1, console_complete: true, files: [] };
  const vite = {
    "index.html": { file: "assets/main-12345678.js", isEntry: true, imports: ["_shared"], dynamicImports: ["src/lazy.ts"], css: ["assets/main.css"], assets: ["logo.svg"] },
    _shared: { file: "assets/shared.js", imports: ["index.html"] },
    "src/lazy.ts": { file: "assets/lazy.js", css: ["assets/lazy.css"], assets: ["assets/lazy.png"] },
  };
  function put(relative, value) {
    const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
    const target = path.join(payload, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, bytes);
    let entry = manifest.files.find(entry => entry.path === relative);
    if (!entry) { entry = { path: relative }; manifest.files.push(entry); }
    entry.sha256 = hash(bytes);
  }
  function save() {
    writeFileSync(path.join(directory, "manifest.json"), JSON.stringify(manifest));
  }
  function saveVite() { put("console/.vite/manifest.json", JSON.stringify(vite)); save(); }
  function check(options = {}) { save(); return checkConsolePayload({ patchRoot: directory, ...options }); }
  put("app/console_static.py", "# payload source\n");
  put("console/index.html", '<script type="module" src="./assets/main-12345678.js"></script><link rel="stylesheet" href="./assets/main.css"><img src="logo.svg">');
  put("console/assets/main-12345678.js", 'import "./shared.js"; import("./lazy.js"); new Worker(new URL("./editor.worker.js", import.meta.url)); new Worker(""+__qpBase+"/assets/json.worker.js");');
  put("console/assets/shared.js", "export const shared = 1;");
  put("console/assets/lazy.js", "export default 'lazy';");
  put("console/assets/editor.worker.js", "self.onmessage = () => {};");
  put("console/assets/json.worker.js", "self.onmessage = () => {};");
  put("console/assets/main.css", 'body { background: url("./background.png"); }');
  put("console/assets/background.png", Buffer.from([137, 80, 78, 71]));
  put("console/assets/lazy.css", "body { color: black; }");
  put("console/assets/lazy.png", Buffer.from([137, 80, 78, 71]));
  put("console/logo.svg", "<svg/>");
  const large = Buffer.from("/* compression fixture */\n".repeat(80));
  put("console/assets/large.js", large);
  put("console/assets/large.js.gz", gzipSync(large));
  put("console/assets/large.js.br", brotliCompressSync(large));
  saveVite();
  return { directory, payload, manifest, vite, put, saveVite, check };
}

function fails(report, pattern) {
  assert.equal(report.ok, false, JSON.stringify(report));
  assert.match(report.errors.join("\n"), pattern);
}

test("valid payload checks hashes, compression, cyclic and dynamic Vite edges and workers", t => {
  const f = fixture(t);
  const report = f.check({ expectedFiles: f.manifest.files.length });
  assert.deepEqual(report.errors, []);
  assert.equal(report.ok, true);
  assert.equal(report.compressedPairs, 1);
  assert.ok(report.checkedReferences >= 15);
});

test("SHA256 tampering fails even when the resource remains valid JavaScript", t => {
  const f = fixture(t);
  writeFileSync(path.join(f.payload, "console/assets/shared.js"), "export const shared = 2;");
  fails(f.check(), /SHA256 mismatch: console\/assets\/shared.js/);
});

test("declared missing files and undeclared extras fail", t => {
  const f = fixture(t);
  rmSync(path.join(f.payload, "console/assets/shared.js"));
  writeFileSync(path.join(f.payload, "extra.txt"), "extra");
  const report = f.check();
  fails(report, /missing payload file: console\/assets\/shared.js/);
  fails(report, /extra payload file: extra.txt/);
});

test("unsafe and duplicate manifest paths fail", t => {
  const f = fixture(t);
  for (const relative of ["../escape", "/absolute", "C:/drive", "console\\escape", "console//bad", "console/./bad", "console/%2e%2e/escape", "console/file?query", "console/file\u0000"]) {
    f.manifest.files.push({ path: relative, sha256: "a".repeat(64) });
  }
  f.manifest.files.push({ ...f.manifest.files[0] });
  const report = f.check();
  fails(report, /unsafe payload path/);
  fails(report, /duplicate payload path/);
});

test("expected manifest count is an optional explicit gate", t => {
  const f = fixture(t);
  fails(f.check({ expectedFiles: 916 }), /manifest file count: expected 916/);
});

test("missing compression fails after removing it from both disk and manifest", t => {
  const f = fixture(t);
  for (const suffix of [".gz", ".br"]) {
    rmSync(path.join(f.payload, "console/assets/large.js" + suffix));
    f.manifest.files = f.manifest.files.filter(entry => entry.path !== "console/assets/large.js" + suffix);
  }
  fails(f.check(), /missing compressed file: console\/assets\/large.js\.(?:gz|br)/);
});

test("gzip and Brotli must decode to original bytes even with valid manifest hashes", t => {
  const f = fixture(t);
  f.put("console/assets/large.js.gz", gzipSync(Buffer.from("wrong")));
  f.put("console/assets/large.js.br", brotliCompressSync(Buffer.from("wrong")));
  const report = f.check();
  fails(report, /compressed content mismatch: console\/assets\/large.js.gz/);
  fails(report, /compressed content mismatch: console\/assets\/large.js.br/);
});

test("invalid compressed bytes and orphan sidecars fail", t => {
  const f = fixture(t);
  f.put("console/assets/large.js.br", "not brotli");
  f.put("console/assets/orphan.js.gz", gzipSync(Buffer.from("orphan")));
  const report = f.check();
  fails(report, /invalid compressed content/);
  fails(report, /orphan compressed file/);
});

test("HTML direct references and module entry must exist", t => {
  const f = fixture(t);
  f.put("console/index.html", '<script type="module" src="./assets/missing.js"></script><img src="missing.svg">');
  const report = f.check();
  fails(report, /missing resource reference: console\/index.html -> console\/assets\/missing.js/);
  fails(report, /missing resource reference: console\/index.html -> console\/missing.svg/);
  fails(report, /HTML module is not a Vite entry/);
});

test("Vite imports and dynamicImports resolve source keys, including absent dynamic targets", t => {
  const f = fixture(t);
  f.vite["index.html"].imports.push("_absent");
  f.vite["index.html"].dynamicImports.push("src/missing.ts");
  f.saveVite();
  const report = f.check();
  fails(report, /missing Vite imports key: index.html -> _absent/);
  fails(report, /missing Vite dynamicImports key: index.html -> src\/missing.ts/);
});

test("files referenced only from dynamicImports are checked for existence", t => {
  const f = fixture(t);
  f.vite["src/lazy.ts"].file = "assets/missing-lazy.js";
  f.saveVite();
  fails(f.check(), /missing resource reference: .* -> console\/assets\/missing-lazy.js/);
});

test("Vite css, assets and worker fields check resource existence", t => {
  const f = fixture(t);
  f.vite["src/lazy.ts"].css = ["assets/missing.css"];
  f.vite["src/lazy.ts"].assets = ["assets/missing.png"];
  f.vite["src/lazy.ts"].workers = ["assets/missing.worker.js"];
  f.saveVite();
  const report = f.check();
  for (const suffix of ["missing.css", "missing.png", "missing.worker.js"]) {
    fails(report, new RegExp(suffix.replaceAll(".", "\\.")));
  }
});

test("literal JavaScript dynamic imports, worker URLs and CSS URLs check existence", t => {
  const f = fixture(t);
  f.put("console/assets/main-12345678.js", 'import("./missing-dynamic.js"); new Worker(new URL("./missing.worker.js", import.meta.url)); new Worker(""+__qpBase+"/assets/missing-base.worker.js");');
  f.put("console/assets/main.css", 'body { background: url("missing.png"); }');
  const report = f.check();
  for (const suffix of ["missing-dynamic.js", "missing.worker.js", "missing-base.worker.js", "missing.png"]) {
    fails(report, new RegExp(suffix.replaceAll(".", "\\.")));
  }
});

test("fonts referenced by CSS and Vite assets must exist", t => {
  const f = fixture(t);
  f.put("console/assets/font.woff2", Buffer.from([119, 79, 70, 50]));
  f.put("console/assets/main.css", '@font-face { src: url("./font.woff2"); }');
  f.vite["index.html"].assets.push("assets/font.woff2");
  f.saveVite();
  assert.equal(f.check().ok, true);
  rmSync(path.join(f.payload, "console/assets/font.woff2"));
  fails(f.check(), /missing resource reference: .* -> console\/assets\/font.woff2/);
});

test("HTML comments and inline script strings are not parsed as real tags", t => {
  const f = fixture(t);
  f.put("console/index.html", '<!-- <img src="missing-comment.svg"> --><script>const ignored = \'<img src="missing-script.svg">\';</script><script type="module" src="./assets/main-12345678.js"></script>');
  assert.equal(f.check().ok, true);
});

test("JS diagnostic strings, comments, template text and regex bodies are not imports", t => {
  const f = fixture(t);
  f.put("console/assets/main-12345678.js", [
    'const diagnostic = "Invalid import \\\"./missing.js\\\". Did you mean";',
    "// import './missing-comment.js'; new Worker('./comment.worker.js');",
    "/* import('./missing-block.js') */",
    "const text = `import('./missing-template.js')`;",
    "const pattern = /import\\(\"missing-regex.js\"\\)/;",
    'import value from "./shared.js"; export { value } from "./shared.js";',
  ].join("\n"));
  const report = f.check();
  assert.equal(report.ok, true, report.errors.join("\n"));
  assert.deepEqual(report.dynamicReferences, []);
});

test("dynamic expressions and unproved files are reported and never deleted", t => {
  const f = fixture(t);
  f.put("console/assets/main-12345678.js", "import(window.moduleName); new Worker(window.workerUrl);");
  f.put("console/assets/runtime-only.js", "export const retained = true;");
  const target = path.join(f.payload, "console/assets/runtime-only.js");
  const before = readFileSync(target);
  const report = f.check();
  assert.equal(report.ok, true, report.errors.join("\n"));
  assert.equal(report.dynamicReferences.length, 2);
  assert.ok(report.unprovenFiles.includes("console/assets/runtime-only.js"));
  assert.deepEqual(readFileSync(target), before);
});

test("external URLs and data resources do not require local files", t => {
  const f = fixture(t);
  f.put("console/assets/main.css", 'body { background: url("data:image/svg+xml;base64,AA=="); }');
  f.put("console/index.html", '<script type="module" src="./assets/main-12345678.js?v=1&amp;mode=2"></script><img src="https://example.test/icon.png">');
  const report = f.check();
  assert.equal(report.ok, true, report.errors.join("\n"));
});

test("unchanged upstream entries are counted and retained without inferring deletion", t => {
  const f = fixture(t);
  const entry = f.manifest.files.find(entry => entry.path === "console/assets/background.png");
  entry.original_sha256 = entry.sha256;
  const before = readFileSync(path.join(f.payload, entry.path));
  const report = f.check();
  assert.equal(report.ok, true, report.errors.join("\n"));
  assert.equal(report.inheritedFiles, 1);
  assert.deepEqual(readFileSync(path.join(f.payload, entry.path)), before);
});

test("delete_files validates hashes but does not require obsolete source files", t => {
  const f = fixture(t);
  f.manifest.delete_files = [{ path: "console/assets/obsolete.js", original_sha256: "1".repeat(64), previous_sha256: ["2".repeat(64)] }];
  const report = f.check();
  assert.equal(report.ok, true, report.errors.join("\n"));
  assert.equal(report.deleteFiles, 1);
});

test("delete_files rejects traversal, duplicates, overlap and invalid old digests", t => {
  const f = fixture(t);
  const old = { path: "console/assets/obsolete.js", original_sha256: "a".repeat(64) };
  f.manifest.delete_files = [old, old, { ...old, path: "../escape" }, { ...old, path: "console/index.html" },
    { path: "console/assets/bad.js", original_sha256: "bad", previous_sha256: ["not-a-hash"] }];
  const report = f.check();
  for (const pattern of [/unsafe delete_files path/, /duplicate delete_files path/, /delete_files overlaps files/, /invalid delete_files original_sha256/, /invalid delete_files previous_sha256/]) fails(report, pattern);
});

test("delta manifest requires an explicit official Console baseline", t => {
  const f = fixture(t);
  delete f.manifest.console_complete;
  const report = f.check();
  fails(report, /delta manifest requires --base-console/);
  assert.equal(report.errors.some(message => message.startsWith("missing resource reference")), false);
});

test("delta uses base plus payload minus delete_files, while complete cannot borrow base files", t => {
  const f = fixture(t);
  const base = path.join(f.directory, "official-console");
  mkdirSync(base);
  writeFileSync(path.join(base, "baseline.svg"), "<svg/>");
  writeFileSync(path.join(base, "obsolete.js"), "// old\n");
  f.put("console/index.html", '<script type="module" src="./assets/main-12345678.js"></script><img src="baseline.svg">');
  f.manifest.console_complete = false;
  f.manifest.delete_files = [{ path: "console/obsolete.js", original_sha256: hash("// old\n") }];
  let report = f.check({ baseConsole: base });
  assert.equal(report.ok, true, report.errors.join("\n"));
  assert.equal(report.unprovenFiles.includes("console/obsolete.js"), false);
  f.manifest.console_complete = true;
  fails(f.check({ baseConsole: base }), /missing resource reference: console\/index.html -> console\/baseline.svg/);
});
