// Shared by the registry scripts: reading and checking packages, hashing,
// signing. No dependencies beyond Node itself.
import { createHash, createPrivateKey, createPublicKey, sign, verify } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

export const ROOT = new URL("..", import.meta.url).pathname.replace(/\/$/, "");
export const PACKAGES = join(ROOT, "packages");

/** The index and manifest layout this registry writes. Kinora refuses formats it doesn't know. */
export const FORMAT = 1;
/** Module ABIs a module may target (docs/modules.md). */
export const MODULE_ABIS = [1];

export const KINDS = ["module", "theme", "player-skin", "app-skin"];
export const EXTENSION_POINTS = ["metadata_provider", "track_selection", "event_subscriber"];
export const PERMISSIONS = ["http", "storage", "activity"];
export const SETTING_KINDS = ["text", "secret", "bool"];

// The same limits Kinora enforces when it installs a package.
export const MAX_FILE_BYTES = 4 * 1024 * 1024;
export const MAX_PACKAGE_BYTES = 20 * 1024 * 1024;
export const MAX_FILES = 300;

/** Theme tokens and rules, the same as Kinora's `Theme::validate`. */
export const THEME_TOKENS = [
  "background", "foreground", "card", "card-foreground", "muted", "muted-foreground", "primary",
  "primary-foreground", "accent", "accent-foreground", "border", "ring", "destructive", "radius", "font-sans",
];
const PLAYER_OPTIONS = ["bar-blur", "seek-height", "button-radius", "controls-opacity"];
/** Theme ids Kinora ships itself; a package can't replace them. */
const RESERVED_THEMES = ["kinora", "kinora-light"];

export const sha256 = (data) => createHash("sha256").update(data).digest("hex");
export const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
export const validId = (id) => typeof id === "string" && /^[a-z0-9][a-z0-9-]{0,63}$/.test(id);
export const validVersion = (v) => typeof v === "string" && /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(v);

/** A host a module may call: a DNS name, not an IP address. The same rule as Kinora's `valid_http_host`. */
export const validHttpHost = (h) =>
  typeof h === "string" &&
  h.length <= 253 &&
  h.includes(".") &&
  /^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$/.test(h) &&
  /[a-z]/.test(h);

/** Negative when a is older than b. Versions are plain x.y.z. */
export function compareVersions(a, b) {
  const [x, y] = [a, b].map((v) => v.split(".").map(Number));
  return x[0] - y[0] || x[1] - y[1] || x[2] - y[2];
}

/** Every file under `dir`, as sorted forward-slash paths relative to it. */
export function listFiles(dir) {
  const out = [];
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, entry.name);
      if (entry.isDirectory()) walk(p);
      else out.push(relative(dir, p).split(sep).join("/"));
    }
  };
  if (existsSync(dir)) walk(dir);
  return out.sort();
}

/** A path Kinora will accept inside a package: relative, plain names, nothing hidden. */
export function safePath(path) {
  return (
    path.length <= 200 &&
    path.split("/").every((part) => /^[A-Za-z0-9_][A-Za-z0-9._-]*$/.test(part) && !part.endsWith("."))
  );
}

