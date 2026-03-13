use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriftSeverity {
    Informational,
    Minor,
    Major,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    Uninitialized,
    Approved,
    DriftDetected,
    Blocked,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    AlertOnly,
    Block,
}

impl Default for PolicyMode {
    fn default() -> Self {
        Self::AlertOnly
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    FileJson,
    HttpJson,
    McpStdio,

}
impl Default for SourceKind {
    fn default() -> Self {
        Self::FileJson
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallSample {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriftEntry {
    pub path: String,
    pub kind: DriftKind,
    pub severity: DriftSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub system: String,
    pub environment: String,
    pub server: String,
    pub approved_by: String,
    pub approved_at_epoch_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    pub metadata: SnapshotMetadata,
    pub schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftReport {
    pub drift_detected: bool,
    pub entries: Vec<DriftEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentRecord {
    pub opened_at_epoch_ms: u128,
    pub summary: String,
    pub entries: Vec<DriftEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationStatus {
    pub trust_state: TrustState,
    pub drift_detected: bool,
    pub last_checked_epoch_ms: Option<u128>,
    pub summary: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedIntegration {
    pub id: String,
    pub name: String,
    pub system: String,
    pub environment: String,
    pub server: String,
    #[serde(default)]
    pub source_kind: SourceKind,
    pub live_schema_path: Option<String>,
    pub http_url: Option<String>,
    pub mcp_command: Option<String>,
    #[serde(default)]
    pub mcp_args: Vec<String>,
    #[serde(default)]
    pub sample_tool_calls: Vec<ToolCallSample>,
    pub snapshot_path: String,
    #[serde(default)]
    pub policy_mode: PolicyMode,
    pub check_interval_seconds: u64,
    pub approved_snapshot: Option<SchemaSnapshot>,
    pub last_report: Option<DriftReport>,
    pub incidents: Vec<IncidentRecord>,
    pub status: IntegrationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GatePostStore {
    pub integrations: Vec<ManagedIntegration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIntegrationRequest {
    pub name: String,
    pub system: String,
    pub environment: String,
    pub server: String,
    #[serde(default)]
    pub source_kind: SourceKind,
    pub live_schema_path: Option<String>,
    pub http_url: Option<String>,
    pub mcp_command: Option<String>,
    #[serde(default)]
    pub mcp_args: Vec<String>,
    #[serde(default)]
    pub sample_tool_calls: Vec<ToolCallSample>,
    #[serde(default)]
    pub policy_mode: PolicyMode,
    pub check_interval_seconds: u64,
}

pub type UpdateIntegrationRequest = CreateIntegrationRequest;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approved_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub generated_at_epoch_ms: u128,
    pub integrations: Vec<ManagedIntegration>,
}

struct StdioMcpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl DriftReport {
    pub fn summary(&self) -> String {
        if self.entries.is_empty() {
            return "No schema drift detected.".to_string();
        }

        let critical = self
            .entries
            .iter()
            .filter(|entry| entry.severity == DriftSeverity::Critical)
            .count();
        let major = self
            .entries
            .iter()
            .filter(|entry| entry.severity == DriftSeverity::Major)
            .count();
        let minor = self
            .entries
            .iter()
            .filter(|entry| entry.severity == DriftSeverity::Minor)
            .count();
        let informational = self
            .entries
            .iter()
            .filter(|entry| entry.severity == DriftSeverity::Informational)
            .count();

        format!(
            "Detected {} drift change(s): critical={}, major={}, minor={}, informational={}",
            self.entries.len(), critical, major, minor, informational
        )
    }
}

impl ManagedIntegration {
    pub fn should_check(&self, now_epoch_ms: u128) -> bool {
        match self.status.last_checked_epoch_ms {
            Some(last_checked) => {
                now_epoch_ms.saturating_sub(last_checked)
                    >= (self.check_interval_seconds as u128 * 1000)
            }
            None => true,
        }
    }

    pub fn source_summary(&self) -> String {
        match self.source_kind {
            SourceKind::FileJson => self
                .live_schema_path
                .clone()
                .unwrap_or_else(|| "No file path configured".to_string()),
            SourceKind::HttpJson => self
                .http_url
                .clone()
                .unwrap_or_else(|| "No HTTP URL configured".to_string()),
            SourceKind::McpStdio => {
                let command = self
                    .mcp_command
                    .clone()
                    .unwrap_or_else(|| "<missing command>".to_string());
                if self.mcp_args.is_empty() {
                    command
                } else {
                    format!("{} {}", command, self.mcp_args.join(" "))
                }
            }
        }
    }
}

impl StdioMcpSession {
    fn connect(command: &str, args: &[String]) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Failed to spawn MCP server '{command}': {error}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open MCP stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open MCP stdout".to_string())?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let payload = match params {
            Some(params) => json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
            None => json!({
                "jsonrpc": "2.0",
                "method": method,
            }),
        };
        self.write_message(&payload)
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&payload)?;

        loop {
            let message = self.read_message()?;
            let Some(message_id) = message.get("id") else {
                continue;
            };
            if message_id != &json!(id) {
                continue;
            }

            if let Some(error) = message.get("error") {
                return Err(format!(
                    "MCP server returned an error for {method}: {}",
                    error
                ));
            }

            return message
                .get("result")
                .cloned()
                .ok_or_else(|| format!("MCP response for {method} did not include a result"));
        }
    }

    fn write_message(&mut self, payload: &Value) -> Result<(), String> {
        let line = serde_json::to_string(payload)
            .map_err(|error| format!("Failed to serialize MCP request: {error}"))?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|error| format!("Failed to write to MCP stdin: {error}"))
    }

    fn read_message(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self
                .stdout
                .read_line(&mut line)
                .map_err(|error| format!("Failed to read from MCP stdout: {error}"))?;
            if bytes == 0 {
                return Err("MCP server closed stdout before responding".to_string());
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            return serde_json::from_str(trimmed)
                .map_err(|error| format!("Failed to parse MCP JSON-RPC message: {error}"));
        }
    }
}

impl Drop for StdioMcpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn now_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub fn load_json_file(path: &Path) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse JSON in {}: {error}", path.display()))
}

pub fn load_snapshot(path: &Path) -> Result<SchemaSnapshot, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse snapshot {}: {error}", path.display()))
}

pub fn write_snapshot(path: &Path, snapshot: &SchemaSnapshot) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize snapshot: {error}"))?;
    fs::write(path, json).map_err(|error| format!("Failed to write {}: {error}", path.display()))
}
pub fn create_snapshot(
    schema: Value,
    system: String,
    environment: String,
    server: String,
    approved_by: String,
) -> SchemaSnapshot {
    SchemaSnapshot {
        metadata: SnapshotMetadata {
            system,
            environment,
            server,
            approved_by,
            approved_at_epoch_ms: now_epoch_ms(),
        },
        schema: canonicalize_value(schema),
    }
}

pub fn detect_drift(snapshot_schema: &Value, live_schema: &Value) -> DriftReport {
    let mut entries = Vec::new();
    diff_value(
        "$schema",
        snapshot_schema,
        &canonicalize_value(live_schema.clone()),
        &mut entries,
    );

    DriftReport {
        drift_detected: !entries.is_empty(),
        entries,
    }
}

pub fn load_store(path: &Path) -> Result<GatePostStore, String> {
    if !path.exists() {
        return Ok(GatePostStore::default());
    }

    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse state file {}: {error}", path.display()))
}

pub fn save_store(path: &Path, store: &GatePostStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Failed to serialize GatePOST state: {error}"))?;
    fs::write(path, json).map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

pub fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("gatepost-state.json")
}

pub fn snapshot_path_for(data_dir: &Path, integration_id: &str) -> PathBuf {
    data_dir
        .join("snapshots")
        .join(format!("{integration_id}-approved.json"))
}

pub fn add_integration(
    store: &mut GatePostStore,
    data_dir: &Path,
    request: CreateIntegrationRequest,
) -> Result<ManagedIntegration, String> {
    let request = normalize_request(request);
    let id = slugify(&request.name);
    if store.integrations.iter().any(|integration| integration.id == id) {
        return Err(format!("Integration '{}' already exists.", request.name));
    }

    validate_source_config(&request)?;

    let snapshot_path = snapshot_path_for(data_dir, &id);
    let integration = ManagedIntegration {
        id,
        name: request.name,
        system: request.system,
        environment: request.environment,
        server: request.server,
        source_kind: request.source_kind,
        live_schema_path: request.live_schema_path,
        http_url: request.http_url,
        mcp_command: request.mcp_command,
        mcp_args: request.mcp_args,
        sample_tool_calls: request.sample_tool_calls,
        snapshot_path: snapshot_path.to_string_lossy().to_string(),
        policy_mode: request.policy_mode,
        check_interval_seconds: request.check_interval_seconds.max(5),
        approved_snapshot: None,
        last_report: None,
        incidents: Vec::new(),
        status: IntegrationStatus {
            trust_state: TrustState::Uninitialized,
            drift_detected: false,
            last_checked_epoch_ms: None,
            summary: "Awaiting first approval snapshot.".to_string(),
            error_message: None,
        },
    };

    store.integrations.push(integration.clone());
    Ok(integration)
}

pub fn approve_integration(
    store: &mut GatePostStore,
    integration_id: &str,
    approved_by: &str,
) -> Result<ManagedIntegration, String> {
    let integration = find_integration_mut(store, integration_id)?;
    let schema = match fetch_live_schema(integration) {
        Ok(schema) => schema,
        Err(error) => {
            integration.status = IntegrationStatus {
                trust_state: TrustState::Error,
                drift_detected: false,
                last_checked_epoch_ms: Some(now_epoch_ms()),
                summary: "GatePOST could not capture the current MCP contract for approval.".to_string(),
                error_message: Some(error.clone()),
            };
            return Err(error);
        }
    };
    let snapshot = create_snapshot(
        schema,
        integration.system.clone(),
        integration.environment.clone(),
        integration.server.clone(),
        approved_by.to_string(),
    );

    if let Err(error) = write_snapshot(Path::new(&integration.snapshot_path), &snapshot) {
        integration.status = IntegrationStatus {
            trust_state: TrustState::Error,
            drift_detected: false,
            last_checked_epoch_ms: Some(now_epoch_ms()),
            summary: "GatePOST failed to write the approved baseline snapshot.".to_string(),
            error_message: Some(error.clone()),
        };
        return Err(error);
    }
    integration.approved_snapshot = Some(snapshot);
    integration.last_report = None;
    integration.incidents.clear();
    integration.status = IntegrationStatus {
        trust_state: TrustState::Approved,
        drift_detected: false,
        last_checked_epoch_ms: Some(now_epoch_ms()),
        summary: format!("Approved by {approved_by}."),
        error_message: None,
    };

    Ok(integration.clone())
}

pub fn check_integration(
    store: &mut GatePostStore,
    integration_id: &str,
) -> Result<ManagedIntegration, String> {
    let integration = find_integration_mut(store, integration_id)?;
    run_check_on_integration(integration)?;
    Ok(integration.clone())
}

pub fn update_integration(
    store: &mut GatePostStore,
    integration_id: &str,
    request: UpdateIntegrationRequest,
) -> Result<ManagedIntegration, String> {
    let request = normalize_request(request);
    validate_source_config(&request)?;
    let integration = find_integration_mut(store, integration_id)?;

    integration.name = request.name;
    integration.system = request.system;
    integration.environment = request.environment;
    integration.server = request.server;
    integration.source_kind = request.source_kind;
    integration.live_schema_path = request.live_schema_path;
    integration.http_url = request.http_url;
    integration.mcp_command = request.mcp_command;
    integration.mcp_args = request.mcp_args;
    integration.sample_tool_calls = request.sample_tool_calls;
    integration.policy_mode = request.policy_mode;
    integration.check_interval_seconds = request.check_interval_seconds.max(5);

    // Changing target/source details invalidates the previous baseline.
    integration.approved_snapshot = None;
    integration.last_report = None;
    integration.incidents.clear();
    integration.status = IntegrationStatus {
        trust_state: TrustState::Uninitialized,
        drift_detected: false,
        last_checked_epoch_ms: Some(now_epoch_ms()),
        summary: "Integration target updated. Approve a new baseline snapshot.".to_string(),
        error_message: None,
    };

    Ok(integration.clone())
}

pub fn ping_baseline(
    store: &mut GatePostStore,
    integration_id: &str,
) -> Result<ManagedIntegration, String> {
    let integration = find_integration_mut(store, integration_id)?;
    integration.status.last_checked_epoch_ms = Some(now_epoch_ms());

    let schema = match fetch_live_schema(integration) {
        Ok(schema) => schema,
        Err(error) => {
            integration.status = IntegrationStatus {
                trust_state: TrustState::Error,
                drift_detected: false,
                last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
                summary: "Baseline ping failed.".to_string(),
                error_message: Some(error.clone()),
            };
            return Err(error);
        }
    };

    let approved_by = integration
        .approved_snapshot
        .as_ref()
        .map(|snapshot| snapshot.metadata.approved_by.clone())
        .or_else(|| {
            load_snapshot(Path::new(&integration.snapshot_path))
                .ok()
                .map(|snapshot| snapshot.metadata.approved_by)
        })
        .unwrap_or_else(|| "manual.baseline.ping".to_string());

    let snapshot = create_snapshot(
        schema,
        integration.system.clone(),
        integration.environment.clone(),
        integration.server.clone(),
        approved_by,
    );
    if let Err(error) = write_snapshot(Path::new(&integration.snapshot_path), &snapshot) {
        integration.status = IntegrationStatus {
            trust_state: TrustState::Error,
            drift_detected: false,
            last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
            summary: "Baseline ping failed while writing snapshot.".to_string(),
            error_message: Some(error.clone()),
        };
        return Err(error);
    }

    integration.approved_snapshot = Some(snapshot);
    integration.last_report = None;
    integration.incidents.clear();
    integration.status = IntegrationStatus {
        trust_state: TrustState::Approved,
        drift_detected: false,
        last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
        summary: "Baseline updated from live schema via Ping baseline.".to_string(),
        error_message: None,
    };
    Ok(integration.clone())
}

pub fn check_due_integrations(store: &mut GatePostStore) {
    let now = now_epoch_ms();
    for integration in &mut store.integrations {
        if integration.should_check(now) {
            let _ = run_check_on_integration(integration);
        }
    }
}

pub fn dashboard_summary(store: &GatePostStore) -> DashboardSummary {
    DashboardSummary {
        generated_at_epoch_ms: now_epoch_ms(),
        integrations: store.integrations.clone(),
    }
}

fn run_check_on_integration(integration: &mut ManagedIntegration) -> Result<(), String> {
    integration.status.last_checked_epoch_ms = Some(now_epoch_ms());

    let snapshot = integration
        .approved_snapshot
        .clone()
        .or_else(|| load_snapshot(Path::new(&integration.snapshot_path)).ok());

    let Some(snapshot) = snapshot else {
        let message = baseline_not_found_message(integration);
        integration.status = IntegrationStatus {
            trust_state: TrustState::Error,
            drift_detected: false,
            last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
            summary: "No approved baseline snapshot exists for this integration.".to_string(),
            error_message: Some(message.clone()),
        };
        return Err(message);
    };

    integration.approved_snapshot = Some(snapshot.clone());

    let live_schema = match fetch_live_schema(integration) {
        Ok(schema) => schema,
        Err(error) => {
            integration.status = IntegrationStatus {
                trust_state: TrustState::Error,
                drift_detected: false,
                last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
                summary: "GatePOST could not capture the current MCP contract.".to_string(),
                error_message: Some(error.clone()),
            };
            return Err(error);
        }
    };

    let report = detect_drift(&snapshot.schema, &live_schema);
    let summary = report.summary();

    if report.drift_detected {
        let trust_state = match integration.policy_mode {
            PolicyMode::AlertOnly => TrustState::DriftDetected,
            PolicyMode::Block => TrustState::Blocked,
        };

        if should_open_incident(integration, &report) {
            integration.incidents.push(IncidentRecord {
                opened_at_epoch_ms: now_epoch_ms(),
                summary: summary.clone(),
                entries: report.entries.clone(),
            });
        }

        integration.status = IntegrationStatus {
            trust_state,
            drift_detected: true,
            last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
            summary,
            error_message: None,
        };
    } else {
        integration.status = IntegrationStatus {
            trust_state: TrustState::Approved,
            drift_detected: false,
            last_checked_epoch_ms: integration.status.last_checked_epoch_ms,
            summary,
            error_message: None,
        };
    }

    integration.last_report = Some(report);
    Ok(())
}

fn baseline_not_found_message(integration: &ManagedIntegration) -> String {
    format!(
        "Baseline not found. Expected snapshot at {}. Approve a snapshot before running checks.",
        integration.snapshot_path
    )
}

fn fetch_live_schema(integration: &ManagedIntegration) -> Result<Value, String> {
    match integration.source_kind {
        SourceKind::FileJson => {
            let path = integration
                .live_schema_path
                .as_deref()
                .ok_or_else(|| "This integration is missing a live schema path.".to_string())?;
            load_json_file(Path::new(path))
        }
        SourceKind::HttpJson => {
            let url = integration
                .http_url
                .as_deref()
                .ok_or_else(|| "This integration is missing an HTTP schema URL.".to_string())?;
            load_json_from_http(url)
        }
        SourceKind::McpStdio => {
            let command = integration
                .mcp_command
                .as_deref()
                .ok_or_else(|| "This integration is missing an MCP command.".to_string())?;
            capture_mcp_stdio_contract(command, &integration.mcp_args, &integration.sample_tool_calls)
        }
    }
}

fn load_json_from_http(url: &str) -> Result<Value, String> {
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|error| format!("Failed to connect to {host}:{port}: {error}"))?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
        path, host
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("Failed to write HTTP request: {error}"))?;

