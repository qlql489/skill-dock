// Single source of truth for the app version is package.json.
// tauri.conf.json points at it via "version": "../package.json"; this script
// mirrors the value into Cargo.toml / Cargo.lock so `cargo` metadata and the
// built binary stay in sync. Release flow: bump package.json, then tag.
import { readFileSync, writeFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const version = pkg.version;
if (!/^\d+\.\d+\.\d+/.test(version)) {
  console.error(`[sync-version] package.json version looks invalid: ${version}`);
  process.exit(1);
}

const tomlPath = new URL("../src-tauri/Cargo.toml", import.meta.url);
let toml = readFileSync(tomlPath, "utf8");
// First `version = "…"` in the file is [package]'s ([package] is the first table).
toml = toml.replace(/^version = ".*"$/m, `version = "${version}"`);
writeFileSync(tomlPath, toml);

const lockPath = new URL("../src-tauri/Cargo.lock", import.meta.url);
let lock = readFileSync(lockPath, "utf8");
lock = lock.replace(
  /name = "(?:skill-manager|skill-dock)"\nversion = ".*"/,
  `name = "skill-dock"\nversion = "${version}"`,
);
writeFileSync(lockPath, lock);

console.log(`[sync-version] ${version} -> Cargo.toml / Cargo.lock`);
