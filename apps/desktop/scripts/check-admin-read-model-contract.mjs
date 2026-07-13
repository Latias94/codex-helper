import fs from "node:fs";
import {
  buildDesktopContracts,
  contractOutputPath,
  formatContractJson,
  parseTypescriptObjectFields,
  parseTypescriptObjectShape,
  parseTypescriptStringUnion,
  typescriptObjectShapeFailures,
} from "./desktop-contracts.mjs";

const generatedContracts = buildDesktopContracts();

const failures = [];

function checkTypescriptTarget(target) {
  const actualFields = parseTypescriptObjectFields(target.file, target.type);
  for (const field of target.fields) {
    if (!actualFields.includes(field)) {
      failures.push(`${target.file}: ${target.type} missing field ${field}`);
    }
  }

  const extraFields = actualFields.filter((field) => !target.fields.includes(field));
  for (const field of extraFields) {
    failures.push(`${target.file}: ${target.type} has extra field ${field}`);
  }

  if (target.shape) {
    const actualShape = parseTypescriptObjectShape(target.file, target.type);
    failures.push(
      ...typescriptObjectShapeFailures(
        actualShape,
        target.shape,
        `${target.file}: ${target.type}`,
      ),
    );
  }
}

for (const { output, contract } of generatedContracts) {
  const outputPath = contractOutputPath(output);
  const expected = formatContractJson(contract);
  const actual = fs.existsSync(outputPath) ? fs.readFileSync(outputPath, "utf8") : "";
  if (actual !== expected) {
    failures.push(`${output}: generated contract is out of date; run pnpm --dir apps/desktop generate:contracts`);
  }

  for (const target of contract.typescript ?? []) {
    checkTypescriptTarget(target);
  }

  for (const enumTarget of contract.enums ?? []) {
    const actual = parseTypescriptStringUnion(enumTarget.typescriptFile, enumTarget.typescript);
    if (JSON.stringify(actual) !== JSON.stringify(enumTarget.values)) {
      failures.push(
        `${enumTarget.typescriptFile}: ${enumTarget.typescript} values ${JSON.stringify(actual)}, expected ${JSON.stringify(enumTarget.values)}`,
      );
    }
  }
}

if (failures.length > 0) {
  console.error("Desktop contract drift detected:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Desktop contracts in sync: ${generatedContracts.map(({ contract }) => contract.contract).join(", ")}.`);