    let mut response = String::new();
    let mut reader = BufReader::new(stream);
    std::io::Read::read_to_string(&mut reader, &mut response)
        .map_err(|error| format!("Failed to read HTTP response: {error}"))?;

    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "HTTP response was malformed.".to_string())?;
    let status_line = headers.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") {
        return Err(format!("HTTP schema endpoint returned non-200 status: {status_line}"));
    }

    let body = decode_http_body(headers, body)?;

    let parsed: Value = serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse JSON from HTTP schema endpoint: {error}"))?;
    validate_mcp_tool_catalog_schema(&parsed)?;
    Ok(parsed)
}

fn decode_http_body(headers: &str, body: &str) -> Result<String, String> {
    let is_chunked = headers.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });

    if is_chunked {
        return decode_chunked_body(body);
    }

    Ok(body.to_string())
}

fn decode_chunked_body(body: &str) -> Result<String, String> {
    let mut remaining = body;
    let mut decoded = String::new();

    loop {
        let Some((size_line, after_size_line)) = remaining.split_once("\r\n") else {
            return Err("Chunked HTTP response is malformed (missing chunk size line).".to_string());
        };
        let size_token = size_line
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|error| format!("Invalid chunk size '{size_token}': {error}"))?;

        if size == 0 {
            break;
        }

        if after_size_line.len() < size + 2 {
            return Err("Chunked HTTP response ended before chunk data completed.".to_string());
        }

        let (chunk_data, after_chunk_data) = after_size_line.split_at(size);
        decoded.push_str(chunk_data);

        if !after_chunk_data.starts_with("\r\n") {
            return Err("Chunked HTTP response is malformed (missing chunk terminator).".to_string());
        }

        remaining = &after_chunk_data[2..];
    }

    Ok(decoded)
}

fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let sanitized = sanitize_schema_url(url);
    let without_scheme = sanitized
        .strip_prefix("http://")
        .ok_or_else(|| "Only plain http:// schema URLs are supported right now.".to_string())?;
    let (host_port, path_part) = match without_scheme.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{path}")),
        None => (without_scheme, "/".to_string()),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|error| format!("Invalid HTTP port in schema URL: {error}"))?;
            (host.to_string(), port)
        }
        None => (host_port.to_string(), 80_u16),
    };

    if host.trim().is_empty() {
        return Err("HTTP schema URL is missing a host.".to_string());
    }

    Ok((host, port, path_part))
}

fn sanitize_schema_url(url: &str) -> String {
    let compact: String = url.chars().filter(|character| !character.is_whitespace()).collect();
    if let Some(rest) = compact.strip_prefix("http://") {
        return format!("http://{}", rest.trim_start_matches('/'));
    }
    compact
}

fn validate_mcp_tool_catalog_schema(payload: &Value) -> Result<(), String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "HTTP schema payload must be a JSON object.".to_string())?;

    let tools_value = object
        .get("tools")
        .ok_or_else(|| "HTTP schema payload must include a 'tools' array.".to_string())?;
    let tools = tools_value
        .as_array()
        .ok_or_else(|| "HTTP schema payload field 'tools' must be an array.".to_string())?;

    if let Some(tool_count) = object.get("toolCount").and_then(Value::as_u64) {
        let actual = tools.len() as u64;
        if tool_count != actual {
            return Err(format!(
                "HTTP schema payload 'toolCount' ({tool_count}) does not match tools length ({actual})."
            ));
        }
    }

    let mut names = std::collections::BTreeSet::new();
    for (index, tool) in tools.iter().enumerate() {
        let tool_obj = tool
            .as_object()
            .ok_or_else(|| format!("HTTP schema tools[{index}] must be an object."))?;

        let name = tool_obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("HTTP schema tools[{index}] is missing a non-empty 'name'."))?;
        if !names.insert(name.to_string()) {
            return Err(format!("HTTP schema contains duplicate tool name '{name}'."));
        }

        let _description = tool_obj
            .get("description")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("HTTP schema tools[{index}] is missing 'description'."))?;

        let input_schema = tool_obj
            .get("inputSchema")
            .ok_or_else(|| format!("HTTP schema tools[{index}] is missing 'inputSchema'."))?;
        if !input_schema.is_object() {
            return Err(format!(
                "HTTP schema tools[{index}] field 'inputSchema' must be an object."
            ));
        }
    }

    Ok(())
}
fn capture_mcp_stdio_contract(
    command: &str,
    args: &[String],
    sample_tool_calls: &[ToolCallSample],
) -> Result<Value, String> {
    let mut session = StdioMcpSession::connect(command, args)?;

    let initialize_result = session.request(
        1,
        "initialize",
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {
                "roots": { "listChanged": false },
                "sampling": {},
            },
            "clientInfo": {
                "name": "gatepost",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )?;

    session.notify("notifications/initialized", None)?;

    let tools = fetch_all_tools(&mut session)?;
    let tool_call_samples = run_tool_call_samples(&mut session, sample_tool_calls)?;

    Ok(canonicalize_value(json!({
        "baselineType": "mcp_stdio_contract",
        "initialize": initialize_result,
        "tools": tools,
        "toolCallSamples": tool_call_samples,
    })))
}
fn fetch_all_tools(session: &mut StdioMcpSession) -> Result<Vec<Value>, String> {
    let mut id = 10_u64;
    let mut cursor: Option<String> = None;
    let mut tools = Vec::new();

    loop {
        let params = match &cursor {
            Some(cursor) => json!({ "cursor": cursor }),
            None => json!({}),
        };
        let result = session.request(id, "tools/list", params)?;
        id += 1;

        let page_tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        tools.extend(page_tools);

        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                result
                    .get("next_cursor")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            });

        if cursor.is_none() {
            break;
        }
    }

    tools.sort_by(|left, right| {
        let left_name = left.get("name").and_then(Value::as_str).unwrap_or_default();
        let right_name = right.get("name").and_then(Value::as_str).unwrap_or_default();
        left_name.cmp(right_name)
    });

    Ok(tools)
}

