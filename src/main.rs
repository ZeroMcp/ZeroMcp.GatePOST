use gatepost::{
    add_integration, approve_integration, check_due_integrations, check_integration, create_snapshot,
    dashboard_summary, load_json_file, load_snapshot, load_store, ping_baseline, save_store,
    store_path, update_integration, write_snapshot, ApprovalRequest, CreateIntegrationRequest,
    GatePostStore, UpdateIntegrationRequest,
};
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() {
    let exit_code = match parse_command(env::args().skip(1).collect()) {
        Ok(Command::Snapshot(args)) => run_snapshot(args),
        Ok(Command::Check(args)) => run_check(args),
        Ok(Command::Serve(args)) => run_server(args),
        Ok(Command::Help) => {
            print_usage();
            0
        }
        Err(error) => {
            eprintln!("{error}\n");
            print_usage();
            1
        }
    };

    std::process::exit(exit_code);
}

struct SnapshotArgs {
    schema: PathBuf,
    out: PathBuf,
    system: String,
    environment: String,
    server: String,
    approved_by: String,
}

struct CheckArgs {
    snapshot: PathBuf,
    schema: PathBuf,
}

struct ServeArgs {
    addr: String,
    data_dir: PathBuf,
}

enum Command {
    Snapshot(SnapshotArgs),
    Check(CheckArgs),
    Serve(ServeArgs),
    Help,
}

#[derive(Clone)]
struct AppContext {
    data_dir: PathBuf,
    state_path: PathBuf,
    store: Arc<Mutex<GatePostStore>>,
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct HttpResponse {
    status_code: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

fn parse_command(args: Vec<String>) -> Result<Command, String> {
    let Some(command) = args.first() else {
        return Ok(Command::Help);
    };

    match command.as_str() {
        "snapshot" => parse_snapshot_args(&args[1..]).map(Command::Snapshot),
        "check" => parse_check_args(&args[1..]).map(Command::Check),
        "serve" => parse_serve_args(&args[1..]).map(Command::Serve),
        "help" | "--help" | "-h" => Ok(Command::Help),
        other => Err(format!("Unknown command '{other}'.")),
    }
}

fn parse_snapshot_args(args: &[String]) -> Result<SnapshotArgs, String> {
    let values = parse_flag_map(args)?;

    Ok(SnapshotArgs {
        schema: required_path(&values, "schema")?,
        out: required_path(&values, "out")?,
        system: required_string(&values, "system")?,
        environment: required_string(&values, "environment")?,
        server: required_string(&values, "server")?,
        approved_by: required_string(&values, "approved_by")?,
    })
}

fn parse_check_args(args: &[String]) -> Result<CheckArgs, String> {
    let values = parse_flag_map(args)?;

    Ok(CheckArgs {
        snapshot: required_path(&values, "snapshot")?,
        schema: required_path(&values, "schema")?,
    })
}

fn parse_serve_args(args: &[String]) -> Result<ServeArgs, String> {
    let values = parse_flag_map(args)?;

    Ok(ServeArgs {
        addr: values
            .get("addr")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1:8080".to_string()),
        data_dir: values
            .get("data_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".gatepost")),
    })
}

fn parse_flag_map(args: &[String]) -> Result<HashMap<String, String>, String> {
    if !args.len().is_multiple_of(2) {
        return Err("Flags must be provided as --name value pairs.".to_string());
    }

    let mut values = HashMap::new();
    let mut index = 0;
    while index < args.len() {
        let key = &args[index];
        let value = &args[index + 1];

        if !key.starts_with("--") {
            return Err(format!("Expected a flag starting with '--', found '{key}'."));
        }

        let normalized_key = key.trim_start_matches("--").replace('-', "_");
        values.insert(normalized_key, value.clone());
        index += 2;
    }

    Ok(values)
}

fn required_string(values: &HashMap<String, String>, key: &str) -> Result<String, String> {
    values
        .get(key)
        .cloned()
        .ok_or_else(|| format!("Missing required flag --{key}."))
}

fn required_path(values: &HashMap<String, String>, key: &str) -> Result<PathBuf, String> {
    required_string(values, key).map(PathBuf::from)
}

fn run_snapshot(args: SnapshotArgs) -> i32 {
    match load_json_file(&args.schema) {
        Ok(schema) => {
            let snapshot = create_snapshot(
                schema,
                args.system,
                args.environment,
                args.server,
                args.approved_by,
            );

            match write_snapshot(&args.out, &snapshot) {
                Ok(()) => {
                    println!("Approved snapshot written to {}", args.out.display());
                    0
                }
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn run_check(args: CheckArgs) -> i32 {
    let snapshot = match load_snapshot(&args.snapshot) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };

    let live_schema = match load_json_file(&args.schema) {
        Ok(schema) => schema,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };

    let report = gatepost::detect_drift(&snapshot.schema, &live_schema);
    println!("{}", report.summary());

    if report.entries.is_empty() {
        return 0;
    }

    for entry in &report.entries {
        println!(
            "- [{} {:?}] {}: {}",
            format_severity(&entry.severity),
            entry.kind,
            entry.path,
            entry.message
        );
    }

    2
}

fn run_server(args: ServeArgs) -> i32 {
    if let Err(error) = std::fs::create_dir_all(&args.data_dir) {
        eprintln!("Failed to create data directory {}: {error}", args.data_dir.display());
        return 1;
    }

    let state_path = store_path(&args.data_dir);
    let store = match load_store(&state_path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };

    let context = AppContext {
        data_dir: args.data_dir.clone(),
        state_path,
        store: Arc::new(Mutex::new(store)),
    };

    start_poll_loop(context.clone());

    let listener = match TcpListener::bind(&args.addr) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Failed to bind {}: {error}", args.addr);
            return 1;
        }
    };

    println!("GatePOST UI running at http://{}", args.addr);
    println!("State stored in {}", context.state_path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => handle_connection(stream, context.clone()),
            Err(error) => eprintln!("Connection failed: {error}"),
        }
    }

    0
}

fn start_poll_loop(context: AppContext) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(3));
        let save_result = {
            let mut guard = match context.store.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            check_due_integrations(&mut guard);
            save_store(&context.state_path, &guard)
        };

        if let Err(error) = save_result {
            eprintln!("Failed to persist GatePOST state: {error}");
        }
    });
}

