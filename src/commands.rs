use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};

use crate::model::{PairingGrant, SyncReport};
use crate::p2p;
use crate::store::Store;

#[derive(Debug, Parser)]
#[command(
    name = "workmesh",
    version,
    about = "Local-first P2P checkpoints for agent work",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print a one-time join command and share this workspace with a peer.
    Share {
        #[arg(long)]
        direct_only: bool,
    },
    /// Join a workspace using the ticket printed by `workmesh share`.
    Join {
        ticket: String,
        #[arg(long, default_value = ".")]
        target: PathBuf,
    },
    /// Start automatic checkpoints and background synchronization.
    Start {
        #[arg(long, default_value_t = 10)]
        interval_secs: u64,
    },
    /// Stop the background daemon.
    Stop,
    /// Synchronize missing objects with all paired peers now.
    Sync {
        #[arg(long)]
        direct_only: bool,
        #[arg(long)]
        peer: Option<String>,
    },
    /// Show workspace, peer, and daemon status.
    Status,
    /// Show checkpoint history, or agent events with --events.
    Log {
        #[arg(long)]
        events: bool,
    },
    /// Initialize manually; `workmesh share` also does this automatically.
    #[command(hide = true)]
    Init,
    /// Capture the current files and agent intent without creating a Git commit.
    #[command(hide = true)]
    Checkpoint {
        #[arg(long)]
        intent: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        agent: Option<String>,
    },
    /// Append a prompt, tool call, decision, test result, or note to the agent event log.
    #[command(hide = true)]
    Record {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        checkpoint: Option<String>,
    },
    /// Accept synchronization requests from paired peers in the foreground.
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        direct_only: bool,
    },
    /// Materialize a checkpoint into a new or empty directory.
    #[command(hide = true)]
    Export {
        checkpoint: String,
        #[arg(long)]
        target: PathBuf,
    },
    /// Low-level daemon controls.
    #[command(hide = true)]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Run in the foreground.
    Run {
        #[arg(long, default_value_t = 10)]
        interval_secs: u64,
    },
    /// Start a detached background daemon.
    Start {
        #[arg(long, default_value_t = 10)]
        interval_secs: u64,
    },
    /// Stop the background daemon.
    Stop,
    /// Show whether the daemon is running.
    Status,
}

