#!/usr/bin/env node

import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

export async function startCredentialSmokeUpstream({
  credentials,
  host = "127.0.0.1",
  onRecord = () => {},
  captureRecords = true,
}) {
  validateCredentials(credentials);
  const generationsByAuthorization = new Map([
    [`Bearer ${credentials.old}`, "old"],
    [`Bearer ${credentials.new}`, "new"],
  ]);
  const records = [];
  let connections = 0;
  let requests = 0;
  const server = http.createServer((request, response) => {
    const authorization = request.headers.authorization;
    const generation = generationsByAuthorization.get(authorization) ?? "unknown";
    const probeHeader = request.headers["x-codex-helper-smoke-probe"];
    const record = {
      method: request.method,
      path: request.url,
      generation,
      probe_id: typeof probeHeader === "string" ? probeHeader : null,
    };
    requests += 1;
    if (captureRecords) {
      records.push(record);
    }
    onRecord(record);

    request.resume();
    request.once("end", () => {
      if (generation === "unknown") {
        response.writeHead(401, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: { message: "credential generation rejected" } }));
        return;
      }
      response.writeHead(200, { "content-type": "application/json" });
      if (request.url?.split("?", 1)[0].endsWith("/models")) {
        response.end(JSON.stringify({ object: "list", data: [] }));
        return;
      }
      response.end(
        JSON.stringify({
          id: "resp_credential_smoke",
          object: "response",
          status: "completed",
          model: "gpt-5",
          output: [],
          usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
        }),
      );
    });
  });
  server.on("connection", () => {
    connections += 1;
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen({ host, port: 0, exclusive: true }, resolve);
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    await closeServer(server);
    throw new Error("credential smoke upstream did not expose a TCP port");
  }
  return {
    port: address.port,
    connectionCount: () => connections,
    requestCount: () => requests,
    records: () => records.map((record) => ({ ...record })),
    close: () => closeServer(server),
  };
}

async function runCli() {
  const options = parseArguments(process.argv.slice(2));
  const credentials = JSON.parse(fs.readFileSync(options.credentials, "utf8"));
  const upstream = await startCredentialSmokeUpstream({
    credentials,
    host: "0.0.0.0",
    captureRecords: false,
    onRecord: (record) => {
      fs.appendFileSync(options.records, `${JSON.stringify(record)}\n`, { mode: 0o600 });
    },
  });
  fs.writeFileSync(options.ready, `${JSON.stringify({ port: upstream.port })}\n`, {
    mode: 0o600,
    flag: "wx",
  });
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => {
      void upstream.close().then(() => process.exit(0));
    });
  }
}

function validateCredentials(credentials) {
  if (
    typeof credentials?.old !== "string" ||
    typeof credentials?.new !== "string" ||
    credentials.old.length === 0 ||
    credentials.new.length === 0 ||
    credentials.old === credentials.new
  ) {
    throw new Error("credential fixture must contain distinct, non-empty old and new values");
  }
}

function closeServer(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
    server.closeAllConnections?.();
  });
}

function parseArguments(args) {
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!["--credentials", "--records", "--ready"].includes(flag) || !value) {
      throw new Error(
        "usage: docker-smoke-upstream.mjs --credentials PATH --records PATH --ready PATH",
      );
    }
    values.set(flag, value);
  }
  if (values.size !== 3) {
    throw new Error(
      "usage: docker-smoke-upstream.mjs --credentials PATH --records PATH --ready PATH",
    );
  }
  return {
    credentials: values.get("--credentials"),
    records: values.get("--records"),
    ready: values.get("--ready"),
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : null;
if (invokedPath === import.meta.url) {
  await runCli();
}
