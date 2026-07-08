import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktopRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(desktopRoot, "..", "..");

const checks = [
  {
    label: "root Cargo.toml package",
    version: packageVersionFromCargoToml(path.join(repoRoot, "Cargo.toml")),
  },
  {
    label: "core crate",
    version: packageVersionFromCargoToml(path.join(repoRoot, "crates", "core", "Cargo.toml")),
  },
  {
    label: "tui crate",
    version: packageVersionFromCargoToml(path.join(repoRoot, "crates", "tui", "Cargo.toml")),
  },
  {
    label: "desktop crate",
    version: packageVersionFromCargoToml(path.join(desktopRoot, "src-tauri", "Cargo.toml")),
  },
  {
    label: "desktop package.json",
    version: JSON.parse(fs.readFileSync(path.join(desktopRoot, "package.json"), "utf8")).version,
  },
  {
    label: "tauri.conf.json",
    version: JSON.parse(fs.readFileSync(path.join(desktopRoot, "src-tauri", "tauri.conf.json"), "utf8")).version,
  },
];

const expected = checks[0].version;
const failures = checks.filter((check) => check.version !== expected);

if (failures.length > 0) {
  console.error(`Version sync failed; expected ${expected}.`);
  for (const failure of failures) {
    console.error(`- ${failure.label}: ${failure.version}`);
  }
  process.exit(1);
}

console.log(`Version sync OK: ${expected}`);

function packageVersionFromCargoToml(file) {
  const text = fs.readFileSync(file, "utf8");
  const lines = text.split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === "[package]");
  if (start === -1) {
    throw new Error(`${file}: missing [package] section`);
  }
  for (const line of lines.slice(start + 1)) {
    if (line.trim().startsWith("[")) {
      break;
    }
    const version = line.match(/^\s*version\s*=\s*"(?<version>[^"]+)"/);
    if (version?.groups?.version) {
      return version.groups.version;
    }
  }
  throw new Error(`${file}: missing [package].version`);
}