function checkTheme(theme, file, errors, themeIds) {
  const err = (m) => errors.push(`${file}: ${m}`);
  if (!validId(theme.id)) return err("invalid theme id");
  if (`${theme.id}.json` !== file.split("/").pop()) err(`must be named ${theme.id}.json`);
  if (RESERVED_THEMES.includes(theme.id)) err(`"${theme.id}" is one of Kinora's own themes`);
  if (themeIds.has(theme.id)) err(`theme id "${theme.id}" is used by another package`);
  themeIds.add(theme.id);
  if (typeof theme.name !== "string" || !theme.name.trim()) err("name is missing");
  if (!["dark", "light"].includes(theme.scheme ?? "dark")) err('scheme must be "dark" or "light"');
  if (theme.pair !== undefined && (!validId(theme.pair) || theme.pair === theme.id)) err("invalid pair");
  const badValue = (v) => typeof v !== "string" || v.length > 200 || /[;{}<>]/.test(v);
  for (const [k, v] of Object.entries(theme.tokens ?? {})) {
    if (!THEME_TOKENS.includes(k)) err(`unknown token "${k}"`);
    if (badValue(v)) err(`invalid value for "${k}"`);
  }
  for (const token of THEME_TOKENS) {
    if (!(token in (theme.tokens ?? {}))) err(`token "${token}" is missing`);
  }
  if (theme.player?.layout !== undefined && !validId(theme.player.layout)) err("invalid player layout");
  for (const [k, v] of Object.entries(theme.player?.options ?? {})) {
    if (!PLAYER_OPTIONS.includes(k)) err(`unknown player option "${k}"`);
    if (badValue(v) || /url\(/i.test(v)) err(`invalid value for player option "${k}"`);
  }
}

function checkPlayerSkin(dir, pkg, errors) {
  const read = (name) => (existsSync(join(dir, name)) ? readFileSync(join(dir, name), "utf8") : null);
  const json = read("skin.json");
  const html = read("skin.html");
  if (!json || !html?.trim()) return errors.push("a player skin needs files/skin.json and files/skin.html");
  const skin = JSON.parse(json);
  if (skin.id !== pkg.id) errors.push(`skin.json id must be "${pkg.id}"`);
  if (typeof skin.name !== "string" || !skin.name.trim() || skin.name.length > 80) errors.push("skin.json name");
  for (const bad of ["<script", "<iframe", "<object", "<embed", "<link", "<style", "<base", "<meta", "javascript:"]) {
    if (html.toLowerCase().includes(bad)) errors.push(`skin.html may not contain ${bad}`);
  }
  const css = (read("skin.css") ?? "").toLowerCase();
  for (const bad of ["@import", "expression(", "-moz-binding", "behavior:", "</"]) {
    if (css.includes(bad)) errors.push(`skin.css may not contain ${bad}`);
  }
}

function checkModule(dir, pkg, errors) {
  const m = pkg.module;
  if (!m || typeof m !== "object") return errors.push('a module needs a "module" block');
  if (!MODULE_ABIS.includes(m.abi)) errors.push(`module.abi must be one of ${MODULE_ABIS}`);
  if (!Array.isArray(m.provides) || m.provides.length === 0) errors.push("module.provides is empty");
  for (const p of m.provides ?? []) {
    if (!EXTENSION_POINTS.includes(p)) errors.push(`unknown extension point "${p}"`);
  }
  for (const [k, v] of Object.entries(m.permissions ?? {})) {
    if (!PERMISSIONS.includes(k)) errors.push(`unknown permission "${k}"`);
    else if (k === "http") {
      const ok = Array.isArray(v) && v.length > 0 && v.every(validHttpHost);
      if (!ok) errors.push("permissions.http must list host names, e.g. api.example.com");
    } else if (typeof v !== "boolean") errors.push(`permissions.${k} must be true or false`);
  }
  const keys = new Set();
  for (const f of m.settings ?? []) {
    if (!/^[a-z][a-z0-9_]{0,39}$/.test(f.key ?? "")) errors.push(`invalid setting key "${f.key}"`);
    if (keys.has(f.key)) errors.push(`setting "${f.key}" is listed twice`);
    keys.add(f.key);
    if (typeof f.label !== "string" || !f.label.trim()) errors.push(`setting "${f.key}" needs a label`);
    if (!SETTING_KINDS.includes(f.kind)) errors.push(`setting "${f.key}": unknown kind "${f.kind}"`);
    if (f.kind === "bool" && f.default !== undefined && !["true", "false"].includes(f.default)) {
      errors.push(`setting "${f.key}": default must be "true" or "false"`);
    }
  }
  const cargo = join(dir, "Cargo.toml");
  if (!existsSync(cargo)) return errors.push("a module needs a Cargo.toml");
  const toml = readFileSync(cargo, "utf8");
  const name = toml.match(/^name\s*=\s*"([^"]+)"/m)?.[1];
  const version = toml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (name !== m.crate) errors.push(`module.crate ("${m.crate}") must be the Cargo package name ("${name}")`);
  if (version !== pkg.version) errors.push(`Cargo.toml version (${version}) must match package.json (${pkg.version})`);
}

/**
 * Read and check every package. Returns `{ packages, errors }`; each package
 * is `{ dir, manifest, files }` where `files` are the content files (not the
 * built module).
 */