fn run_tool_call_samples(
    session: &mut StdioMcpSession,
    sample_tool_calls: &[ToolCallSample],
) -> Result<Vec<Value>, String> {
    let mut samples = Vec::new();
    let mut id = 100_u64;

    for sample in sample_tool_calls {
        let result = session.request(
            id,
            "tools/call",
            json!({
                "name": sample.name,
                "arguments": sample.arguments,
            }),
        )?;
        id += 1;

        samples.push(json!({
            "name": sample.name,
            "arguments": sample.arguments,
            "result": result,
        }));
    }

    Ok(samples)
}

fn validate_source_config(request: &CreateIntegrationRequest) -> Result<(), String> {
    match request.source_kind {
        SourceKind::FileJson => {
            let has_path = request
                .live_schema_path
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty());
            if !has_path {
                return Err("File-json integrations require a live schema path.".to_string());
            }
        }
        SourceKind::HttpJson => {
            let has_url = request
                .http_url
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty());
            if !has_url {
                return Err("Http-json integrations require an HTTP schema URL.".to_string());
            }
        }
        SourceKind::McpStdio => {
            let has_command = request
                .mcp_command
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty());
            if !has_command {
                return Err("MCP-stdio integrations require an MCP command.".to_string());
            }
        }
    }

    Ok(())
}

