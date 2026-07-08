import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktopRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const contractPaths = [
  path.join(desktopRoot, "src", "generated", "admin-read-model.contract.json"),
  path.join(desktopRoot, "src", "generated", "request-chain.contract.json"),
];
const contracts = contractPaths.map((contractPath) =>
  JSON.parse(fs.readFileSync(contractPath, "utf8")),
);

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

function checkRustTarget(target) {
  for (const field of target.fields ?? []) {
    requireText(target.file, `pub ${field}:`, `${target.struct}.${field}`);
  }
}

function checkTypescriptTarget(target) {
  for (const field of target.fields) {
    requireText(target.file, `${field}`, `${target.type}.${field}`);
  }
}

for (const contract of contracts) {
  const rustTargets = Array.isArray(contract.rust) ? contract.rust : [contract.rust].filter(Boolean);
  for (const target of rustTargets) {
    checkRustTarget(target);
  }

  for (const target of contract.typescript ?? []) {
    checkTypescriptTarget(target);
  }

  for (const requirement of contract.requiredText ?? []) {
    requireText(requirement.file, requirement.text, requirement.reason ?? contract.contract);
  }
}

if (failures.length > 0) {
  console.error("Admin contract drift detected:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Admin contracts in sync: ${contracts.map((contract) => contract.contract).join(", ")}.`);