fn handle_connection(mut stream: TcpStream, context: AppContext) {
    match read_http_request(&mut stream) {
        Ok(request) => {
            let response = route_request(request, &context);
            if let Err(error) = write_response(&mut stream, response) {
                eprintln!("Failed to write response: {error}");
            }
        }
        Err(error) => {
            let _ = write_response(
                &mut stream,
                text_response(400, format!("Bad request: {error}")),
            );
        }
    }
}

fn route_request(request: HttpRequest, context: &AppContext) -> HttpResponse {
    let result: Result<HttpResponse, String> = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Ok(html_response(APP_HTML.to_string())),
        ("GET", "/api/status") => Ok(with_store(context, |store| json_response(&dashboard_summary(store)))),
        ("POST", "/api/integrations") => {
            parse_json::<CreateIntegrationRequest>(&request.body).and_then(|payload| {
                with_store_mut(context, |store| {
                    let integration = add_integration(store, &context.data_dir, payload)?;
                    save_store(&context.state_path, store)?;
                    Ok(json_response(&integration))
                })
            })
        }
        _ if request.method == "POST" && request.path.ends_with("/approve") => {
            match extract_integration_id(&request.path, "/approve") {
                Ok(integration_id) => parse_json::<ApprovalRequest>(&request.body).and_then(|payload| {
                    with_store_mut(context, |store| {
                        let result = approve_integration(store, &integration_id, &payload.approved_by);
                        save_store(&context.state_path, store)?;
                        match result {
                            Ok(integration) => Ok(json_response(&integration)),
                            Err(error) => Err(error),
                        }
                    })
                }),
                Err(error) => Err(error),
            }
        }
        _ if request.method == "PUT" && request.path.starts_with("/api/integrations/") => {
            match extract_integration_id_exact(&request.path) {
                Ok(integration_id) => parse_json::<UpdateIntegrationRequest>(&request.body).and_then(|payload| {
                    with_store_mut(context, |store| {
                        let integration = update_integration(store, &integration_id, payload)?;
                        save_store(&context.state_path, store)?;
                        Ok(json_response(&integration))
                    })
                }),
                Err(error) => Err(error),
            }
        }
        _ if request.method == "POST" && request.path.ends_with("/check") => {
            match extract_integration_id(&request.path, "/check") {
                Ok(integration_id) => with_store_mut(context, |store| {
                    let result = check_integration(store, &integration_id);
                    save_store(&context.state_path, store)?;
                    match result {
                        Ok(integration) => Ok(json_response(&integration)),
                        Err(error) => Err(error),
                    }
                }),
                Err(error) => Err(error),
            }
        }
        _ if request.method == "POST" && request.path.ends_with("/baseline/ping") => {
            match extract_integration_id(&request.path, "/baseline/ping") {
                Ok(integration_id) => with_store_mut(context, |store| {
                    let result = ping_baseline(store, &integration_id);
                    save_store(&context.state_path, store)?;
                    match result {
                        Ok(integration) => Ok(json_response(&integration)),
                        Err(error) => Err(error),
                    }
                }),
                Err(error) => Err(error),
            }
        }
        _ => Err("Not found".to_string()),
    };

    result.unwrap_or_else(|error| {
        if error == "Not found" {
            text_response(404, error)
        } else {
            text_response(400, error)
        }
    })
}