fn normalize_request(mut request: CreateIntegrationRequest) -> CreateIntegrationRequest {
    request.name = request.name.trim().to_string();
    request.system = request.system.trim().to_string();
    request.environment = request.environment.trim().to_string();
    request.server = request.server.trim().to_string();
    request.live_schema_path = normalize_optional_string(request.live_schema_path);
    request.http_url = normalize_optional_string(request.http_url);
    request.mcp_command = normalize_optional_string(request.mcp_command);
    request.mcp_args = request
        .mcp_args
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    request
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|inner| {
        let trimmed = inner.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn should_open_incident(integration: &ManagedIntegration, report: &DriftReport) -> bool {
    match &integration.last_report {
        Some(previous) => previous.entries != report.entries,
        None => true,
    }
}

fn find_integration_mut<'a>(
    store: &'a mut GatePostStore,
    integration_id: &str,
) -> Result<&'a mut ManagedIntegration, String> {
    store.integrations
        .iter_mut()
        .find(|integration| integration.id == integration_id)
        .ok_or_else(|| format!("Integration '{integration_id}' was not found."))
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut ordered = Map::new();
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                let nested = map.get(&key).cloned().unwrap_or(Value::Null);
                ordered.insert(key, canonicalize_value(nested));
            }
            Value::Object(ordered)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_value).collect()),
        other => other,
    }
}