export function loadPackages() {
  const errors = [];
  const packages = [];
  const themeIds = new Set();
  for (const entry of readdirSync(PACKAGES, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isDirectory()) continue;
    const dir = join(PACKAGES, entry.name);
    const own = [];
    let pkg;
    try {
      pkg = readJson(join(dir, "package.json"));
    } catch (e) {
      errors.push(`${entry.name}: package.json: ${e.message}`);
      continue;
    }
    if (!validId(pkg.id)) own.push("invalid id (lowercase letters, digits and dashes)");
    if (pkg.id !== entry.name) own.push(`the folder must be named after the id ("${pkg.id}")`);
    if (!KINDS.includes(pkg.kind)) own.push(`kind must be one of ${KINDS.join(", ")}`);
    if (!validVersion(pkg.version)) own.push("version must look like 1.2.3");
    if (pkg.min_kinora !== undefined && !validVersion(pkg.min_kinora)) own.push("min_kinora must look like 1.2.3");
    if (typeof pkg.name !== "string" || !pkg.name.trim() || pkg.name.length > 60) own.push("name: 1-60 characters");
    if (typeof pkg.description !== "string" || !pkg.description.trim() || pkg.description.length > 300) {
      own.push("description: 1-300 characters");
    }
    if (typeof pkg.author !== "string" || !pkg.author.trim()) own.push("author is missing");
    if (typeof pkg.license !== "string" || !pkg.license.trim()) own.push("license is missing (an SPDX id)");
    if (pkg.homepage !== undefined && !/^https:\/\//.test(pkg.homepage)) own.push("homepage must be an https URL");
    if (pkg.kind !== "module" && pkg.module !== undefined) own.push('only modules have a "module" block');

    const filesDir = join(dir, "files");
    const files = listFiles(filesDir);
    let total = 0;
    for (const f of files) {
      if (!safePath(f)) own.push(`files/${f}: only letters, digits, . _ - in names, and no hidden files`);
      const size = statSync(join(filesDir, f)).size;
      total += size;
      if (size > MAX_FILE_BYTES) own.push(`files/${f} is larger than ${MAX_FILE_BYTES} bytes`);
    }
    if (files.length > MAX_FILES) own.push(`more than ${MAX_FILES} files`);
    if (total > MAX_PACKAGE_BYTES) own.push(`larger than ${MAX_PACKAGE_BYTES} bytes`);

    try {
      if (pkg.kind === "module") {
        if (files.length > 0) own.push("a module has no files/ folder (its module.wasm is built from src/)");
        checkModule(dir, pkg, own);
      } else if (files.length === 0) {
        own.push("the files/ folder is empty");
      } else if (pkg.kind === "theme") {
        for (const f of files) {
          if (f.includes("/") || !f.endsWith(".json")) own.push(`files/${f}: a theme package holds only *.json files`);
          else checkTheme(readJson(join(filesDir, f)), f, own, themeIds);
        }
      } else if (pkg.kind === "player-skin") {
        checkPlayerSkin(filesDir, pkg, own);
      } else if (pkg.kind === "app-skin") {
        const skin = files.includes("skin.json") ? readJson(join(filesDir, "skin.json")) : null;
        if (!skin) own.push("an app skin needs files/skin.json");
        else {
          if (skin.id !== pkg.id) own.push(`skin.json id must be "${pkg.id}"`);
          if (skin.version !== pkg.version) own.push(`skin.json version must be ${pkg.version}`);
        }
      }
    } catch (e) {
      own.push(e.message);
    }
    errors.push(...own.map((e) => `${entry.name}: ${e}`));
    packages.push({ dir, manifest: pkg, files });
  }
  return { packages, errors };
}

// --- Signing: Ed25519 over the exact bytes of index.json ---------------------

const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

/** A private key from its base64 32-byte seed (what `keygen` prints). */
export function privateKeyFromSeed(seedBase64) {
  const seed = Buffer.from(seedBase64.trim(), "base64");
  if (seed.length !== 32) throw new Error("the signing key must be a base64 32-byte Ed25519 seed");
  return createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, seed]), format: "der", type: "pkcs8" });
}

/** The raw 32-byte public key, base64: what Kinora pins. */
export function publicKeyBase64(privateKey) {
  return createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32).toString("base64");
}

/** Short name for a key, so a registry can change keys without breaking old servers. */
export const keyId = (publicBase64) => sha256(Buffer.from(publicBase64, "base64")).slice(0, 16);

export function signIndex(indexBytes, privateKey) {
  const publicKey = publicKeyBase64(privateKey);
  return {
    algorithm: "ed25519",
    key_id: keyId(publicKey),
    signature: sign(null, indexBytes, privateKey).toString("base64"),
  };
}

export function verifyIndex(indexBytes, signature, publicBase64) {
  const key = createPublicKey({
    key: Buffer.concat([SPKI_PREFIX, Buffer.from(publicBase64, "base64")]),
    format: "der",
    type: "spki",
  });
  return verify(null, indexBytes, key, Buffer.from(signature.signature, "base64"));
}