fn with_store<T>(context: &AppContext, handler: impl FnOnce(&GatePostStore) -> T) -> T {
    let guard = context.store.lock().expect("store lock");
    handler(&guard)
}

fn with_store_mut(
    context: &AppContext,
    handler: impl FnOnce(&mut GatePostStore) -> Result<HttpResponse, String>,
) -> Result<HttpResponse, String> {
    let mut guard = context.store.lock().map_err(|_| "Store lock poisoned".to_string())?;
    handler(&mut guard)
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|error| format!("Invalid JSON payload: {error}"))
}

fn extract_integration_id(path: &str, suffix: &str) -> Result<String, String> {
    let trimmed = path.trim_start_matches("/api/integrations/").trim_end_matches(suffix);
    if trimmed.is_empty() || trimmed == path {
        return Err("Invalid integration endpoint.".to_string());
    }
    Ok(trimmed.trim_matches('/').to_string())
}

fn extract_integration_id_exact(path: &str) -> Result<String, String> {
    let trimmed = path
        .strip_prefix("/api/integrations/")
        .ok_or_else(|| "Invalid integration endpoint.".to_string())?
        .trim_matches('/');
    if trimmed.is_empty() || trimmed.contains('/') {
        return Err("Invalid integration endpoint.".to_string());
    }
    Ok(trimmed.to_string())
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("Failed to set read timeout: {error}"))?;

    let mut buffer = vec![0_u8; 65536];
    let mut read = 0;
    loop {
        let bytes = stream
            .read(&mut buffer[read..])
            .map_err(|error| format!("Failed to read request: {error}"))?;
        if bytes == 0 {
            break;
        }
        read += bytes;
        if read >= 4 && buffer[..read].windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if read == buffer.len() {
            return Err("Request too large".to_string());
        }
    }

    let request_text = String::from_utf8_lossy(&buffer[..read]).to_string();
    let header_end = request_text
        .find("\r\n\r\n")
        .ok_or_else(|| "Malformed HTTP request".to_string())?;
    let head = &request_text[..header_end];
    let mut lines = head.lines();
    let request_line = lines.next().ok_or_else(|| "Missing request line".to_string())?;
    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts
        .next()
        .ok_or_else(|| "Missing request method".to_string())?
        .to_string();
    let path = request_line_parts
        .next()
        .ok_or_else(|| "Missing request path".to_string())?
        .to_string();

    let mut content_length = 0_usize;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            content_length = line
                .split(':')
                .nth(1)
                .map(str::trim)
                .ok_or_else(|| "Invalid Content-Length header".to_string())?
                .parse::<usize>()
                .map_err(|error| format!("Invalid Content-Length value: {error}"))?;
        }
    }

    let body_start = header_end + 4;
    let mut body = buffer[body_start..read].to_vec();
    while body.len() < content_length {
        let remaining = content_length - body.len();
        let mut chunk = vec![0_u8; remaining.min(8192)];
        let bytes = stream
            .read(&mut chunk)
            .map_err(|error| format!("Failed to read request body: {error}"))?;
        if bytes == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..bytes]);
    }

    Ok(HttpRequest { method, path, body })
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<(), String> {
    let status_text = match response.status_code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };

    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status_code,
        status_text,
        response.content_type,
        response.body.len()
    );

    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(&response.body))
        .map_err(|error| format!("Failed to send response: {error}"))
}

fn json_response<T: Serialize>(value: &T) -> HttpResponse {
    let body = serde_json::to_vec_pretty(value).unwrap_or_else(|_| b"{}".to_vec());
    HttpResponse {
        status_code: 200,
        content_type: "application/json",
        body,
    }
}

fn html_response(body: String) -> HttpResponse {
    HttpResponse {
        status_code: 200,
        content_type: "text/html",
        body: body.into_bytes(),
    }
}

