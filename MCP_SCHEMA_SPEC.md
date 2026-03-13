# MCP Tool Catalog Schema Profile (GatePOST)

This document defines the schema profile GatePOST enforces for `http_json` integration sources.

## Purpose

GatePOST uses this profile to ensure that a captured baseline is a real MCP tool catalog contract, not a general server info payload.

## Required Top-Level Shape

The HTTP payload must be a JSON object and must include:

- `tools`: array of tool definitions

Optional but validated when present:

- `toolCount`: integer, must equal `tools.length`

## Required Fields Per Tool

Each entry in `tools` must be an object containing:

- `name`: non-empty string, unique across the array
- `description`: string
- `inputSchema`: JSON object

## Validation Outcomes

- If payload matches this profile: baseline capture/check proceeds.
- If payload fails: GatePOST returns a validation error and does not update baseline.

## Notes

- This is a practical MCP contract profile for drift safety.
- It does not attempt to fully model every possible MCP server extension field.
