#!/usr/bin/env node
// Read-only payload gate. No dependency installation and no resource deletion.
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { brotliDecompressSync, gunzipSync } from "node:zlib";

const DEFAULT_PATCH = fileURLToPath(new URL("../patches/qwenpaw-2.3.0-base-path/", import.meta.url));
const SHA256 = /^[a-f0-9]{64}$/;
const COMPRESSIBLE = /\.(?:html|css|js|mjs|json|svg|txt|xml|map)$/i;
const PRECOMPRESS_MIN_BYTES = 1024;

function safePath(value) {
  return typeof value === "string" && value.length > 0 && !/[\\:%?#\x00-\x1f\x7f]/.test(value)
    && !value.startsWith("/") && value.split("/").every(part => part && part !== "." && part !== "..");
}

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function validHash(value) {
  return typeof value === "string" && SHA256.test(value);
}

// Locate candidate syntax in code, skipping quoted diagnostics, comments and
// regex bodies. This is a small lexer, not a complete JavaScript parser; the
// Vite manifest remains authoritative for the chunk graph.
function codeOffsets(source, candidates) {
  const offsets = [...new Set(candidates)].sort((a, b) => a - b);
  const code = new Set();
  let next = 0;
  let cursor = 0;
  let regexAllowed = true;
  const skipQuoted = (start, quote) => {
    let end = start + 1;
    while (end < source.length) {
      if (source[end] === "\\") { end += 2; continue; }
      if (source[end++] === quote) break;
    }
    return end;
  };
  while (cursor < source.length && next < offsets.length) {
    const start = cursor;
    const char = source[cursor];
    let isCode = true;
    if (/\s/.test(char)) { cursor++; }
    else if (char === '"' || char === "'" || char === "`") {
      cursor = skipQuoted(cursor, char);
      isCode = false;
      regexAllowed = false;
    } else if (source.startsWith("//", cursor)) {
      const end = source.indexOf("\n", cursor + 2);
      cursor = end < 0 ? source.length : end + 1;
      isCode = false;
    } else if (source.startsWith("/*", cursor)) {
      const end = source.indexOf("*/", cursor + 2);
      cursor = end < 0 ? source.length : end + 2;
      isCode = false;
    } else if (char === "/" && regexAllowed) {
      cursor++;
      let characterClass = false;
      while (cursor < source.length) {
        const current = source[cursor++];
        if (current === "\\") { cursor++; continue; }
        if (current === "[") characterClass = true;
        if (current === "]") characterClass = false;
        if (current === "/" && !characterClass) break;
        if (current === "\n") break;
      }
      while (/[a-z]/i.test(source[cursor] ?? "")) cursor++;
      isCode = false;
      regexAllowed = false;
    } else if (/[a-z_$]/i.test(char)) {
      cursor++;
      while (/[\w$]/.test(source[cursor] ?? "")) cursor++;
      regexAllowed = /^(?:return|throw|case|delete|void|typeof|instanceof|in|of|yield|await|else|do)$/.test(source.slice(start, cursor));
    } else if (/[0-9]/.test(char)) {
      cursor++;
      while (/[\w.]/.test(source[cursor] ?? "")) cursor++;
      regexAllowed = false;
    } else {
      cursor++;
      regexAllowed = !/[\])}.]/.test(char);
      if ((char === "+" || char === "-") && source[cursor] === char) {
        cursor++;
        regexAllowed = false;
      }
    }
    while (next < offsets.length && offsets[next] < cursor) {
      if (isCode && offsets[next] === start) code.add(offsets[next]);
      next++;
    }
  }
  return code;
}

