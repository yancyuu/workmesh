use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, TextContent,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Installed {
    pub name: &'static str,
    pub adapter: &'static str,
    pub args: Vec<&'static str>,
    pub path: Option<PathBuf>,
    pub runtime_path: Option<PathBuf>,
    pub upstream: &'static str,
}

pub fn list() -> Vec<Installed> {
    [
        (
            "claude",
            "claude-agent-acp",
            "claude",
            Vec::new(),
            "https://github.com/agentclientprotocol/claude-agent-acp",
        ),
        (
            "codex",
            "codex-acp",
            "codex",
            Vec::new(),
            "https://github.com/agentclientprotocol/codex-acp",
        ),
        (
            "pi",
            "pi-acp",
            "pi",
            Vec::new(),
            "https://github.com/regadas/pi-acp",
        ),
        (
            "opencode",
            "opencode",
            "opencode",
            vec!["acp"],
            "https://opencode.ai/docs/acp/",
        ),
    ]
    .into_iter()
    .map(|(name, adapter, runtime, args, upstream)| Installed {
        name,
        adapter,
        args,
        path: locate(adapter),
        runtime_path: locate(runtime),
        upstream,
    })
    .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub initialized: bool,
    pub agent: Option<serde_json::Value>,
    pub capabilities: Option<serde_json::Value>,
    pub auth_methods: Option<serde_json::Value>,
    pub session: Option<String>,
    pub stop_reason: Option<String>,
    pub notifications: usize,
    pub permission_requests_cancelled: usize,
}

pub fn check(
    name: &str,
    executable: Option<PathBuf>,
    args: Vec<String>,
    seconds: u64,
) -> Result<RunReport> {
    execute(name, executable, args, None, None, seconds)
}

pub fn prompt(
    name: &str,
    executable: Option<PathBuf>,
    args: Vec<String>,
    cwd: PathBuf,
    prompt: String,
    seconds: u64,
) -> Result<RunReport> {
    if prompt.trim().is_empty() || prompt.len() > 64 * 1024 {
        bail!("prompt must be non-empty and at most 64 KiB");
    }
    let cwd = cwd.canonicalize().context("working directory must exist")?;
    if !cwd.is_dir() {
        bail!("working directory must be a directory");
    }
    execute(name, executable, args, Some(cwd), Some(prompt), seconds)
}

fn execute(
    name: &str,
    executable: Option<PathBuf>,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    prompt: Option<String>,
    seconds: u64,
) -> Result<RunReport> {
    if !(1..=3600).contains(&seconds) {
        bail!("timeout must be 1..3600 seconds");
    }
    let (command, arguments) = if let Some(path) = executable {
        if !path.is_absolute() {
            bail!("custom ACP command must be an absolute path");
        }
        if !path.is_file() {
            bail!("custom ACP command does not exist");
        }
        (path, args)
    } else {
        if !args.is_empty() {
            bail!("custom arguments require --command");
        }
        let installed = list()
            .into_iter()
            .find(|i| i.name == name)
            .context("unknown adapter; use harness list or --command /absolute/path")?;
        let command = installed.path.context("ACP adapter not installed (the native CLI alone may not serve ACP). Install/configure the upstream adapter separately; WorkMesh never downloads it implicitly")?;
        (
            command,
            installed.args.into_iter().map(str::to_owned).collect(),
        )
    };
    let report = Arc::new(Mutex::new(RunReport {
        initialized: false,
        agent: None,
        capabilities: None,
        auth_methods: None,
        session: None,
        stop_reason: None,
        notifications: 0,
        permission_requests_cancelled: 0,
    }));
    let notifications = report.clone();
    let permissions = report.clone();
    let lifecycle = report.clone();
    let agent = AcpAgent::new(AcpAgentConfig::new(command).args(arguments));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let connection = Client
            .v1()
            .on_receive_notification(
                async move |_notification: SessionNotification, _cx| {
                    notifications.lock().unwrap().notifications += 1;
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |_request: RequestPermissionRequest, responder, _connection| {
                    permissions.lock().unwrap().permission_requests_cancelled += 1;
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
                let initialized = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                {
                    let mut report = lifecycle.lock().unwrap();
                    report.initialized = true;
                    report.agent =
                        Some(serde_json::to_value(&initialized.agent_info).unwrap_or_default());
                    report.capabilities = Some(
                        serde_json::to_value(&initialized.agent_capabilities).unwrap_or_default(),
                    );
                    report.auth_methods =
                        Some(serde_json::to_value(&initialized.auth_methods).unwrap_or_default());
                }
                if let (Some(cwd), Some(prompt)) = (cwd, prompt) {
                    let session = connection
                        .send_request(NewSessionRequest::new(cwd))
                        .block_task()
                        .await?;
                    lifecycle.lock().unwrap().session = Some(session.session_id.to_string());
                    let result = connection
                        .send_request(PromptRequest::new(
                            session.session_id,
                            vec![ContentBlock::Text(TextContent::new(prompt))],
                        ))
                        .block_task()
                        .await?;
                    lifecycle.lock().unwrap().stop_reason =
                        Some(format!("{:?}", result.stop_reason));
                }
                Ok(())
            });
        tokio::time::timeout(Duration::from_secs(seconds), connection)
            .await
            .context("ACP operation timed out; the owned subprocess is terminated by the SDK")??;
        Ok::<(), anyhow::Error>(())
    })?;
    let result = report.lock().unwrap().clone();
    Ok(result)
}

fn locate(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        if !dir.is_absolute() {
            continue;
        }
        let path = dir.join(name);
        if executable(&path) {
            return Some(path);
        }
        #[cfg(windows)]
        for suffix in ["exe", "cmd", "bat"] {
            let path = dir.join(format!("{name}.{suffix}"));
            if executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