fn diff_value(path: &str, approved: &Value, live: &Value, entries: &mut Vec<DriftEntry>) {
    match (approved, live) {
        (Value::Object(approved_map), Value::Object(live_map)) => {
            for (key, approved_value) in approved_map {
                let next_path = format!("{path}.{key}");
                match live_map.get(key) {
                    Some(live_value) => diff_value(&next_path, approved_value, live_value, entries),
                    None => entries.push(DriftEntry {
                        path: next_path,
                        kind: DriftKind::Removed,
                        severity: DriftSeverity::Critical,
                        message: format!("Approved field '{key}' is missing from the live schema."),
                    }),
                }
            }

            for key in live_map.keys() {
                if !approved_map.contains_key(key) {
                    entries.push(DriftEntry {
                        path: format!("{path}.{key}"),
                        kind: DriftKind::Added,
                        severity: DriftSeverity::Minor,
                        message: format!("Live schema introduced new field '{key}'."),
                    });
                }
            }
        }
        (Value::Array(approved_values), Value::Array(live_values)) => {
            let shared = approved_values.len().min(live_values.len());
            for index in 0..shared {
                diff_value(
                    &format!("{path}[{index}]"),
                    &approved_values[index],
                    &live_values[index],
                    entries,
                );
            }

            if approved_values.len() > live_values.len() {
                for index in live_values.len()..approved_values.len() {
                    entries.push(DriftEntry {
                        path: format!("{path}[{index}]"),
                        kind: DriftKind::Removed,
                        severity: DriftSeverity::Critical,
                        message: "Live schema removed an approved array entry.".to_string(),
                    });
                }
            }

            if live_values.len() > approved_values.len() {
                for index in approved_values.len()..live_values.len() {
                    entries.push(DriftEntry {
                        path: format!("{path}[{index}]"),
                        kind: DriftKind::Added,
                        severity: DriftSeverity::Minor,
                        message: "Live schema added a new array entry.".to_string(),
                    });
                }
            }
        }
        _ => {
            if approved != live {
                entries.push(DriftEntry {
                    path: path.to_string(),
                    kind: DriftKind::Changed,
                    severity: classify_change(path, approved, live),
                    message: format!(
                        "Value changed from {} to {}.",
                        preview_value(approved),
                        preview_value(live)
                    ),
                });
            }
        }
    }
}

fn classify_change(path: &str, approved: &Value, live: &Value) -> DriftSeverity {
    if path.contains("description")
        || path.contains("title")
        || path.contains("metadata")
        || path.contains("instructions")
    {
        return DriftSeverity::Informational;
    }

    match (approved, live) {
        (Value::String(_), Value::String(_)) => DriftSeverity::Major,
        (Value::Number(_), Value::Number(_)) => DriftSeverity::Major,
        (Value::Bool(_), Value::Bool(_)) => DriftSeverity::Major,
        (Value::Null, _) | (_, Value::Null) => DriftSeverity::Major,
        _ => DriftSeverity::Critical,
    }
}