fn text_response(status_code: u16, body: String) -> HttpResponse {
    HttpResponse {
        status_code,
        content_type: "text/plain",
        body: body.into_bytes(),
    }
}

fn format_severity(severity: &gatepost::DriftSeverity) -> &'static str {
    match severity {
        gatepost::DriftSeverity::Informational => "info",
        gatepost::DriftSeverity::Minor => "minor",
        gatepost::DriftSeverity::Major => "major",
        gatepost::DriftSeverity::Critical => "critical",
    }
}

fn print_usage() {
    println!(
        "GatePOST\n\nCommands:\n  serve --addr <host:port> --data-dir <dir>\n  snapshot --schema <file> --out <file> --system <name> --environment <env> --server <name> --approved-by <name>\n  check --snapshot <file> --schema <file>\n"
    );
}

const APP_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>GatePOST</title>
  <style>
    :root {
      --bg: #f4efe7;
      --panel: rgba(255, 251, 245, 0.9);
      --ink: #1f2933;
      --muted: #5f6c7b;
      --line: rgba(31, 41, 51, 0.12);
      --accent: #0f766e;
      --accent-2: #d97706;
      --danger: #b42318;
      --ok: #18794e;
      --shadow: 0 24px 60px rgba(39, 43, 51, 0.14);
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: Georgia, "Times New Roman", serif;
      color: var(--ink);
      background:
        radial-gradient(circle at top left, rgba(15, 118, 110, 0.18), transparent 32%),
        radial-gradient(circle at top right, rgba(217, 119, 6, 0.18), transparent 28%),
        linear-gradient(180deg, #f7f1e8 0%, #efe6d9 100%);
      min-height: 100vh;
    }
    .shell {
      width: min(1180px, calc(100% - 32px));
      margin: 24px auto 56px;
    }
    .hero {
      padding: 28px;
      border: 1px solid var(--line);
      border-radius: 28px;
      background: rgba(23, 29, 38, 0.94);
      color: #f8f4ec;
      box-shadow: var(--shadow);
      position: relative;
      overflow: hidden;
    }
    .hero::after {
      content: "";
      position: absolute;
      inset: auto -40px -60px auto;
      width: 220px;
      height: 220px;
      border-radius: 999px;
      background: radial-gradient(circle, rgba(240, 171, 0, 0.32), transparent 65%);
    }
    .eyebrow {
      letter-spacing: 0.16em;
      text-transform: uppercase;
      font-size: 12px;
      color: #f7b955;
      margin-bottom: 12px;
    }
    h1 {
      margin: 0;
      font-size: clamp(34px, 5vw, 58px);
      line-height: 0.95;
      max-width: 9ch;
    }
    .hero p {
      max-width: 680px;
      color: rgba(248, 244, 236, 0.78);
      font-size: 18px;
      line-height: 1.5;
      margin-top: 16px;
    }
    .grid {
      display: grid;
      grid-template-columns: 320px 1fr;
      gap: 18px;
      margin-top: 20px;
      align-items: start;
    }
    .panel {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 24px;
      box-shadow: var(--shadow);
      backdrop-filter: blur(8px);
    }
    .panel-inner { padding: 20px; }
    .panel h2, .panel h3 { margin: 0 0 12px; }
    .stats {
      display: grid;
      grid-template-columns: repeat(3, 1fr);
      gap: 12px;
      margin-top: 18px;
    }
    .stat {
      padding: 14px;
      border-radius: 18px;
      background: rgba(255,255,255,0.08);
      border: 1px solid rgba(255,255,255,0.12);
    }
    .stat strong { display: block; font-size: 28px; margin-top: 4px; }
    label { display: block; font-size: 13px; color: var(--muted); margin-bottom: 6px; }
    input, select, textarea {
      width: 100%;
      padding: 12px 14px;
      border-radius: 14px;
      border: 1px solid var(--line);
      background: rgba(255,255,255,0.7);
      font: inherit;
      margin-bottom: 12px;
    }
    button {
      border: 0;
      border-radius: 999px;
      padding: 11px 16px;
      font: inherit;
      background: var(--ink);
      color: white;
      cursor: pointer;
      transition: transform 120ms ease, opacity 120ms ease;
    }
    button:hover { transform: translateY(-1px); }
    button.secondary { background: white; color: var(--ink); border: 1px solid var(--line); }
    button.accent { background: var(--accent); }
    button.warn { background: var(--accent-2); }
    .cards {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
      gap: 16px;
    }
    .integration {
      padding: 18px;
      border-radius: 22px;
      border: 1px solid var(--line);
      background: linear-gradient(180deg, rgba(255,255,255,0.88), rgba(255,248,240,0.88));
    }
    .integration header {
      display: flex;
      align-items: start;
      justify-content: space-between;
      gap: 10px;
      margin-bottom: 10px;
    }
    .integration h3 { font-size: 22px; margin: 0; }
    .muted { color: var(--muted); }
    .pill {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      border-radius: 999px;
      padding: 6px 10px;
      font-size: 12px;
      text-transform: uppercase;
      letter-spacing: 0.08em;
      border: 1px solid var(--line);
      background: white;
    }
    .approved { color: var(--ok); }
    .driftdetected, .blocked, .error { color: var(--danger); }
    .uninitialized { color: var(--accent-2); }
    .actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 16px; }
    .meta { display: grid; grid-template-columns: repeat(2, 1fr); gap: 8px; font-size: 13px; margin-top: 14px; }
    .meta div {
      padding: 10px;
      border-radius: 14px;
      background: rgba(15, 23, 42, 0.04);
    }
    .report, .incidents, .baseline {
      margin-top: 14px;
      padding-top: 14px;
      border-top: 1px solid var(--line);
    }
    pre {
      margin: 10px 0 0;
      padding: 14px;
      border-radius: 16px;
      background: rgba(23, 29, 38, 0.92);
      color: #f8f4ec;
      overflow-x: auto;
      font-size: 12px;
      line-height: 1.45;
    }
    ul { padding-left: 18px; }
    .notice {
      margin-top: 12px;
      padding: 12px 14px;
      border-radius: 16px;
      background: rgba(15, 118, 110, 0.08);
      border: 1px solid rgba(15, 118, 110, 0.18);
      color: #0f4f4a;
    }
    .error-box {
      background: rgba(180, 35, 24, 0.08);
      border-color: rgba(180, 35, 24, 0.16);
      color: var(--danger);
    }
    @media (max-width: 900px) {
      .grid { grid-template-columns: 1fr; }
      .stats { grid-template-columns: 1fr; }
      .meta { grid-template-columns: 1fr; }
    }
  </style>
