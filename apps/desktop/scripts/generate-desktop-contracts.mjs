import fs from "node:fs";
import path from "node:path";
import {
  buildDesktopContracts,
  contractOutputPath,
  formatContractJson,
} from "./desktop-contracts.mjs";

for (const { output, contract } of buildDesktopContracts()) {
  const outputPath = contractOutputPath(output);
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, formatContractJson(contract));
}

console.log("Generated desktop contracts.");