pub async fn run(cli: Cli) -> Result<()> {
    let current = std::env::current_dir()?;
    match cli.command {
        Command::Init => {
            let store = Store::init(&current)?;
            println!("Initialized workmesh in {}", store.root().display());
            println!("Repository: {}", store.config().repo_id);
            println!("Device: {}", store.config().device_id);
            Ok(())
        }
        Command::Checkpoint {
            intent,
            note,
            agent,
        } => {
            let mut store = open_or_init(&current)?;
            let (hash, capture) = store.checkpoint(intent, note, agent, false)?;
            print_capture(&hash, &capture);
            Ok(())
        }
        Command::Record {
            kind,
            text,
            agent,
            checkpoint,
        } => {
            let mut store = open_or_init(&current)?;
            let checkpoint = checkpoint
                .map(|prefix| resolve_checkpoint(&store, &prefix))
                .transpose()?;
            let hash = store.record_event(kind, agent, text, checkpoint)?;
            println!("Event {}", short(&hash));
            Ok(())
        }
        Command::Share { direct_only } => {
            let mut store = open_or_init(&current)?;
            let (hash, capture) = store.checkpoint(None, None, None, true)?;
            print_capture(&hash, &capture);
            store.config_mut().pairing = Some(PairingGrant {
                token: Store::random_token(),
                created_at: Utc::now(),
            });
            store.save_config()?;
            let result = p2p::share(&mut store, direct_only).await;
            store.config_mut().pairing = None;
            store.save_config()?;
            let report = result?;
            print_report(&report);
            println!("Run `workmesh start` on both machines for automatic synchronization.");
            Ok(())
        }
        Command::Join { ticket, target } => {
            let ticket = p2p::decode_ticket(&ticket)?;
            ensure_join_target(&target)?;
            let mut store = Store::init(&target)?;
            store.config_mut().repo_id = ticket.repo_id.clone();
            store.save_config()?;
            let report = p2p::join(&mut store, &ticket).await?;
            let files = store.export(&ticket.checkpoint, &target)?;
            store.set_local_head(&ticket.checkpoint)?;
            print_report(&report);
            println!("Materialized {files} files in {}", store.root().display());
            println!("Run `workmesh start` on both machines for automatic synchronization.");
            Ok(())
        }
        Command::Start { interval_secs } => {
            let root = current.clone();
            tokio::task::spawn_blocking(move || {
                crate::daemon::start(&root, Duration::from_secs(interval_secs))
            })
            .await?
        }
        Command::Stop => {
            let root = current.clone();
            tokio::task::spawn_blocking(move || crate::daemon::stop(&root)).await?
        }
        Command::Serve { direct_only } => {
            let mut store = Store::open(&current)?;
            p2p::serve(&mut store, direct_only).await
        }
        Command::Sync { direct_only, peer } => {
            let mut store = Store::open(&current)?;
            let peers = store
                .config()
                .peers
                .iter()
                .filter(|(id, _)| peer.as_ref().is_none_or(|wanted| *id == wanted))
                .map(|(id, peer)| (id.clone(), peer.clone()))
                .collect::<Vec<_>>();
            if peers.is_empty() {
                bail!("no matching paired peers");
            }
            let mut failures = Vec::new();
            for (id, peer) in peers {
                match p2p::sync_peer(
                    &mut store,
                    &peer.address,
                    &peer.token,
                    direct_only || peer.direct_only,
                )
                .await
                {
                    Ok(report) => {
                        println!("Synced with {id}");
                        print_report(&report);
                        if let Some(head) = report.remote_head
                            && store.apply_fast_forward(&head)?
                        {
                            println!("Fast-forwarded workspace to {}", short(&head));
                        }
                    }
                    Err(error) => {
                        eprintln!("Could not sync with {id}: {error:#}");
                        failures.push(id);
                    }
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                bail!(
                    "{} peer(s) unavailable; local checkpoints are safe",
                    failures.len()
                )
            }
        }
        Command::Log { events } => {
            let store = Store::open(&current)?;
            if events {
                for (hash, event) in store.events()? {
                    println!(
                        "{} {} {} {} [{}]",
                        short(&hash),
                        event.created_at.format("%Y-%m-%d %H:%M:%S"),
                        short(&event.device_id),
                        event.kind,
                        event.agent.as_deref().unwrap_or("-")
                    );
                    for line in event.text.lines() {
                        println!("  {line}");
                    }
                    if let Some(checkpoint) = event.checkpoint {
                        println!("  checkpoint: {}", short(&checkpoint));
                    }
                }
                return Ok(());
            }
            for (hash, checkpoint) in store.checkpoints()? {
                println!(
                    "{} {} {}{}",
                    short(&hash),
                    checkpoint.created_at.format("%Y-%m-%d %H:%M:%S"),
                    short(&checkpoint.device_id),
                    checkpoint
                        .intent
                        .as_ref()
                        .map(|intent| format!("  {intent}"))
                        .unwrap_or_default()
                );
                if let Some(note) = checkpoint.note {
                    println!("  note: {note}");
                }
                if let Some(agent) = checkpoint.agent {
                    println!("  agent: {agent}");
                }
                if let Some(git_head) = checkpoint.git_head {
                    println!("  git: {}", short(&git_head));
                }
            }
            Ok(())
        }
        Command::Status => {
            let store = Store::open(&current)?;
            println!("Repository: {}", store.config().repo_id);
            println!("Device: {}", store.config().device_id);
            println!(
                "Workspace head: {}",
                store
                    .config()
                    .workspace_head
                    .as_deref()
                    .map(short)
                    .unwrap_or_else(|| "none".to_owned())
            );
            println!("Objects: {}", store.list_objects()?.len());
            println!("Checkpoints: {}", store.checkpoints()?.len());
            for (id, peer) in &store.config().peers {
                println!(
                    "Peer {}: head={}, last_sync={}",
                    short(id),
                    peer.head
                        .as_deref()
                        .map(short)
                        .unwrap_or_else(|| "none".into()),
                    peer.last_sync
                        .map(|time| time.to_rfc3339())
                        .unwrap_or_else(|| "never".into())
                );
            }
            let root = current.clone();
            tokio::task::spawn_blocking(move || crate::daemon::status(&root)).await??;
            Ok(())
        }
        Command::Export { checkpoint, target } => {
            let store = Store::open(&current)?;
            let checkpoint = resolve_checkpoint(&store, &checkpoint)?;
            let files = store.export(&checkpoint, &target)?;
            println!("Exported {files} files to {}", target.display());
            Ok(())
        }
        Command::Daemon { command } => {
            let root = current.clone();
            match command {
                DaemonCommand::Run { interval_secs } => {
                    tokio::task::spawn_blocking(move || {
                        crate::daemon::run(&root, Duration::from_secs(interval_secs))
                    })
                    .await?
                }
                DaemonCommand::Start { interval_secs } => {
                    tokio::task::spawn_blocking(move || {
                        crate::daemon::start(&root, Duration::from_secs(interval_secs))
                    })
                    .await?
                }
                DaemonCommand::Stop => {
                    tokio::task::spawn_blocking(move || crate::daemon::stop(&root)).await?
                }
                DaemonCommand::Status => {
                    tokio::task::spawn_blocking(move || crate::daemon::status(&root)).await?
                }
            }
        }
    }
}

fn open_or_init(root: &Path) -> Result<Store> {
    if root.join(".workmesh/config.json").is_file() {
        Store::open(root)
    } else {
        Store::init(root)
    }
}

fn ensure_join_target(target: &PathBuf) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(target).with_context(|| format!("reading {}", target.display()))? {
        if entry?.file_name() != ".workmesh" {
            bail!("join target must be new or empty: {}", target.display());
        }
    }
    Ok(())
}

fn resolve_checkpoint(store: &Store, prefix: &str) -> Result<String> {
    if prefix.len() < 8 {
        bail!("checkpoint prefix must contain at least 8 characters");
    }
    let matches = store
        .checkpoints()?
        .into_iter()
        .map(|(hash, _)| hash)
        .filter(|hash| hash.starts_with(prefix))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => bail!("checkpoint not found: {prefix}"),
        1 => Ok(matches[0].clone()),
        _ => bail!("checkpoint prefix is ambiguous: {prefix}"),
    }
}

fn print_capture(hash: &str, capture: &crate::snapshot::Capture) {
    println!(
        "Checkpoint {} ({} files, {} skipped)",
        short(hash),
        capture.file_count,
        capture.skipped.len()
    );
    for skipped in &capture.skipped {
        eprintln!("Skipped {skipped}");
    }
}

fn print_report(report: &SyncReport) {
    println!(
        "Synchronized: {} downloaded, {} uploaded",
        report.downloaded, report.uploaded
    );
}

fn short(value: &str) -> String {
    value.chars().take(12).collect()
}
