use std::io::Read;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use rand::RngCore;
use serde_json::json;

use crate::tasks::model::{CaptureKind, Status};
use crate::tasks::organizer::{self, ReviewState};
use crate::tasks::store::Ledger;
use crate::tasks::views;

const MAX_BODY: usize = 128 * 1024;
const ALLOWED_FILES: &[(&str, &str)] = &[
    ("index.html", "text/html; charset=utf-8"),
    ("style.css", "text/css; charset=utf-8"),
    ("app.js", "application/javascript; charset=utf-8"),
];

pub fn install(from: Option<&Path>, force: bool) -> Result<()> {
    let home = crate::tasks::store::home_dir()?;
    let source = from
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
    if !source.is_dir() {
        bail!(
            "UI source not found: {}; pass --from /path/to/ui",
            source.display()
        );
    }
    for (name, _) in ALLOWED_FILES {
        let src = source.join(name);
        if !src.is_file() {
            bail!("UI file missing: {}", src.display());
        }
    }
    let target = home.join("plugins/ui/current");
    if target.exists() && !force {
        bail!("UI already installed; use --force to overwrite or uninstall first");
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = home.join("plugins/ui/.staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    for (name, _) in ALLOWED_FILES {
        std::fs::copy(source.join(name), staging.join(name))?;
    }
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(&staging, &target)?;
    println!("UI installed to {}", target.display());
    println!("Task data and configuration are never modified.");
    Ok(())
}

pub fn status() -> Result<()> {
    let home = crate::tasks::store::home_dir()?;
    let target = home.join("plugins/ui/current");
    if !target.is_dir() {
        println!("UI: not installed");
        println!("Install: workmesh ui install");
        return Ok(());
    }
    let mut ok = true;
    for (name, _) in ALLOWED_FILES {
        let path = target.join(name);
        let exists = path.is_file();
        if !exists {
            ok = false;
        }
        println!("  {name}: {}", if exists { "✓" } else { "missing" });
    }
    println!("UI: {}", if ok { "ready" } else { "incomplete" });
    println!("Directory: {}", target.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let home = crate::tasks::store::home_dir()?;
    let target = home.join("plugins/ui");
    if !target.exists() {
        println!("UI is not installed.");
        return Ok(());
    }
    std::fs::remove_dir_all(&target)?;
    println!("UI removed. Task data and configuration are untouched.");
    Ok(())
}

pub fn start(port: u16, open_browser: bool) -> Result<()> {
    let token = generate_token();
    let addr = std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow::anyhow!("bind {addr}: {e}"))?;
    let server = Arc::new(server);
    let counter = Arc::new(AtomicU64::new(0));
    let ui_dir = find_ui_dir()?;

    let url = format!("http://{addr}/#token={token}");
    println!("WorkMesh UI: {url}");
    println!("Session token is in the URL fragment; it is not sent to any server.");
    if open_browser {
        let _ = std::process::Command::new(if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "linux") {
            "xdg-open"
        } else {
            "start"
        })
        .arg(&url)
        .spawn();
    }

    let workers: Vec<_> = (0..2)
        .map(|_| {
            let server = server.clone();
            let token = token.clone();
            let ui_dir = ui_dir.clone();
            let counter = counter.clone();
            std::thread::spawn(move || {
                loop {
                    let request = match server.recv() {
                        Ok(r) => r,
                        Err(_) => break,
                    };
                    let id = counter.fetch_add(1, Ordering::Relaxed);
                    if let Err(error) = handle(request, &ui_dir, &token) {
                        eprintln!("[ui:{id}] {error:#}");
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn handle(mut request: tiny_http::Request, ui_dir: &Path, token: &str) -> Result<()> {
    let method = request.method().to_string();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("");
    let auth = request
        .headers()
        .iter()
        .find(|h| {
            h.field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("authorization")
        })
        .map(|h| h.value.as_str().to_string());

    let is_api = path.starts_with("/api/");
    if is_api && check_auth(auth.as_deref(), token).is_err() {
        return respond_text(
            request,
            401,
            "Unauthorized: invalid or missing session token",
        );
    }

    match (method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => serve_static(request, ui_dir, "index.html"),
        ("GET", "/style.css") => serve_static(request, ui_dir, "style.css"),
        ("GET", "/app.js") => serve_static(request, ui_dir, "app.js"),
        ("GET", "/api/state") => {
            let query = url.split('?').nth(1).unwrap_or("");
            let params: std::collections::HashMap<_, _> = query
                .split('&')
                .filter_map(|p| {
                    let mut kv = p.splitn(2, '=');
                    let k = kv.next()?;
                    let v = kv.next().unwrap_or("");
                    Some((k, v))
                })
                .collect();
            let view = params.get("view").copied().unwrap_or("now");
            let search = params.get("search").map(|s| percent_decode(s));
            match api_state(view, search) {
                Ok(response) => respond_json(request, 200, &response),
                Err(e) => respond_json(request, 500, &json!({"error": format!("{e:#}")})),
            }
        }
        ("POST", "/api/capture") => {
            let result: Result<_> = (|| {
                let body = read_body(&mut request)?;
                let input: serde_json::Value =
                    serde_json::from_slice(&body).context("invalid JSON body")?;
                let text = input["text"]
                    .as_str()
                    .context("text is required")?
                    .to_string();
                let kind = match input["kind"].as_str().unwrap_or("idea") {
                    "question" => CaptureKind::Question,
                    "bug" => CaptureKind::Bug,
                    "request" => CaptureKind::Request,
                    _ => CaptureKind::Idea,
                };
                let mut ledger = Ledger::open()?;
                let id = ledger.capture(text, kind, None, "web-ui".into())?;
                Ok(json!({"captured": id}))
            })();
            match result {
                Ok(v) => respond_json(request, 201, &v),
                Err(e) => respond_json(request, 400, &json!({"error": format!("{e:#}")})),
            }
        }
        ("POST", "/api/review") => {
            let result: Result<_> = (|| {
                let body = read_body(&mut request)?;
                let input: serde_json::Value =
                    serde_json::from_slice(&body).context("invalid JSON body")?;
                let id = input["id"].as_str().context("id is required")?.to_string();
                let accept = input["accept"].as_bool().context("accept is required")?;
                let mut ledger = Ledger::open()?;
                let id = organizer::resolve(&ledger, &id)?;
                ledger.review(
                    &id,
                    accept,
                    "web-ui".into(),
                    if accept {
                        None
                    } else {
                        Some("Rejected from web UI".into())
                    },
                )?;
                Ok(json!({"proposal": id, "status": if accept { "accepted" } else { "rejected" }}))
            })();
            match result {
                Ok(v) => respond_json(request, 200, &v),
                Err(e) => respond_json(request, 400, &json!({"error": format!("{e:#}")})),
            }
        }
        _ => respond_text(request, 404, "Not found"),
    }
}

fn check_auth(header: Option<&str>, token: &str) -> Result<()> {
    let Some(header) = header else {
        bail!("missing Authorization header")
    };
    let expected = format!("Bearer {token}");
    if !constant_time_eq(header.as_bytes(), expected.as_bytes()) {
        bail!("invalid session token")
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn api_state(view: &str, search: Option<String>) -> Result<serde_json::Value> {
    let ledger = Ledger::open()?;
    let all = ledger.views()?;
    let organizer_state = organizer::state(&ledger)?;

    let open_captures: Vec<_> = organizer_state
        .captures
        .iter()
        .filter(|c| c.conflict || c.directions.is_empty())
        .filter(|c| {
            search
                .as_ref()
                .is_none_or(|s| c.text.to_lowercase().contains(&s.to_lowercase()))
        })
        .collect();
    let pending: Vec<_> = organizer_state
        .proposals
        .iter()
        .filter(|p| matches!(p.state, ReviewState::Pending | ReviewState::Conflict))
        .collect();

    let selected: Vec<_> = match view {
        "inbox" => all
            .iter()
            .filter(|v| v.task.status == Status::Inbox)
            .cloned()
            .collect(),
        "blocked" => all
            .iter()
            .filter(|v| v.task.status == Status::Blocked)
            .cloned()
            .collect(),
        "stale" => all
            .iter()
            .filter(|v| {
                !v.task.status.closed()
                    && v.updated_at < chrono::Utc::now() - chrono::Duration::days(7)
            })
            .cloned()
            .collect(),
        "needs-me" => all
            .iter()
            .filter(|v| v.task.needs_input || v.conflict)
            .cloned()
            .collect(),
        _ => views::select(all, "now", None, search.as_deref(), 7, 50),
    };

    Ok(json!({
        "view": view,
        "counts": {
            "active": selected.iter().filter(|v| v.task.status == Status::Active).count(),
            "inbox": open_captures.len(),
            "needs_review": pending.len(),
            "conflicts": selected.iter().filter(|v| v.conflict).count(),
            "wip_limit": ledger.config.wip_limit,
        },
        "items": selected,
        "captures": open_captures,
        "proposals": pending,
    }))
}

fn serve_static(request: tiny_http::Request, ui_dir: &Path, filename: &str) -> Result<()> {
    let Some((_, mime)) = ALLOWED_FILES.iter().find(|(name, _)| *name == filename) else {
        bail!("file not allowed: {filename}")
    };
    let path = ui_dir.join(filename);
    let content = std::fs::read(&path).with_context(|| {
        format!("read UI file {filename}; run `workmesh ui install` or build from source")
    })?;
    let ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
        .map_err(|_| anyhow::anyhow!("invalid content type header"))?;
    let cc = tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..])
        .map_err(|_| anyhow::anyhow!("invalid cache header"))?;
    let csp = tiny_http::Header::from_bytes(&b"Content-Security-Policy"[..],
        &b"default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; img-src 'self' data:;"[..])
        .map_err(|_| anyhow::anyhow!("invalid CSP header"))?;
    let response = tiny_http::Response::from_data(content)
        .with_header(ct)
        .with_header(cc)
        .with_header(csp);
    request.respond(response)?;
    Ok(())
}

fn read_body(request: &mut tiny_http::Request) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut reader = request.as_reader().take(MAX_BODY as u64 + 1);
    reader.read_to_end(&mut body)?;
    if body.len() > MAX_BODY {
        bail!("request body exceeds {MAX_BODY} bytes")
    };
    Ok(body)
}

fn header(name: &[u8], value: &[u8]) -> Result<tiny_http::Header> {
    tiny_http::Header::from_bytes(name, value).map_err(|_| anyhow::anyhow!("invalid HTTP header"))
}

fn respond_json(request: tiny_http::Request, code: u16, value: &serde_json::Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    let response = tiny_http::Response::from_data(body)
        .with_status_code(code)
        .with_header(header(&b"Content-Type"[..], &b"application/json"[..])?);
    request.respond(response)?;
    Ok(())
}

fn respond_text(request: tiny_http::Request, code: u16, text: &str) -> Result<()> {
    let response = tiny_http::Response::from_string(text)
        .with_status_code(code)
        .with_header(header(
            &b"Content-Type"[..],
            &b"text/plain; charset=utf-8"[..],
        )?);
    request.respond(response)?;
    Ok(())
}

fn find_ui_dir() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    if manifest.is_dir() {
        return Ok(manifest);
    }
    let home = crate::tasks::store::home_dir()?.join("plugins/ui/current");
    if home.is_dir() {
        return Ok(home);
    }
    bail!("UI not found; run `workmesh ui install` or use the source repository")
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn percent_decode(s: &str) -> String {
    s.replace("%20", " ")
        .replace("%22", "\"")
        .replace("%27", "'")
        .replace("%2F", "/")
}
