# GatePOST

GatePOST is a lightweight ZeroMcp companion that detects MCP schema drift after testing sign-off and exposes the result through a built-in status UI.

## What GatePOST Can Baseline

GatePOST now supports three baseline sources:

- `file_json`: compare a JSON contract file against an approved snapshot.
- `http_json`: fetch a JSON contract over plain HTTP and compare it against an approved snapshot.
- `mcp_stdio`: connect to a real MCP server over stdio, capture `initialize`, `tools/list`, and optional sampled `tools/call` outputs.

For MCP integrations, the approved baseline stores:

- negotiated `initialize` data;
- the current tool contract from `tools/list`;
- optional sampled `tools/call` results you define at sign-off.

## Run the App

Start the GatePOST service and UI:

```powershell
cargo run -- serve --addr 127.0.0.1:8080 --data-dir .gatepost
```

Then open [http://127.0.0.1:8080](http://127.0.0.1:8080).

## UI Workflow

The UI lets you:

- register a protected integration;
- edit an existing target definition when the live source changes;
- choose `File JSON`, `HTTP JSON`, or `MCP stdio` as the live source;
- approve a baseline when testing signs off;
- manually refresh/update baseline from the live source using `Ping baseline`;
- force a fresh drift check;
- inspect the captured baseline, current trust state, latest diff, and incident history.

If a drift check runs before an approved baseline exists, GatePOST now surfaces a clear baseline-not-found error in both API responses and the UI status card.

## HTTP Demo

If you have a schema endpoint such as `http://127.0.0.1:8081/schema`, use these values in the UI:

- `Source type`: `HTTP JSON`
- `HTTP schema URL`: your schema endpoint

GatePOST will fetch that JSON document at sign-off and on each subsequent check. This is a good fit for external providers that publish schemas or OpenAPI-style contracts over HTTP.

For MCP tool-catalog style HTTP endpoints, GatePOST now enforces a schema profile documented in `MCP_SCHEMA_SPEC.md`:

- payload must be a JSON object with `tools` array;
- each tool must include `name`, `description`, and `inputSchema` object;
- optional `toolCount` must match the array length when present.

## Local MCP Demo

A mock MCP server is included at [examples/mock_mcp_server.py](C:/Users/Matt.Anderson/OneDrive%20-%20Corpay/Documents/ZeroMcp.Gatepost/examples/mock_mcp_server.py).

Use these values in the UI:

- `Source type`: `MCP stdio`
- `MCP command`: `python`
- `MCP args`: `examples\mock_mcp_server.py`
- `Sample tools/call JSON`:

```json
[{"name":"search","arguments":{"q":"gatepost"}}]
```

When you approve the integration, GatePOST will capture the mock server's `initialize` response, tool list, and the sampled `search` tool call into the approved baseline.

## File-Based Demo

For a file-backed integration, use:

- live source: `examples\live-schema.json`
- then swap to: `examples\drifted-schema.json`

That lets you see schema drift without a running MCP server.

## CLI Commands

Create an approved snapshot directly from a JSON file:

```powershell
cargo run -- snapshot `
  --schema examples\live-schema.json `
  --out examples\approved-snapshot.json `
  --system "zero-suite" `
  --environment "test" `
  --server "external-mcp" `
  --approved-by "qa.signoff"
```

Check a JSON contract directly against an approved snapshot:

```powershell
cargo run -- check `
  --snapshot examples\approved-snapshot.json `
  --schema examples\live-schema.json
```

Exit codes:

- `0`: no drift detected
- `1`: input or service error
- `2`: drift detected

## State Storage

GatePOST writes app state to:

- `.gatepost\gatepost-state.json`
- `.gatepost\snapshots\<integration-id>-approved.json`

## Notes

The current `http_json` source supports plain `http://` URLs and expects a JSON response body.

The current live MCP support assumes newline-delimited JSON-RPC over stdio and uses the MCP `initialize`, `notifications/initialized`, `tools/list`, and optional `tools/call` flow.

## Next Steps

- add HTTPS and authenticated HTTP schema fetch support;
- add direct HTTP/SSE MCP transports where needed;
- add webhook and ZeroMcp-native alert channels;
- add manual override and retest workflow states;
- add auth if the UI will be used outside local trusted environments;
- move from polling to inline gateway enforcement for production use.