// Only explicit syntax is evidence. Arbitrary strings and dynamic expressions
// are never treated as a complete resource graph or as grounds for pruning.
function scriptReferences(source) {
  const candidates = [];
  const expressions = [...source.matchAll(/(?<![\w$.])import\s*\(|\bnew\s+(?:Shared)?Worker\s*\(|\bnew\s+URL\s*\(/g)];
  const add = (pattern, valueIndex, kind, rootRelative = false) => {
    for (const match of source.matchAll(pattern)) {
      candidates.push({ value: match[valueIndex], kind, rootRelative, start: match.index, end: match.index + match[0].length });
    }
  };
  add(/(?<![\w$.])import\s*(["'])([^"'\r\n]+)\1/g, 2, "module");
  add(/(?<![\w$.])(?:import\s+[\w$*{},\s]+?|export\s*(?:\*|\{[^}]*\}))\s*from\s*(["'])([^"'\r\n]+)\1/g, 2, "module");
  add(/(?<![\w$.])import\s*\(\s*(["'])([^"'\r\n]+)\1\s*\)/g, 2, "dynamic import");
  add(/\bnew\s+URL\s*\(\s*(["'])([^"'\r\n]+)\1\s*,\s*import\.meta\.url\s*\)/g, 2, "URL");
  add(/\bnew\s+(?:Shared)?Worker\s*\(\s*(["'])([^"'\r\n]+)\1(?=\s*[,\)])/g, 2, "worker");
  // Vite's renderBuiltUrl contract in this payload has a fixed deployment
  // prefix followed by a literal asset path. Its suffix can be proved locally.
  add(/\b__qpBase\s*\+\s*(["'])(\/[^"'\r\n]+)\1/g, 2, "base-path asset", true);
  const positions = codeOffsets(source, [...candidates.map(entry => entry.start), ...expressions.map(entry => entry.index)]);
  const references = candidates.filter(entry => positions.has(entry.start));
  const dynamic = expressions.filter(match => positions.has(match.index)
    && !references.some(entry => entry.start <= match.index && match.index < entry.end))
    .map(match => ({ offset: match.index, expression: source.slice(match.index, match.index + 120) }));
  return { references, dynamic };
}

function htmlReferences(source) {
  const references = [];
  const modules = [];
  // Remove script bodies before reading tags: JS strings may contain markup.
  const markup = source.replace(/<!--[\s\S]*?-->/g, "").replace(/(<script\b[^>]*>)[\s\S]*?<\/script\s*>/gi, "$1</script>");
  for (const tag of markup.matchAll(/<(script|link|img|source|video|audio|object|iframe)\b([^>]*)>/gi)) {
    const attributes = new Map();
    for (const attribute of tag[2].matchAll(/([^\s=]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/g)) {
      attributes.set(attribute[1].toLowerCase(), attribute[2] ?? attribute[3] ?? attribute[4]);
    }
    const name = tag[1].toLowerCase();
    const field = name === "link" ? "href" : name === "object" ? "data" : "src";
    const value = attributes.get(field);
    if (value) {
      references.push({ value, kind: `HTML ${name}` });
      if (name === "script" && attributes.get("type") === "module") modules.push(value);
    }
    if (attributes.has("poster")) references.push({ value: attributes.get("poster"), kind: "HTML poster" });
    if (attributes.has("srcset")) {
      for (const item of attributes.get("srcset").split(",")) {
        references.push({ value: item.trim().split(/\s+/)[0], kind: "HTML srcset" });
      }
    }
  }
  return { references, modules };
}

export function checkConsolePayload({ patchRoot = DEFAULT_PATCH, expectedFiles, baseConsole } = {}) {
  const errors = new Set();
  const warnings = new Set();
  const references = new Map();
  const dynamicReferences = [];
  const files = new Map();
  const declared = new Map();
  const deleted = new Set();
  const error = message => errors.add(message);
  const result = () => ({
    ok: errors.size === 0, errors: [...errors].sort(), warnings: [...warnings].sort(),
    files: declared.size, deleteFiles: deleted.size, inheritedFiles, compressedPairs, mode,
    checkedReferences: [...references.values()].reduce((sum, refs) => sum + refs.size, 0),
    dynamicReferences, unprovenFiles,
  });
  let compressedPairs = 0;
  let inheritedFiles = 0;
  let mode = "unknown";
  let unprovenFiles = [];
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(path.join(patchRoot, "manifest.json"), "utf8"));
  } catch (cause) {
    error(`cannot read payload manifest: ${cause.message}`);
    return result();
  }
  if (!object(manifest) || manifest.schema_version !== 1 || !Array.isArray(manifest.files) || !manifest.files.length) {
    error("invalid payload manifest schema/files");
    return result();
  }
  if (expectedFiles !== undefined && manifest.files.length !== expectedFiles) {
    error(`manifest file count: expected ${expectedFiles}, got ${manifest.files.length}`);
  }
  const folded = new Set();
  for (const entry of manifest.files) {
    if (!object(entry) || !safePath(entry.path)) {
      error(`unsafe payload path: ${JSON.stringify(entry?.path)}`);
      continue;
    }
    if (folded.has(entry.path.toLowerCase())) error(`duplicate payload path: ${entry.path}`);
    folded.add(entry.path.toLowerCase());
    if (!validHash(entry.sha256)) error(`invalid SHA256: ${entry.path}`);
    if (validHash(entry.original_sha256) && entry.original_sha256 === entry.sha256) inheritedFiles++;
    declared.set(entry.path, entry.sha256);
  }
  if (manifest.delete_files !== undefined && !Array.isArray(manifest.delete_files)) {
    error("delete_files must be an array");
  }
  const deletedFolded = new Set();
  for (const entry of Array.isArray(manifest.delete_files) ? manifest.delete_files : []) {
    if (!object(entry) || !safePath(entry.path)) {
      error(`unsafe delete_files path: ${JSON.stringify(entry?.path)}`);
      continue;
    }
    const key = entry.path.toLowerCase();
    if (deletedFolded.has(key)) error(`duplicate delete_files path: ${entry.path}`);
    if (folded.has(key)) error(`delete_files overlaps files: ${entry.path}`);
    deletedFolded.add(key);
    deleted.add(entry.path);
    if (!validHash(entry.original_sha256)) error(`invalid delete_files original_sha256: ${entry.path}`);
    if (entry.previous_sha256 !== undefined && (!Array.isArray(entry.previous_sha256)
        || entry.previous_sha256.some(hash => !validHash(hash)))) {
      error(`invalid delete_files previous_sha256: ${entry.path}`);
    }
  }
  const payload = path.join(patchRoot, "payload");
  function walk(directory, prefix = "") {
    const metadata = lstatSync(directory);
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error(`non-directory payload root: ${directory}`);
    for (const child of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const relative = prefix + child.name;
      if (!safePath(relative)) { error(`unsafe filesystem path: ${relative}`); continue; }
      if (child.isSymbolicLink() || (!child.isFile() && !child.isDirectory())) {
        error(`non-regular payload entry: ${relative}`);
      } else if (child.isDirectory()) {
        walk(path.join(directory, child.name), relative + "/");
      } else {
        const bytes = readFileSync(path.join(directory, child.name));
        files.set(relative, bytes);
        if (!declared.has(relative)) error(`extra payload file: ${relative}`);
        else if (sha256(bytes) !== declared.get(relative)) error(`SHA256 mismatch: ${relative}`);
      }
    }
  }
  try { walk(payload); } catch (cause) { error(`cannot inspect payload: ${cause.message}`); }
  for (const relative of declared.keys()) if (!files.has(relative)) error(`missing payload file: ${relative}`);

  mode = manifest.console_complete === true ? "complete" : "delta";
  if (manifest.console_complete !== undefined && typeof manifest.console_complete !== "boolean") {
    error("console_complete must be a boolean");
  }
  const effective = new Map();
  if (mode === "delta") {
    if (!baseConsole) {
      error("delta manifest requires --base-console from the verified official wheel; effective install references cannot be verified without its baseline");
    } else {
      function readBase(directory, prefix = "console/") {
        if (!lstatSync(directory).isDirectory() || lstatSync(directory).isSymbolicLink()) {
          throw new Error(`non-directory baseline: ${directory}`);
        }
        for (const child of readdirSync(directory, { withFileTypes: true })) {
          const name = prefix + child.name;
          if (!safePath(name) || child.isSymbolicLink() || (!child.isDirectory() && !child.isFile())) {
            throw new Error(`unsafe baseline entry: ${name}`);
          }
          if (child.isDirectory()) readBase(path.join(directory, child.name), name + "/");
          else effective.set(name, readFileSync(path.join(directory, child.name)));
        }
      }
      try { readBase(baseConsole); } catch (cause) { error(`cannot inspect official Console baseline: ${cause.message}`); }
    }
    for (const name of deleted) effective.delete(name);
  }
  for (const [name, bytes] of files) effective.set(name, bytes);
  // Missing delta input is an unavailable baseline, not evidence of missing
  // resources on a deployed installation. Payload hashes still get checked.
  if (mode === "delta" && !baseConsole) return result();
  const originals = [...effective.keys()].filter(name => name.startsWith("console/") && !/\.(?:gz|br)$/.test(name));
  for (const relative of effective.keys()) {
    if (!relative.startsWith("console/") || !/\.(?:gz|br)$/.test(relative)) continue;
    if (!effective.has(relative.replace(/\.(?:gz|br)$/, ""))) error(`orphan compressed file: ${relative}`);
  }
  for (const relative of originals) {
    const original = effective.get(relative);
    const required = COMPRESSIBLE.test(relative) && original.length >= PRECOMPRESS_MIN_BYTES;
    const hasSidecar = effective.has(relative + ".gz") || effective.has(relative + ".br");
    for (const [suffix, decompress] of [[".gz", gunzipSync], [".br", brotliDecompressSync]]) {
      const compressed = effective.get(relative + suffix);
      if (!compressed) {
        if (required || hasSidecar) error(`missing compressed file: ${relative + suffix}`);
        continue;
      }
      try {
        const decoded = decompress(compressed, { maxOutputLength: original.length + 1 });
        if (!decoded.equals(original)) error(`compressed content mismatch: ${relative + suffix}`);
      } catch (cause) { error(`invalid compressed content: ${relative + suffix}: ${cause.message}`); }
    }
    if (effective.has(relative + ".gz") && effective.has(relative + ".br")) compressedPairs++;
  }

  function reference(from, raw, { rootRelative = false, bareIsExternal = false } = {}) {
    if (typeof raw !== "string" || !raw.trim()) { error(`empty resource reference in ${from}`); return null; }
    const value = raw.trim().replaceAll("&amp;", "&");
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/|#)/i.test(value)) return null;
    if (bareIsExternal && !value.startsWith(".") && !value.startsWith("/")) {
      warnings.add(`external/bare module reference in ${from}: ${value}`);
      return null;
    }
    let pathname;
    try { pathname = decodeURIComponent(value.split(/[?#]/, 1)[0]); }
    catch { error(`invalid URL reference in ${from}: ${value}`); return null; }
    if (/[\\:\x00-\x1f\x7f]/.test(pathname)) { error(`unsafe URL reference in ${from}: ${value}`); return null; }
    const resolved = pathname.startsWith("/") || rootRelative
      ? path.posix.normalize("console/" + pathname.replace(/^\/+/, ""))
      : path.posix.normalize(path.posix.join(path.posix.dirname(from), pathname));
    if (!safePath(resolved) || !resolved.startsWith("console/")) {
      error(`resource reference escapes console in ${from}: ${value}`);
      return null;
    }
    if (!references.has(from)) references.set(from, new Set());
    references.get(from).add(resolved);
    if (!effective.has(resolved) || (mode === "complete" && !declared.has(resolved))) error(`missing resource reference: ${from} -> ${resolved}`);
    return resolved;
  }

  const vitePath = "console/.vite/manifest.json";
  let vite;
  try {
    vite = JSON.parse(effective.get(vitePath)?.toString("utf8") ?? "null");
    if (!object(vite) || !Object.keys(vite).length) throw new Error("expected a nonempty Vite manifest object");
  } catch (cause) { error(`invalid Vite manifest: ${cause.message}`); }
  const viteEdges = new Map();
  if (object(vite)) {
    for (const [key, chunk] of Object.entries(vite)) {
      if (!object(chunk) || !safePath(chunk.file)) { error(`invalid Vite file for ${key}`); continue; }
      const from = reference(vitePath, chunk.file, { rootRelative: true });
      const edges = [];
      for (const field of ["imports", "dynamicImports", "css", "assets", "workers"]) {
        if (chunk[field] === undefined) continue;
        if (!Array.isArray(chunk[field]) || chunk[field].some(value => typeof value !== "string")) {
          error(`invalid Vite ${field} for ${key}`);
          continue;
        }
        for (const value of chunk[field]) {
          if (field === "imports" || field === "dynamicImports") {
            if (!Object.hasOwn(vite, value)) error(`missing Vite ${field} key: ${key} -> ${value}`);
            else edges.push(value);
          } else {
            reference(from ?? vitePath, value, { rootRelative: true });
          }
        }
      }
      viteEdges.set(key, edges);
    }
    // Resolve both static and dynamic graph edges, including disconnected
    // entries and cycles. A source key such as src/... is never a disk path.
    for (const [key, edges] of viteEdges) {
      const from = "console/" + vite[key].file;
      for (const target of edges) {
        if (safePath(vite[target]?.file)) reference(from, vite[target].file, { rootRelative: true });
        else error(`invalid Vite import target: ${key} -> ${target}`);
      }
    }
  }

  let htmlModules = [];
  for (const relative of originals) {
    const source = effective.get(relative).toString("utf8");
    if (/\.html$/.test(relative)) {
      const parsed = htmlReferences(source);
      for (const ref of parsed.references) reference(relative, ref.value);
      if (relative === "console/index.html") {
        htmlModules = parsed.modules.map(value => reference(relative, value)).filter(Boolean);
        if (!htmlModules.length) error("index.html has no direct module entry");
      }
      for (const script of source.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script\s*>/gi)) {
        const parsedScript = scriptReferences(script[1]);
        for (const ref of parsedScript.references) reference(relative, ref.value, { ...ref, bareIsExternal: ref.kind.includes("module") || ref.kind === "dynamic import" });
        for (const entry of parsedScript.dynamic) dynamicReferences.push({ file: relative, ...entry });
      }
    } else if (/\.(?:m?js)$/.test(relative)) {
      const parsed = scriptReferences(source);
      for (const ref of parsed.references) reference(relative, ref.value, { ...ref, bareIsExternal: ref.kind === "module" || ref.kind === "dynamic import" });
      for (const entry of parsed.dynamic) dynamicReferences.push({ file: relative, ...entry });
    } else if (/\.css$/.test(relative)) {
      for (const match of source.matchAll(/\burl\(\s*(?:"([^"]*)"|'([^']*)'|([^\s)]+))\s*\)|@import\s+["']([^"']+)["']/gi)) {
        reference(relative, match[1] ?? match[2] ?? match[3] ?? match[4]);
      }
    }
  }
  if (!effective.has("console/index.html")) error("missing Console index.html");
  if (object(vite)) {
    const entries = Object.values(vite).filter(chunk => object(chunk) && chunk.isEntry).map(chunk => "console/" + chunk.file);
    if (!entries.length) error("Vite manifest has no entry");
    for (const entry of htmlModules) if (!entries.includes(entry)) error(`HTML module is not a Vite entry: ${entry}`);
    for (const entry of entries) if (!htmlModules.includes(entry)) error(`Vite entry missing from index.html: ${entry}`);
  }
  const reachable = new Set();
  const pending = ["console/index.html"];
  while (pending.length) {
    const name = pending.pop();
    if (reachable.has(name)) continue;
    reachable.add(name);
    pending.push(...(references.get(name) ?? []));
  }
  unprovenFiles = originals.filter(name => name !== vitePath && !reachable.has(name)).sort();
  if (dynamicReferences.length) warnings.add(`${dynamicReferences.length} dynamic expressions cannot be proved statically; report only, keep all files`);
  if (unprovenFiles.length) warnings.add(`${unprovenFiles.length} resources have no proved path from index.html; report only, keep all files`);
  return result();
}

function main(argv) {
  let patchRoot = DEFAULT_PATCH;
  let expectedFiles;
  let baseConsole;
  let json = false;
  for (let index = 0; index < argv.length; index++) {
    if (argv[index] === "--patch-root" && argv[index + 1]) patchRoot = path.resolve(argv[++index]);
    else if (argv[index] === "--base-console" && argv[index + 1]) baseConsole = path.resolve(argv[++index]);
    else if (argv[index] === "--expected-files" && /^\d+$/.test(argv[index + 1] ?? "")) expectedFiles = Number(argv[++index]);
    else if (argv[index] === "--json") json = true;
    else if (argv[index] === "--help") {
      console.log("Usage: node scripts/check-console-payload.mjs [--patch-root DIR] [--base-console VERIFIED_OFFICIAL_CONSOLE] [--expected-files N] [--json]");
      return;
    } else throw new Error(`unknown or incomplete argument: ${argv[index]}`);
  }
  const report = checkConsolePayload({ patchRoot, expectedFiles, baseConsole });
  if (json) console.log(JSON.stringify(report, null, 2));
  else {
    console.log(`Console payload: ${report.files} files, ${report.inheritedFiles} unchanged upstream files retained, ${report.deleteFiles} deletion records, ${report.compressedPairs} gzip/br pairs, ${report.checkedReferences} references`);
    for (const message of report.errors) console.error(`ERROR ${message}`);
    for (const message of report.warnings) console.log(`REPORT ${message}`);
    for (const name of report.unprovenFiles) console.log(`UNPROVEN ${name}`);
    for (const entry of report.dynamicReferences) console.log(`DYNAMIC ${entry.file}:${entry.offset} ${entry.expression}`);
    console.log(report.ok ? "Console payload integrity passed" : "Console payload integrity failed");
  }
  if (!report.ok) process.exitCode = 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(process.argv.slice(2)); }
  catch (cause) { console.error(cause.message); process.exitCode = 1; }
}