fn preview_value(value: &Value) -> String {
    let text = match value {
        Value::String(inner) => format!("\"{inner}\""),
        _ => value.to_string(),
    };

    if text.len() > 80 {
        format!("{}...", &text[..77])
    } else {
        text
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    #[test]
    fn canonicalization_keeps_object_key_order_stable() {
        let source = json!({
            "b": { "d": 1, "c": 2 },
            "a": 1
        });

        let canonical = canonicalize_value(source);
        let object = canonical.as_object().expect("object");
        let keys = object.keys().cloned().collect::<Vec<_>>();

        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn detects_removed_field_as_critical_drift() {
        let approved = json!({
            "tools": {
                "search": {
                    "inputSchema": { "type": "object", "required": ["q"] }
                }
            }
        });
        let live = json!({ "tools": {} });

        let report = detect_drift(&approved, &live);

        assert!(report.drift_detected);
        assert_eq!(report.entries[0].kind, DriftKind::Removed);
        assert_eq!(report.entries[0].severity, DriftSeverity::Critical);
    }

    #[test]
    fn detects_description_only_change_as_informational() {
        let approved = json!({ "description": "Old description" });
        let live = json!({ "description": "New description" });

        let report = detect_drift(&approved, &live);

        assert!(report.drift_detected);
        assert_eq!(report.entries[0].severity, DriftSeverity::Informational);
    }

    #[test]
    fn approve_and_check_file_integration_updates_status() {
        let root = env::temp_dir().join(format!("gatepost-test-{}", now_epoch_ms()));
        fs::create_dir_all(&root).expect("temp dir");

        let live_schema_path = root.join("live.json");
        fs::write(
            &live_schema_path,
            serde_json::to_string(&json!({
                "tools": {
                    "search": {
                        "inputSchema": { "type": "object", "required": ["q"] }
                    }
                }
            }))
            .expect("json"),
        )
        .expect("write live");

        let mut store = GatePostStore::default();
        let integration = add_integration(
            &mut store,
            &root,
            CreateIntegrationRequest {
                name: "External Search".to_string(),
                system: "zero".to_string(),
                environment: "test".to_string(),
                server: "partner".to_string(),
                source_kind: SourceKind::FileJson,
                live_schema_path: Some(live_schema_path.to_string_lossy().to_string()),
                http_url: None,
                mcp_command: None,
                mcp_args: Vec::new(),
                sample_tool_calls: Vec::new(),
                policy_mode: PolicyMode::Block,
                check_interval_seconds: 5,
            },
        )
        .expect("integration added");

        let approved = approve_integration(&mut store, &integration.id, "qa.user").expect("approved");
        assert_eq!(approved.status.trust_state, TrustState::Approved);

        fs::write(
            &live_schema_path,
            serde_json::to_string(&json!({
                "tools": {}
            }))
            .expect("json"),
        )
        .expect("write drift");

        let checked = check_integration(&mut store, &integration.id).expect("checked");
        assert_eq!(checked.status.trust_state, TrustState::Blocked);
        assert!(checked.status.drift_detected);
        assert_eq!(checked.incidents.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn approve_and_check_http_integration_updates_status() {
        let root = env::temp_dir().join(format!("gatepost-http-test-{}", now_epoch_ms()));
        fs::create_dir_all(&root).expect("temp dir");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let payload = json!({
            "serverName": "HTTP Search",
            "toolCount": 1,
            "tools": [
                {
                    "name": "search",
                    "description": "Search partner data",
                    "inputSchema": { "type": "object", "required": ["q"] }
                }
            ]
        });

        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut request_buffer = [0_u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut request_buffer);
                let body = serde_json::to_string(&payload).expect("json body");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                std::io::Write::write_all(&mut stream, response.as_bytes()).expect("write response");
                let _ = std::net::Shutdown::Write;
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });

        let mut store = GatePostStore::default();
        let integration = add_integration(
            &mut store,
            &root,
            CreateIntegrationRequest {
                name: "HTTP Search".to_string(),
                system: "zero".to_string(),
                environment: "test".to_string(),
                server: "partner-http".to_string(),
                source_kind: SourceKind::HttpJson,
                live_schema_path: None,
                http_url: Some(format!("http://{}/schema", address)),
                mcp_command: None,
                mcp_args: Vec::new(),
                sample_tool_calls: Vec::new(),
                policy_mode: PolicyMode::AlertOnly,
                check_interval_seconds: 5,
            },
        )
        .expect("integration added");

        let approved = approve_integration(&mut store, &integration.id, "qa.user").expect("approved");
        assert_eq!(approved.status.trust_state, TrustState::Approved);

        let checked = check_integration(&mut store, &integration.id).expect("checked");
        assert_eq!(checked.status.trust_state, TrustState::Approved);
        assert!(!checked.status.drift_detected);

        server.join().expect("server joined");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn check_reports_error_when_baseline_missing() {
        let root = env::temp_dir().join(format!("gatepost-missing-baseline-{}", now_epoch_ms()));
        fs::create_dir_all(&root).expect("temp dir");

        let live_schema_path = root.join("live.json");
        fs::write(
            &live_schema_path,
            serde_json::to_string(&json!({ "tools": {} })).expect("json"),
        )
        .expect("write live");

        let mut store = GatePostStore::default();
        let integration = add_integration(
            &mut store,
            &root,
            CreateIntegrationRequest {
                name: "No Baseline".to_string(),
                system: "zero".to_string(),
                environment: "test".to_string(),
                server: "partner".to_string(),
                source_kind: SourceKind::FileJson,
                live_schema_path: Some(live_schema_path.to_string_lossy().to_string()),
                http_url: None,
                mcp_command: None,
                mcp_args: Vec::new(),
                sample_tool_calls: Vec::new(),
                policy_mode: PolicyMode::AlertOnly,
                check_interval_seconds: 5,
            },
        )
        .expect("integration added");

        let error = check_integration(&mut store, &integration.id).expect_err("missing baseline should error");
        assert!(error.contains("Baseline not found."));
        let current = store
            .integrations
            .iter()
            .find(|item| item.id == integration.id)
            .expect("integration present");
        assert_eq!(current.status.trust_state, TrustState::Error);
        assert!(current.status.error_message.clone().unwrap_or_default().contains("Baseline not found."));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_integration_resets_baseline_state() {
        let root = env::temp_dir().join(format!("gatepost-update-target-{}", now_epoch_ms()));
        fs::create_dir_all(&root).expect("temp dir");

        let live_schema_path = root.join("live.json");
        fs::write(
            &live_schema_path,
            serde_json::to_string(&json!({ "tools": { "search": {} } })).expect("json"),
        )
        .expect("write live");

        let mut store = GatePostStore::default();
        let integration = add_integration(
            &mut store,
            &root,
            CreateIntegrationRequest {
                name: "Editable".to_string(),
                system: "zero".to_string(),
                environment: "test".to_string(),
                server: "partner".to_string(),
                source_kind: SourceKind::FileJson,
                live_schema_path: Some(live_schema_path.to_string_lossy().to_string()),
                http_url: None,
                mcp_command: None,
                mcp_args: Vec::new(),
                sample_tool_calls: Vec::new(),
                policy_mode: PolicyMode::AlertOnly,
                check_interval_seconds: 5,
            },
        )
        .expect("integration added");
        let _ = approve_integration(&mut store, &integration.id, "qa.user").expect("approved");

        let updated = update_integration(
            &mut store,
            &integration.id,
            UpdateIntegrationRequest {
                name: "Editable Updated".to_string(),
                system: "zero".to_string(),
                environment: "prod".to_string(),
                server: "partner-v2".to_string(),
                source_kind: SourceKind::FileJson,
                live_schema_path: Some(live_schema_path.to_string_lossy().to_string()),
                http_url: None,
                mcp_command: None,
                mcp_args: Vec::new(),
                sample_tool_calls: Vec::new(),
                policy_mode: PolicyMode::Block,
                check_interval_seconds: 10,
            },
        )
        .expect("updated");

        assert_eq!(updated.name, "Editable Updated");
        assert_eq!(updated.status.trust_state, TrustState::Uninitialized);
        assert!(updated.approved_snapshot.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ping_baseline_creates_and_updates_snapshot() {
        let root = env::temp_dir().join(format!("gatepost-ping-baseline-{}", now_epoch_ms()));
        fs::create_dir_all(&root).expect("temp dir");

        let live_schema_path = root.join("live.json");
        fs::write(
            &live_schema_path,
            serde_json::to_string(&json!({
                "tools": {
                    "search": {
                        "inputSchema": { "type": "object", "required": ["q"] }
                    }
                }
            }))
            .expect("json"),
        )
        .expect("write live");

        let mut store = GatePostStore::default();
        let integration = add_integration(
            &mut store,
            &root,
            CreateIntegrationRequest {
                name: "Ping Baseline".to_string(),
                system: "zero".to_string(),
                environment: "test".to_string(),
                server: "partner".to_string(),
                source_kind: SourceKind::FileJson,
                live_schema_path: Some(live_schema_path.to_string_lossy().to_string()),
                http_url: None,
                mcp_command: None,
                mcp_args: Vec::new(),
                sample_tool_calls: Vec::new(),
                policy_mode: PolicyMode::AlertOnly,
                check_interval_seconds: 5,
            },
        )
        .expect("integration added");

        let pinged = ping_baseline(&mut store, &integration.id).expect("ping baseline");
        assert_eq!(pinged.status.trust_state, TrustState::Approved);
        assert!(pinged.approved_snapshot.is_some());
        assert!(Path::new(&pinged.snapshot_path).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_http_url_ignores_whitespace_in_url() {
        let (host, port, path) = parse_http_url(" http:// /localhost:41131/mcp/tools ")
            .expect("sanitized URL should parse");
        assert_eq!(host, "localhost");
        assert_eq!(port, 41131);
        assert_eq!(path, "/mcp/tools");
    }

    #[test]
    fn decode_chunked_body_parses_valid_payload() {
        let raw = "7\r\n{\"a\":1}\r\n0\r\n\r\n";
        let decoded = decode_chunked_body(raw).expect("chunked body decode");
        assert_eq!(decoded, "{\"a\":1}");
    }

    #[test]
    fn validate_mcp_tool_catalog_schema_accepts_valid_payload() {
        let payload = json!({
            "serverName": "Orders API",
            "toolCount": 1,
            "tools": [
                {
                    "name": "list_orders",
                    "description": "Lists orders.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                }
            ]
        });
        validate_mcp_tool_catalog_schema(&payload).expect("valid tool catalog");
    }

    #[test]
    fn validate_mcp_tool_catalog_schema_rejects_missing_tools_array() {
        let payload = json!({
            "protocol": "MCP",
            "message": "initialize details"
        });
        let error = validate_mcp_tool_catalog_schema(&payload).expect_err("invalid payload");
        assert!(error.contains("'tools' array"));
    }











}
