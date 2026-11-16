use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::p2p;
use crate::store::Store;

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonStatusFile {
    pub repo_id: String,
    pub pid: u32,
}

pub fn run(root: &Path, interval: Duration) -> Result<()> {
    let mut store = Store::open(root)?;
    let state = store.state_dir().to_path_buf();
    fs::create_dir_all(&state)?;
    let lock = File::create(state.join("daemon.lock")).context("create daemon lock")?;
    lock.try_lock_exclusive()
        .context("another aigit daemon already runs for this workspace")?;
    write_status(&state, &store)?;
    let result = tokio::runtime::Handle::try_current();
    let outcome = match result {
        Ok(handle) => handle.block_on(p2p::daemon_loop(&mut store, interval)),
        Err(_) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("build tokio runtime")?
            .block_on(p2p::daemon_loop(&mut store, interval)),
    };
    let _ = fs::remove_file(state.join("daemon.status"));
    lock.unlock().ok();
    outcome
}

pub fn start(root: &Path, interval: Duration) -> Result<()> {
    let store = Store::open(root)?;
    let state = store.state_dir().to_path_buf();
    if let Some(status) = read_status(&state)
        && is_running(status.pid)
    {
        bail!("aigit daemon is already running (pid {})", status.pid);
    }
    fs::remove_file(state.join("daemon.status")).ok();
    let executable = std::env::current_exe().context("locate aigit executable")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("daemon.log"))
        .context("open daemon log")?;
    let mut command = Command::new(executable);
    command
        .arg("daemon")
        .arg("run")
        .arg("--interval-secs")
        .arg(interval.as_secs().to_string())
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().context("spawn aigit daemon")?;
    let pid = child.id();
    drop(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = read_status(&state)
            && status.pid == pid
            && is_running(pid)
        {
            println!(
                "Started aigit daemon (pid {pid}). Log: {}",
                state.join("daemon.log").display()
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "daemon did not become ready; inspect {}",
        state.join("daemon.log").display()
    );
}

pub fn stop(root: &Path) -> Result<()> {
    let store = Store::open(root)?;
    let state = store.state_dir().to_path_buf();
    let Some(status) = read_status(&state) else {
        println!("No aigit daemon is running.");
        return Ok(());
    };
    if !is_running(status.pid) {
        fs::remove_file(state.join("daemon.status")).ok();
        println!("Removed stale daemon status.");
        return Ok(());
    }
    signal(status.pid, false)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !is_running(status.pid) {
            fs::remove_file(state.join("daemon.status")).ok();
            println!("Stopped aigit daemon (pid {}).", status.pid);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    signal(status.pid, true)?;
    fs::remove_file(state.join("daemon.status")).ok();
    println!("Force-stopped aigit daemon (pid {}).", status.pid);
    Ok(())
}

pub fn status(root: &Path) -> Result<()> {
    let store = Store::open(root)?;
    let state = store.state_dir().to_path_buf();
    match read_status(&state) {
        Some(status) if is_running(status.pid) => {
            println!("running pid={} repo={}", status.pid, status.repo_id);
            println!("log={}", state.join("daemon.log").display());
        }
        Some(_) => {
            println!("stopped (stale status)");
            println!("log={}", state.join("daemon.log").display());
        }
        None => println!("stopped"),
    }
    Ok(())
}

pub fn log_path(root: &Path) -> PathBuf {
    root.join(".aigit/daemon.log")
}

fn write_status(state: &Path, store: &Store) -> Result<()> {
    let value = DaemonStatusFile {
        repo_id: store.config().repo_id.clone(),
        pid: std::process::id(),
    };
    let mut file = File::create(state.join("daemon.status"))?;
    writeln!(file, "{}", serde_json::to_string(&value)?)?;
    Ok(())
}

fn read_status(state: &Path) -> Option<DaemonStatusFile> {
    let bytes = fs::read(state.join("daemon.status")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn signal(pid: u32, _force: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let argument = if _force { "-9" } else { "-15" };
        Command::new("kill")
            .args([argument, &pid.to_string()])
            .status()
            .with_context(|| format!("signal daemon {pid}"))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .with_context(|| format!("stop daemon {pid}"))?;
        Ok(())
    }
}