</head>
<body>
  <div class="shell">
    <section class="hero">
      <div class="eyebrow">ZeroMcp Safety Barrier</div>
      <h1>GatePOST status desk.</h1>
      <p>GatePOST watches approved MCP schemas, surfaces drift fast, and keeps retesting visible before changed integrations regain trust.</p>
      <div class="stats" id="stats"></div>
    </section>

    <div class="grid">
      <aside class="panel">
        <div class="panel-inner">
          <h2>Protect an Integration</h2>
          <p class="muted">Register an external MCP schema source, choose a policy, and let GatePOST poll for drift.</p>
          <form id="create-form">
            <label for="name">Name</label>
            <input id="name" name="name" placeholder="Partner Search" required>
            <label for="system">System</label>
            <input id="system" name="system" placeholder="zero-suite" required>
            <label for="environment">Environment</label>
            <input id="environment" name="environment" placeholder="production" required>
            <label for="server">Server</label>
            <input id="server" name="server" placeholder="partner-mcp" required>
            <label for="source_kind">Source type</label>
            <select id="source_kind" name="source_kind">
              <option value="file_json">File JSON</option>
              <option value="http_json">HTTP JSON</option>
              <option value="mcp_stdio">MCP stdio</option>
            </select>
            <label for="live_schema_path">Live schema path</label>
            <input id="live_schema_path" name="live_schema_path" placeholder="examples\live-schema.json">
            <label for="http_url">HTTP schema URL</label>
            <input id="http_url" name="http_url" placeholder="http://127.0.0.1:8081/schema">
            <label for="mcp_command">MCP command</label>
            <input id="mcp_command" name="mcp_command" placeholder="python">
            <label for="mcp_args">MCP args (one per line)</label>
            <textarea id="mcp_args" name="mcp_args" rows="4" placeholder="examples\mock_mcp_server.py"></textarea>
            <label for="sample_tool_calls">Sample tools/call JSON (optional)</label>
            <textarea id="sample_tool_calls" name="sample_tool_calls" rows="5" placeholder='[{"name":"search","arguments":{"q":"gatepost"}}]'></textarea>
            <label for="policy_mode">Policy</label>
            <select id="policy_mode" name="policy_mode">
              <option value="alert_only">Alert only</option>
              <option value="block">Block on drift</option>
            </select>
            <label for="check_interval_seconds">Check interval (seconds)</label>
            <input id="check_interval_seconds" name="check_interval_seconds" type="number" min="5" value="30" required>
            <div class="actions">
              <button id="submit-button" class="accent" type="submit">Add integration</button>
              <button id="cancel-edit-button" class="secondary" type="button" style="display:none">Cancel edit</button>
            </div>
          </form>
          <div id="form-status" class="notice" style="display:none"></div>
        </div>
      </aside>

      <main class="panel">
        <div class="panel-inner">
          <h2>Current Trust Status</h2>
          <p class="muted">Approve a baseline when testing signs off, then use Check now to force a fresh drift evaluation.</p>
          <div id="integrations" class="cards"></div>
        </div>
      </main>
    </div>
  </div>

  <script>
    const integrationsEl = document.getElementById('integrations');
    const statsEl = document.getElementById('stats');
    const form = document.getElementById('create-form');
    const formStatus = document.getElementById('form-status');
    const submitButton = document.getElementById('submit-button');
    const cancelEditButton = document.getElementById('cancel-edit-button');
    let currentIntegrations = [];
    let editingIntegrationId = null;

    async function fetchStatus() {
      const response = await fetch('/api/status');
      if (!response.ok) {
        throw new Error('Failed to load GatePOST status.');
      }
      return response.json();
    }

    async function postJson(url, payload) {
      const response = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
      });
      const text = await response.text();
      if (!response.ok) {
        throw new Error(text || 'Request failed.');
      }
      return text ? JSON.parse(text) : null;
    }

    function formatTime(value) {
      if (!value) return 'Not yet checked';
      return new Date(Number(value)).toLocaleString();
    }

    function formatJson(value) {
      return JSON.stringify(value, null, 2);
    }

    function escapeHtml(value) {
      return value
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;');
    }

    function currentSourceSummary(item) {
      if (item.source_kind === 'mcp_stdio') {
        const command = item.mcp_command || 'missing command';
        const args = Array.isArray(item.mcp_args) ? item.mcp_args.join(' ') : '';
        return args ? `${command} ${args}` : command;
      }
      if (item.source_kind === 'http_json') {
        return item.http_url || 'No HTTP URL configured';
      }
      return item.live_schema_path || 'No file path configured';
    }

    function normalizedNullable(value) {
      if (value === undefined || value === null) return null;
      const trimmed = String(value).trim();
      return trimmed.length ? trimmed : null;
    }

    function renderStats(integrations) {
      const approved = integrations.filter(item => item.status.trust_state === 'approved').length;
      const drifted = integrations.filter(item => item.status.drift_detected).length;
      const blocked = integrations.filter(item => item.status.trust_state === 'blocked').length;
      statsEl.innerHTML = `
        <div class="stat"><span>Protected integrations</span><strong>${integrations.length}</strong></div>
        <div class="stat"><span>Approved</span><strong>${approved}</strong></div>
        <div class="stat"><span>Drifted or blocked</span><strong>${drifted + blocked}</strong></div>
      `;
    }

    function renderIntegrations(integrations) {
      if (!integrations.length) {
        integrationsEl.innerHTML = '<div class="integration"><h3>No integrations yet</h3><p class="muted">Add one on the left to start monitoring schema drift.</p></div>';
        return;
      }

      integrationsEl.innerHTML = integrations.map(item => {
        const report = item.last_report?.entries ?? [];
        const incidents = item.incidents ?? [];
        const baseline = item.approved_snapshot;
        const trustClass = (item.status.trust_state || 'uninitialized').replace(/_/g, '');
        return `
          <article class="integration">
            <header>
              <div>
                <div class="muted">${item.system} / ${item.environment}</div>
                <h3>${item.name}</h3>
              </div>
              <span class="pill ${trustClass}">${item.status.trust_state.replace(/_/g, ' ')}</span>
            </header>
            <p>${item.status.summary}</p>
            <div class="meta">
              <div><strong>Server</strong><br>${escapeHtml(item.server)}</div>
              <div><strong>Policy</strong><br>${escapeHtml(item.policy_mode.replace('_', ' '))}</div>
              <div><strong>Source type</strong><br>${escapeHtml(item.source_kind.replace('_', ' '))}</div>
              <div><strong>Source target</strong><br>${escapeHtml(currentSourceSummary(item))}</div>
              <div><strong>Sample calls</strong><br>${Array.isArray(item.sample_tool_calls) ? item.sample_tool_calls.length : 0}</div>
              <div><strong>Last checked</strong><br>${formatTime(item.status.last_checked_epoch_ms)}</div>
            </div>
            <div class="actions">
              <button class="secondary" data-action="edit" data-id="${item.id}">Edit target</button>
              <button class="accent" data-action="approve" data-id="${item.id}">Approve snapshot</button>
              <button class="secondary" data-action="check" data-id="${item.id}">Check now</button>
              <button class="warn" data-action="ping-baseline" data-id="${item.id}">Ping baseline</button>
            </div>
            ${item.status.error_message ? `<div class="notice error-box">${item.status.error_message}</div>` : ''}
            <section class="baseline">
              <h3>Approved baseline</h3>
              ${baseline ? `
                <p class="muted">Approved by <strong>${baseline.metadata.approved_by}</strong> on ${formatTime(baseline.metadata.approved_at_epoch_ms)}</p>
                <pre>${escapeHtml(formatJson(baseline.schema))}</pre>
              ` : '<p class="muted">No baseline captured yet. Approve a snapshot after testing sign-off.</p>'}
            </section>
            <section class="report">
              <h3>Latest diff</h3>
              ${report.length ? `<ul>${report.map(entry => `<li><strong>${entry.severity}</strong> ${entry.path}: ${entry.message}</li>`).join('')}</ul>` : '<p class="muted">No drift entries recorded.</p>'}
            </section>
            <section class="incidents">
              <h3>Incident history</h3>
              ${incidents.length ? `<ul>${incidents.slice().reverse().map(incident => `<li>${formatTime(incident.opened_at_epoch_ms)}: ${incident.summary}</li>`).join('')}</ul>` : '<p class="muted">No drift incidents yet.</p>'}
            </section>
          </article>
        `;
      }).join('');
    }

    async function refresh() {
      try {
        const payload = await fetchStatus();
        currentIntegrations = payload.integrations || [];
        renderStats(currentIntegrations);
        renderIntegrations(currentIntegrations);
      } catch (error) {
        integrationsEl.innerHTML = `<div class="integration"><h3>UI error</h3><p>${error.message}</p></div>`;
      }
    }

    function clearEditMode() {
      editingIntegrationId = null;
      submitButton.textContent = 'Add integration';
      cancelEditButton.style.display = 'none';
      form.reset();
      form.source_kind.value = 'file_json';
      form.check_interval_seconds.value = 30;
      toggleSourceFields();
    }

    function populateFormForEdit(item) {
      editingIntegrationId = item.id;
      form.name.value = item.name || '';
      form.system.value = item.system || '';
      form.environment.value = item.environment || '';
      form.server.value = item.server || '';
      form.source_kind.value = item.source_kind || 'file_json';
      form.live_schema_path.value = item.live_schema_path || '';
      form.http_url.value = item.http_url || '';
      form.mcp_command.value = item.mcp_command || '';
      form.mcp_args.value = Array.isArray(item.mcp_args) ? item.mcp_args.join('\n') : '';
      form.sample_tool_calls.value = Array.isArray(item.sample_tool_calls) && item.sample_tool_calls.length
        ? formatJson(item.sample_tool_calls)
        : '';
      form.policy_mode.value = item.policy_mode || 'alert_only';
      form.check_interval_seconds.value = Number(item.check_interval_seconds || 30);
      submitButton.textContent = 'Save changes';
      cancelEditButton.style.display = 'inline-flex';
      toggleSourceFields();
    }

    function toggleSourceFields() {
      const sourceKind = form.source_kind.value;
      const isMcp = sourceKind === 'mcp_stdio';
      const isHttp = sourceKind === 'http_json';
      form.live_schema_path.required = sourceKind === 'file_json';
      form.http_url.required = isHttp;
      form.mcp_command.required = isMcp;
    }

    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      try {
        const wasEditing = Boolean(editingIntegrationId);
        const payload = Object.fromEntries(new FormData(form).entries());
        payload.name = String(payload.name || '').trim();
        payload.system = String(payload.system || '').trim();
        payload.environment = String(payload.environment || '').trim();
        payload.server = String(payload.server || '').trim();
        payload.check_interval_seconds = Number(payload.check_interval_seconds);
        payload.mcp_args = payload.mcp_args
          ? payload.mcp_args.split(/\r?\n/).map(value => value.trim()).filter(Boolean)
          : [];
        payload.sample_tool_calls = payload.sample_tool_calls
          ? JSON.parse(payload.sample_tool_calls)
          : [];
        payload.live_schema_path = normalizedNullable(payload.live_schema_path);
        payload.http_url = normalizedNullable(payload.http_url);
        payload.mcp_command = normalizedNullable(payload.mcp_command);
        if (wasEditing) {
          const response = await fetch(`/api/integrations/${editingIntegrationId}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
          });
          const text = await response.text();
          if (!response.ok) {
            throw new Error(text || 'Request failed.');
          }
        } else {
          await postJson('/api/integrations', payload);
        }
        clearEditMode();
        formStatus.style.display = 'block';
        formStatus.className = 'notice';
        formStatus.textContent = wasEditing
          ? 'Integration target updated. Approve a new baseline before drift checks.'
          : 'Integration added. Approve a snapshot when testing is signed off.';
        await refresh();
      } catch (error) {
        formStatus.style.display = 'block';
        formStatus.textContent = error.message;
        formStatus.className = 'notice error-box';
      }
    });

    form.source_kind.addEventListener('change', toggleSourceFields);
    cancelEditButton.addEventListener('click', clearEditMode);
    toggleSourceFields();

    integrationsEl.addEventListener('click', async (event) => {
      const button = event.target.closest('button[data-action]');
      if (!button) return;
      const id = button.dataset.id;
      const action = button.dataset.action;

      try {
        if (action === 'edit') {
          const item = currentIntegrations.find(entry => entry.id === id);
          if (!item) {
            throw new Error('Integration not found in current view.');
          }
          populateFormForEdit(item);
          formStatus.style.display = 'block';
          formStatus.className = 'notice';
          formStatus.textContent = `Editing ${item.name}. Save changes to update this target.`;
          return;
        }
        if (action === 'approve') {
          const approvedBy = window.prompt('Approver name', 'qa.signoff');
          if (!approvedBy) return;
          await postJson(`/api/integrations/${id}/approve`, { approved_by: approvedBy });
        }
        if (action === 'check') {
          await postJson(`/api/integrations/${id}/check`, {});
        }
        if (action === 'ping-baseline') {
          await postJson(`/api/integrations/${id}/baseline/ping`, {});
        }
        await refresh();
      } catch (error) {
        window.alert(error.message);
      }
    });

    refresh();
    setInterval(refresh, 4000);
  </script>
</body>
</html>
"#;












#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;

    #[test]
    fn parse_flag_map_normalizes_hyphenated_keys() {
        let args = vec![
            "--approved-by".to_string(),
            "qa.user".to_string(),
            "--check-interval-seconds".to_string(),
            "30".to_string(),
        ];

        let parsed = parse_flag_map(&args).expect("flags parsed");

        assert_eq!(parsed.get("approved_by"), Some(&"qa.user".to_string()));
        assert_eq!(
            parsed.get("check_interval_seconds"),
            Some(&"30".to_string())
        );
    }

    #[test]
    fn parse_flag_map_rejects_odd_number_of_args() {
        let args = vec!["--schema".to_string()];
        let error = parse_flag_map(&args).expect_err("should fail");
        assert!(error.contains("--name value pairs"));
    }

    #[test]
    fn extract_integration_id_exact_rejects_nested_paths() {
        let error = extract_integration_id_exact("/api/integrations/demo/check")
            .expect_err("nested path should fail");
        assert!(error.contains("Invalid integration endpoint"));
    }

    #[test]
    fn extract_integration_id_trims_suffix_and_slashes() {
        let id = extract_integration_id("/api/integrations/demo/check", "/check")
            .expect("id extracted");
        assert_eq!(id, "demo");
    }

    #[test]
    fn read_http_request_parses_method_path_and_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");

        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).expect("connect client");
            let body = r#"{"approved_by":"qa.user"}"#;
            let request = format!(
                "POST /api/integrations/demo/approve HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(request.as_bytes())
                .expect("write request");
            let _ = stream.shutdown(Shutdown::Both);
        });

        let (mut server_stream, _) = listener.accept().expect("accept server");
        let request = read_http_request(&mut server_stream).expect("request parsed");

        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/integrations/demo/approve");
        assert_eq!(
            String::from_utf8(request.body).expect("utf8 body"),
            r#"{"approved_by":"qa.user"}"#
        );

        client.join().expect("client joined");
    }

    #[test]
    fn parse_command_defaults_to_help_without_args() {
        let command = parse_command(Vec::new()).expect("command parsed");
        assert!(matches!(command, Command::Help));
    }
}
