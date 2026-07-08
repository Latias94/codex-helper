import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktopRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const contractPath = path.join(desktopRoot, "src", "generated", "admin-read-model.contract.json");
const contract = JSON.parse(fs.readFileSync(contractPath, "utf8"));

const failures = [];

function readContractFile(relativePath) {
  return fs.readFileSync(path.join(desktopRoot, relativePath), "utf8");
}

function requireText(file, text, reason) {
  const contents = readContractFile(file);
  if (!contents.includes(text)) {
    failures.push(`${file}: missing ${JSON.stringify(text)} (${reason})`);
  }
}

for (const field of contract.rust.fields) {
  requireText(contract.rust.file, `pub ${field}:`, `${contract.rust.struct}.${field}`);
}

for (const target of contract.typescript) {
  for (const field of target.fields) {
    requireText(target.file, `${field}`, `${target.type}.${field}`);
  }
}

if (failures.length > 0) {
  console.error("Admin read-model contract drift detected:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Admin read-model contract ${contract.contract} is in sync.`);
