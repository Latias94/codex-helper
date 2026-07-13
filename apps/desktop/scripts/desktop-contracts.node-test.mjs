import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDesktopContracts,
  parseTypescriptObjectShapeFromSource,
  parseTypescriptStringUnionFromSource,
  typescriptObjectShapeFailures,
} from "./desktop-contracts.mjs";

test("TypeScript AST parser ignores forged declarations and preserves multiline shape", () => {
  const source = `
    // export type Wire = { forged: string };
    export type Wire = {
      readonly request_id: number;
      nested?: Array<
        ApiNested
      >;
      required_nullable: string |
        null;
    };
  `;

  assert.deepEqual(parseTypescriptObjectShapeFromSource(source, "Wire"), [
    { name: "request_id", optional: false, type: "number" },
    { name: "nested", optional: true, type: "ApiNested[]" },
    { name: "required_nullable", optional: false, type: "null|string" },
  ]);
});

test("TypeScript enum parser rejects non-string union members", () => {
  const source = `export type Status = "ready" | number | "stale";`;

  assert.throws(
    () => parseTypescriptStringUnionFromSource(source, "Status"),
    /only string literal members/,
  );
});

test("TypeScript object parser fails closed on aliases instead of guessing fields", () => {
  const source = `
    type Base = { status: "ready" };
    export type Wire = Base & { data: string };
  `;

  assert.throws(
    () => parseTypescriptObjectShapeFromSource(source, "Wire"),
    /direct object type literal/,
  );
});

test("request-chain payload contract rejects optionality and type mutations", () => {
  const requestChainContract = buildDesktopContracts().find(
    ({ contract }) => contract.contract === "codex-helper-request-chain/v1",
  )?.contract;
  const expectedShape = requestChainContract?.typescript.find(
    ({ type }) => type === "RequestChainPayload",
  )?.shape;
  assert.ok(expectedShape, "request-chain payload must carry a strict Rust-derived shape");

  const optionalityMutation = parseTypescriptObjectShapeFromSource(
    `export type RequestChainPayload = {
      traceId: string;
      requestId?: number;
      session?: string;
      limit?: number;
    };`,
    "RequestChainPayload",
  );
  assert.deepEqual(
    typescriptObjectShapeFailures(
      optionalityMutation,
      expectedShape,
      "RequestChainPayload",
    ),
    ["RequestChainPayload.traceId optional=false, expected true"],
  );

  const typeMutation = parseTypescriptObjectShapeFromSource(
    `export type RequestChainPayload = {
      traceId?: string;
      requestId?: string;
      session?: string;
      limit?: number;
    };`,
    "RequestChainPayload",
  );
  assert.deepEqual(
    typescriptObjectShapeFailures(typeMutation, expectedShape, "RequestChainPayload"),
    ["RequestChainPayload.requestId type string, expected number"],
  );
});
