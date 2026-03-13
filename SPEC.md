# GatePOST Product Specification

## 1. Overview

**GatePOST** is a ZeroMcp product that protects systems from unnoticed MCP schema drift.

It sits between a client system and an external MCP server and acts as a policy and verification layer. At testing sign-off, GatePOST captures a trusted snapshot of the MCP server schema. After sign-off, GatePOST continuously or eventfully compares the live schema against the approved snapshot. If drift is detected, GatePOST raises an alert and can optionally block production use until retesting is completed.

The product goal is simple:

**If an external MCP schema changes after testing sign-off, the system must know immediately and require retesting before trust is restored.**

## 2. Problem Statement

Teams integrating with external MCP servers often assume that once testing is complete, the interface remains stable. In practice, external providers may add, remove, rename, or alter tools, inputs, outputs, annotations, capabilities, or metadata without coordinated release management.

This creates several risks:

- Production behavior may diverge from tested behavior.
- Existing automations or agent workflows may fail unexpectedly.
- Safety, compliance, or approval assumptions may become invalid.
- Breakages may only be discovered after an incident.

Current testing sign-off usually certifies a moment in time. It does not enforce that the certified MCP schema remains unchanged.

GatePOST solves this by turning testing sign-off into an enforceable schema contract.

## 3. Product Vision

GatePOST becomes the trust gate for external MCP dependencies in the ZeroMcp ecosystem.

It enables teams to:

- approve a known-good MCP schema at a defined release milestone;
- detect any post-approval schema drift;
- alert the right people immediately;
- require retesting and re-approval before the changed schema is considered trusted again.

## 4. Goals

- Detect schema drift between an approved snapshot and the currently exposed MCP schema.
- Make drift visible quickly through alerts, status views, and audit records.
- Support a formal testing sign-off workflow tied to an approved snapshot.
- Provide configurable policy responses, from alert-only to hard block.
- Create an auditable chain of evidence showing what changed, when, and whether it was re-approved.
- Fit naturally into the ZeroMcp suite as a governance and reliability component.

## 5. Non-Goals

- GatePOST is not a full MCP server implementation.
- GatePOST is not a generic API gateway for all protocols.
- GatePOST does not validate business correctness of tool behavior beyond schema drift.
- GatePOST does not replace functional, security, or regression testing.
- GatePOST does not infer intent; it only compares approved and current contract representations.

## 6. Core Concept

GatePOST maintains a lifecycle for each protected MCP integration:

1. Connect to an external MCP server.
2. Discover and normalize its schema.
3. Capture and store a signed-off snapshot at testing completion.
4. Re-check the live schema on a defined schedule, on demand, or at connection time.
5. Compare live state to the approved snapshot.
6. If differences exist, classify the drift and trigger policy actions.
7. Keep the integration in a drifted or untrusted state until retesting and re-approval occur.

## 7. Users

Primary users:

- QA and test leads approving external MCP integrations
- Platform engineers operating ZeroMcp environments
- Governance, risk, and compliance stakeholders
- Product teams depending on third-party MCP servers

Secondary users:

- SRE and operations teams responding to alerts
- Developers integrating MCP-dependent workflows

## 8. Key Use Cases

### 8.1 Testing Sign-Off Snapshot

A QA lead completes testing against an external MCP server and instructs GatePOST to capture the approved schema snapshot for version X of an internal release.

### 8.2 Drift Detection During Runtime

An external MCP provider changes a tool definition after sign-off. GatePOST detects the change during a scheduled check or when traffic begins flowing and marks the integration as drifted.

### 8.3 Alert and Retest Enforcement

Once drift is detected, GatePOST sends an alert to configured channels and enforces the configured policy, such as warning-only, degraded mode, or hard block until re-approval.

### 8.4 Re-Approval After Retest

After retesting the changed MCP schema, an authorized user creates a new approved snapshot, clearing the drift condition.

### 8.5 Audit and Change Investigation

A platform or compliance user reviews the drift history, including what changed, when it changed, which policy fired, and when the integration was re-approved.

## 9. Functional Requirements

### 9.1 MCP Schema Discovery

GatePOST must:

- connect to a target MCP server using supported connection methods;
- retrieve the current schema and capabilities required for comparison;
- normalize schema data into a canonical internal representation to reduce false positives caused by ordering or formatting differences;
- support repeated polling or triggered re-discovery.

### 9.2 Snapshot Management

GatePOST must:

- allow an authorized user or workflow to create an approved snapshot;
- timestamp each snapshot;
- associate snapshots with environment, system, MCP endpoint, owner, and release/test metadata;
- store immutable historical snapshots;
- identify exactly one active approved snapshot per protected integration unless versioned policy explicitly allows otherwise.

### 9.3 Drift Detection

GatePOST must compare live schema against the active approved snapshot and detect at minimum:

- added tools;
- removed tools;
- renamed tools where detectable;
- changes to tool descriptions or metadata;
- changes to input schema;
- changes to output schema;
- changes to argument requirements, types, enums, defaults, or constraints;
- capability-level changes outside specific tools;
- permission or annotation changes if exposed by the MCP schema.

### 9.4 Drift Classification

GatePOST should classify detected changes to support prioritization. Example classes:

