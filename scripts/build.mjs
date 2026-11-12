#!/usr/bin/env node
// Check every package and build the registry into dist/.
//
//   node scripts/build.mjs --check              only check the packages
//   node scripts/build.mjs                      build dist/ from scratch
//   node scripts/build.mjs --previous <folder>  build on top of what is already
//                                               published (a checkout of the
//                                               `published` branch)
//
// A version that is already published is never rebuilt or changed: its files
// are copied as they are. Changing a package without raising its version
// fails the build. With REGISTRY_SIGNING_KEY set, index.json is signed.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  compareVersions, FORMAT, listFiles, loadPackages, privateKeyFromSeed, readJson, ROOT, sha256, signIndex,
  validVersion, verifyIndex, publicKeyBase64,
} from "./lib.mjs";

const args = process.argv.slice(2);
const checkOnly = args.includes("--check");
const previous = args.includes("--previous") ? args[args.indexOf("--previous") + 1] : null;
const DIST = join(ROOT, "dist");
const TARGET = "wasm32-unknown-unknown";

const { packages, errors } = loadPackages();
if (errors.length > 0) {
  console.error(errors.map((e) => `error: ${e}`).join("\n"));
  process.exit(1);
}
console.log(`${packages.length} packages are valid.`);
if (checkOnly) process.exit(0);

/** Everything a package is built from, hashed: a change here needs a new version. */
function sourceHash(pkg) {
  const parts = [];
  const add = (dir, files) => {
    for (const f of files) parts.push(`${f}\0${sha256(readFileSync(join(dir, f)))}`);
  };
  // Only the package's own folder counts. A change to the SDK reaches a
  // module the next time that module's version is raised.
  add(pkg.dir, listFiles(pkg.dir).filter((f) => !f.startsWith("target/")));
  return sha256(parts.join("\n"));
}

rmSync(DIST, { recursive: true, force: true });
mkdirSync(join(DIST, "packages"), { recursive: true });
if (previous) {
  if (existsSync(join(previous, "packages"))) cpSync(join(previous, "packages"), join(DIST, "packages"), { recursive: true });
}

const modules = packages.filter((p) => p.manifest.kind === "module");
const toBuild = modules.filter((p) => !existsSync(join(DIST, "packages", p.manifest.id, p.manifest.version)));
if (toBuild.length > 0) {
  const crates = toBuild.flatMap((p) => ["-p", p.manifest.module.crate]);
  execFileSync("cargo", ["build", "--release", "--locked", "--target", TARGET, ...crates], { cwd: ROOT, stdio: "inherit" });
}

const now = process.env.SOURCE_DATE_EPOCH ? Number(process.env.SOURCE_DATE_EPOCH) * 1000 : Date.now();
let failed = false;
for (const pkg of packages) {
  const { id, version, kind } = pkg.manifest;
  const out = join(DIST, "packages", id, version);
  const source = sourceHash(pkg);
  if (existsSync(out)) {
    const published = readJson(join(out, "package.json"));
    if (published.source_sha256 !== source) {
      console.error(`error: ${id} changed but ${version} is already published. Raise its version.`);
      failed = true;
    }
    continue;
  }
  mkdirSync(join(out, "files"), { recursive: true });
  if (kind === "module") {
    const wasm = join(ROOT, "target", TARGET, "release", `${pkg.manifest.module.crate.replaceAll("-", "_")}.wasm`);
    cpSync(wasm, join(out, "files", "module.wasm"));
  } else {
    cpSync(join(pkg.dir, "files"), join(out, "files"), { recursive: true });
  }
  const files = listFiles(join(out, "files")).map((path) => {
    const data = readFileSync(join(out, "files", path));
    return { path, size: data.length, sha256: sha256(data) };
  });
  const manifest = { format: FORMAT, ...pkg.manifest };
  if (kind === "module") {
    const { crate: _crate, ...module } = pkg.manifest.module;
    manifest.module = { ...module, entry: "module.wasm" };
  }
  Object.assign(manifest, { files, source_sha256: source, published_at: now });
  writeFileSync(join(out, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`built ${id} ${version} (${files.length} files)`);
}
if (failed) process.exit(1);

// The index: every published version of every package, newest first. The
// descriptive fields come from the newest version.
const index = { format: FORMAT, packages: [] };
for (const id of readdirSync(join(DIST, "packages")).sort()) {
  const versions = readdirSync(join(DIST, "packages", id)).filter(validVersion).sort(compareVersions).reverse();
  if (versions.length === 0) continue;
  const entries = versions.map((version) => {
    const path = `packages/${id}/${version}/package.json`;
    const raw = readFileSync(join(DIST, path));
    const m = JSON.parse(raw);
    return {
      manifest: m,
      entry: {
        version,
        ...(m.min_kinora ? { min_kinora: m.min_kinora } : {}),
        ...(m.module ? { abi: m.module.abi } : {}),
        size: m.files.reduce((n, f) => n + f.size, 0),
        published_at: m.published_at,
        manifest: path,
        sha256: sha256(raw),
      },
    };
  });
  const latest = entries[0].manifest;
  index.packages.push({
    id,
    kind: latest.kind,
    name: latest.name,
    description: latest.description,
    author: latest.author,
    license: latest.license,
    ...(latest.homepage ? { homepage: latest.homepage } : {}),
    recommended: latest.recommended === true,
    ...(latest.module ? { provides: latest.module.provides, permissions: latest.module.permissions ?? {} } : {}),
    versions: entries.map((e) => e.entry),
  });
}
const indexBytes = Buffer.from(`${JSON.stringify(index, null, 2)}\n`);
writeFileSync(join(DIST, "index.json"), indexBytes);

const seed = process.env.REGISTRY_SIGNING_KEY;
if (seed?.trim()) {
  const key = privateKeyFromSeed(seed);
  const signature = signIndex(indexBytes, key);
  if (!verifyIndex(indexBytes, signature, publicKeyBase64(key))) throw new Error("the signature does not verify");
  writeFileSync(join(DIST, "index.json.sig"), `${JSON.stringify(signature, null, 2)}\n`);
  console.log(`signed index.json with key ${signature.key_id}`);
} else {
  console.log("REGISTRY_SIGNING_KEY is not set: index.json is not signed.");
}
const count = index.packages.reduce((n, p) => n + p.versions.length, 0);
const bytes = listFiles(DIST).reduce((n, f) => n + statSync(join(DIST, f)).size, 0);
console.log(`dist/: ${index.packages.length} packages, ${count} versions, ${bytes} bytes.`);