- `informational`: non-functional metadata differences
- `minor`: additive changes unlikely to break existing callers
- `major`: contract changes likely to require retesting
- `critical`: removals or incompatible changes with high runtime risk

Classification rules must be configurable because teams may treat additive changes as blocking.

### 9.5 Policy Enforcement

GatePOST must support configurable actions when drift is detected:

- log only;
- alert only;
- mark integration as untrusted;
- require manual override;
- block requests to the MCP server;
- allow read-only or reduced operation mode if applicable.

Policy should be configurable per integration and per environment.

### 9.6 Alerting

GatePOST must:

- generate an alert when new drift is detected;
- avoid duplicate alert storms for the same unchanged drift condition;
- include a concise summary of the change and impacted integration;
- support multiple alert targets such as webhook, email, or ZeroMcp-native notification channels;
- record acknowledgement state where relevant.

### 9.7 Approval Workflow

GatePOST must:

- support explicit sign-off by authorized roles;
- support storing approval notes and test evidence references;
- distinguish between approved, drifted, pending review, and blocked states;
- support creating a new approved baseline after retesting.

### 9.8 Audit Trail

GatePOST must persist:

- who approved each snapshot;
- when approval occurred;
- what schema was approved;
- when drift was first detected;
- what changed;
- which alerts were sent;
- what policy was enforced;
- when drift was cleared and by whom.

## 10. Non-Functional Requirements

### 10.1 Reliability

- Drift checks must be repeatable and deterministic for the same source schema.
- Temporary MCP connectivity issues must not be misclassified as schema drift.
- The product must distinguish `unreachable` from `changed`.

### 10.2 Performance

- Schema comparison should complete quickly enough to support scheduled monitoring and optional connection-time checks.
- Snapshot and diff operations should remain efficient for large tool catalogs.

### 10.3 Security

- Access to approve, override, or clear drift must be role-restricted.
- Stored snapshots and audit records must be protected from tampering.
- Credentials used to inspect MCP servers must be stored securely.

### 10.4 Observability

- GatePOST should expose status, last check time, last successful schema fetch, and current trust state.
- Operational logs must make it easy to distinguish fetch failures, normalization issues, policy decisions, and alert delivery outcomes.

### 10.5 Extensibility

- The comparison engine should support future rule packs for custom drift detection logic.
- Notification and policy backends should be pluggable.

## 11. Suggested Domain Model

Core entities:

- `ProtectedIntegration`
- `SchemaSnapshot`
- `SchemaDiff`
- `DriftIncident`
- `ApprovalRecord`
- `PolicyRule`
- `AlertEvent`

Example state model for a protected integration:

- `Uninitialized`
- `Approved`
- `DriftDetected`
- `PendingRetest`
- `Blocked`
- `Overridden`

## 12. Example Workflow

### Happy Path

1. Team configures an external MCP server in GatePOST.
2. GatePOST discovers and displays the current schema.
3. Team completes testing.
4. Authorized approver creates the sign-off snapshot.
5. GatePOST marks the integration as `Approved`.
6. GatePOST continues periodic schema checks.

### Drift Path

1. External MCP server changes after sign-off.
2. GatePOST detects a difference from the approved snapshot.
3. GatePOST classifies the drift.
4. GatePOST creates a drift incident and sends alerts.
5. Configured policy marks the integration `DriftDetected` or `Blocked`.
6. Team retests against the new schema.
7. Approver captures a new snapshot.
8. GatePOST closes the drift incident and restores `Approved`.

## 13. MVP Scope

The MVP should include:

- one protected MCP integration definition;
- schema discovery and normalization;
- manual sign-off snapshot creation;
- scheduled drift checks;
- diff reporting;
- alert generation;
- basic policy modes: `alert-only` and `block`;
- audit history;
- simple UI or API to review status and approve a new baseline.

The MVP may defer:

- advanced rename detection;
- rich workflow orchestration;
- multi-stage approval chains;
- partial compatibility scoring;
- deep analytics dashboards;
- automatic retest orchestration.

## 14. Acceptance Criteria

- A user can register an external MCP server for protection.
- A user can create an approved snapshot at testing sign-off.
- GatePOST can re-fetch the live schema and compare it to the snapshot.
- If the schema changes, GatePOST creates a visible drift event.
- The drift event clearly identifies what changed.
- GatePOST enforces the configured response for that integration.
- The system preserves an audit trail of snapshot, drift, alert, and re-approval activity.
- A user can create a replacement approved snapshot after retesting.

## 15. Risks and Open Questions

- How should GatePOST define the canonical schema representation across different MCP server implementations?
- Which schema elements are contract-critical versus informational?
- Should additive tool changes always require retesting, or only incompatible changes?
- Should GatePOST sit inline in the request path, or operate as an out-of-band monitor with advisory/blocking hooks?
- What is the expected source of truth for testing sign-off metadata?
- How will authorized overrides be governed and expired?
- Does the ZeroMcp suite already provide shared notification, auth, and audit components that GatePOST should reuse?

## 16. Recommended Positioning

Suggested one-line positioning:

**GatePOST is ZeroMcp's schema drift safety gate for external MCP integrations.**

Suggested short description:

**GatePOST captures an approved MCP schema at testing sign-off and alerts or blocks when the external schema drifts, ensuring retesting happens before trust is restored.**
