use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::Utc;
use iroh::SecretKey;
use rand::Rng;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::FORMAT_VERSION;
use crate::model::{AgentEvent, Checkpoint, Config, Peer};
use crate::snapshot::{self, Capture};

pub struct Store {
    root: PathBuf,
    state: PathBuf,
    config: Config,
}

impl Store {
    pub fn init(root: &Path) -> Result<Self> {
        let root = absolute(root)?;
        fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        let state = root.join(".workmesh");
        if state.join("config.json").exists() {
            bail!("workmesh is already initialized in {}", root.display());
        }
        fs::create_dir_all(state.join("objects"))?;
        fs::create_dir_all(state.join("refs/heads"))?;
        set_private_dir(&state)?;
        let device_id = Uuid::new_v4().simple().to_string();
        let secret_key = SecretKey::from_bytes(&rand::rng().random::<[u8; 32]>());
        let config = Config {
            version: FORMAT_VERSION,
            repo_id: Uuid::new_v4().simple().to_string(),
            device_name: format!("device-{}", &device_id[..8]),
            device_id,
            secret_key: hex::encode(secret_key.to_bytes()),
            listen_port: None,
            workspace_head: None,
            peers: BTreeMap::new(),
            pairing: None,
        };
        let store = Self {
            root,
            state,
            config,
        };
        store.save_config()?;
        store.exclude_from_git()?;
        Ok(store)
    }

    pub fn open(root: &Path) -> Result<Self> {
        let root = absolute(root)?;
        let state = root.join(".workmesh");
        let bytes = fs::read(state.join("config.json")).with_context(|| {
            format!(
                "no workmesh repository at {}; run workmesh init first",
                root.display()
            )
        })?;
        let config: Config =
            serde_json::from_slice(&bytes).context("invalid .workmesh/config.json")?;
        if config.version != FORMAT_VERSION {
            bail!("unsupported repository format: {}", config.version);
        }
        Ok(Self {
            root,
            state,
            config,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    pub fn save_config(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.config)?;
        let path = self.state.join("config.json");
        atomic_write(&path, &bytes)?;
        set_private_file(&path)
    }

    pub fn secret_key(&self) -> Result<SecretKey> {
        let bytes = hex::decode(&self.config.secret_key).context("invalid endpoint secret key")?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("endpoint secret key must contain 32 bytes"))?;
        Ok(SecretKey::from_bytes(&bytes))
    }

    pub fn put_object(&self, bytes: &[u8]) -> Result<String> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let path = self.object_path(&hash)?;
        if path.exists() {
            return Ok(hash);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&path, bytes)?;
        Ok(hash)
    }

    pub fn has_object(&self, hash: &str) -> bool {
        self.object_path(hash).is_ok_and(|path| path.is_file())
    }

    pub fn get_object(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.object_path(hash)?;
        let bytes = fs::read(&path).with_context(|| format!("missing object {hash}"))?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != hash {
            bail!("object integrity check failed for {hash}");
        }
        Ok(bytes)
    }

    pub fn put_verified_object(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        let actual = blake3::hash(bytes).to_hex().to_string();
        if actual != hash {
            bail!("peer sent object with invalid hash: expected {hash}, got {actual}");
        }
        self.put_object(bytes)?;
        Ok(())
    }

    pub fn list_objects(&self) -> Result<Vec<String>> {
        let mut objects = Vec::new();
        for prefix in fs::read_dir(self.state.join("objects"))? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for suffix in fs::read_dir(prefix.path())? {
                let suffix = suffix?;
                if !suffix.file_type()?.is_file() {
                    continue;
                }
                let hash = format!(
                    "{}{}",
                    prefix.file_name().to_string_lossy(),
                    suffix.file_name().to_string_lossy()
                );
                if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    objects.push(hash);
                }
            }
        }
        objects.sort();
        Ok(objects)
    }

    pub fn put_json<T: Serialize>(&self, value: &T) -> Result<String> {
        self.put_object(&serde_json::to_vec(value)?)
    }

    pub fn get_json<T: DeserializeOwned>(&self, hash: &str) -> Result<T> {
        serde_json::from_slice(&self.get_object(hash)?)
            .with_context(|| format!("invalid JSON object {hash}"))
    }

    pub fn get_checkpoint(&self, hash: &str) -> Result<Checkpoint> {
        let checkpoint: Checkpoint = self.get_json(hash)?;
        if checkpoint.version != FORMAT_VERSION {
            bail!(
                "unsupported checkpoint format in {hash}: {}",
                checkpoint.version
            );
        }
        Ok(checkpoint)
    }

    pub fn local_head(&self) -> Option<String> {
        fs::read_to_string(self.state.join("refs/heads").join(&self.config.device_id))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    pub fn set_local_head(&mut self, hash: &str) -> Result<()> {
        atomic_write(
            &self.state.join("refs/heads").join(&self.config.device_id),
            format!("{hash}\n").as_bytes(),
        )?;
        self.config.workspace_head = Some(hash.to_owned());
        self.save_config()
    }

    pub fn checkpoint(
        &mut self,
        intent: Option<String>,
        note: Option<String>,
        agent: Option<String>,
        reuse_clean: bool,
    ) -> Result<(String, Capture)> {
        let capture = snapshot::capture(self)?;
        if reuse_clean
            && intent.is_none()
            && note.is_none()
            && agent.is_none()
            && let Some(head) = self.local_head()
        {
            let current = self.get_checkpoint(&head)?;
            if current.manifest == capture.manifest_hash {
                return Ok((head, capture));
            }
        }
        let checkpoint = Checkpoint {
            version: FORMAT_VERSION,
            parents: self.local_head().into_iter().collect(),
            device_id: self.config.device_id.clone(),
            created_at: Utc::now(),
            intent,
            note,
            agent,
            git_head: git_head(&self.root),
            manifest: capture.manifest_hash.clone(),
        };
        let hash = self.put_json(&checkpoint)?;
        self.set_local_head(&hash)?;
        Ok((hash, capture))
    }

    pub fn checkpoint_if_changed(&mut self) -> Result<Option<String>> {
        let capture = snapshot::capture(self)?;
        let Some(head) = self.local_head() else {
            return Ok(None);
        };
        let current = self.get_checkpoint(&head)?;
        if current.manifest == capture.manifest_hash {
            return Ok(None);
        }
        let checkpoint = Checkpoint {
            version: FORMAT_VERSION,
            parents: vec![head],
            device_id: self.config.device_id.clone(),
            created_at: Utc::now(),
            intent: None,
            note: Some("automatic workspace checkpoint".to_owned()),
            agent: Some("workmesh-daemon".to_owned()),
            git_head: git_head(&self.root),
            manifest: capture.manifest_hash,
        };
        let hash = self.put_json(&checkpoint)?;
        self.set_local_head(&hash)?;
        Ok(Some(hash))
    }

    pub fn event_head(&self) -> Option<String> {
        fs::read_to_string(self.state.join("refs/events").join(&self.config.device_id))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    pub fn record_event(
        &mut self,
        kind: String,
        agent: Option<String>,
        text: String,
        checkpoint: Option<String>,
    ) -> Result<String> {
        let normalized_kind = kind.to_ascii_lowercase();
        if !matches!(
            normalized_kind.as_str(),
            "prompt" | "tool" | "decision" | "test" | "note"
        ) {
            bail!("event kind must be prompt, tool, decision, test, or note");
        }
        if text.trim().is_empty() {
            bail!("event text cannot be empty");
        }
        let event = AgentEvent {
            version: FORMAT_VERSION,
            parents: self.event_head().into_iter().collect(),
            device_id: self.config.device_id.clone(),
            created_at: Utc::now(),
            kind: normalized_kind,
            agent,
            text,
            checkpoint,
        };
        let hash = self.put_json(&event)?;
        let path = self.state.join("refs/events").join(&self.config.device_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&path, format!("{hash}\n").as_bytes())?;
        Ok(hash)
    }

    pub fn events(&self) -> Result<Vec<(String, AgentEvent)>> {
        let mut pending = Vec::new();
        if let Some(head) = self.event_head() {
            pending.push(head);
        }
        for peer in self.config.peers.values() {
            if let Some(head) = &peer.event_head {
                pending.push(head.clone());
            }
        }
        let mut seen = BTreeSet::new();
        let mut events = Vec::new();
        while let Some(hash) = pending.pop() {
            if !seen.insert(hash.clone()) || !self.has_object(&hash) {
                continue;
            }
            let event: AgentEvent = self.get_json(&hash)?;
            if event.version != FORMAT_VERSION {
                continue;
            }
            pending.extend(event.parents.clone());
            events.push((hash, event));
        }
        events.sort_by_key(|event| std::cmp::Reverse(event.1.created_at));
        Ok(events)
    }

    pub fn all_heads(&self) -> BTreeSet<String> {
        let mut heads = BTreeSet::new();
        if let Some(head) = self.local_head() {
            heads.insert(head);
        }
        for peer in self.config.peers.values() {
            if let Some(head) = &peer.head {
                heads.insert(head.clone());
            }
        }
        heads
    }

    pub fn checkpoints(&self) -> Result<Vec<(String, Checkpoint)>> {
        let mut pending = self.all_heads().into_iter().collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        let mut checkpoints = Vec::new();
        while let Some(hash) = pending.pop() {
            if !seen.insert(hash.clone()) || !self.has_object(&hash) {
                continue;
            }
            let checkpoint = self.get_checkpoint(&hash)?;
            pending.extend(checkpoint.parents.clone());
            checkpoints.push((hash, checkpoint));
        }
        checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.1.created_at));
        Ok(checkpoints)
    }

    pub fn is_descendant(&self, candidate: &str, ancestor: &str) -> Result<bool> {
        if candidate == ancestor {
            return Ok(true);
        }
        let mut pending = vec![candidate.to_owned()];
        let mut seen = BTreeSet::new();
        while let Some(hash) = pending.pop() {
            if !seen.insert(hash.clone()) || !self.has_object(&hash) {
                continue;
            }
            let checkpoint = self.get_checkpoint(&hash)?;
            if checkpoint.parents.iter().any(|parent| parent == ancestor) {
                return Ok(true);
            }
            pending.extend(checkpoint.parents);
        }
        Ok(false)
    }

    pub fn set_peer(&mut self, endpoint_id: String, peer: Peer) -> Result<()> {
        self.config.peers.insert(endpoint_id, peer);
        self.save_config()
    }

    pub fn export(&self, checkpoint: &str, target: &Path) -> Result<usize> {
        snapshot::materialize_empty(self, checkpoint, target)
    }

    pub fn apply_fast_forward(&mut self, candidate: &str) -> Result<bool> {
        let Some(current) = self.config.workspace_head.clone() else {
            return Ok(false);
        };
        if candidate == current || !self.is_descendant(candidate, &current)? {
            return Ok(false);
        }
        let capture = snapshot::capture(self)?;
        let current_checkpoint = self.get_checkpoint(&current)?;
        if capture.manifest_hash != current_checkpoint.manifest {
            return Ok(false);
        }
        snapshot::materialize_update(self, candidate, &self.root, &current)?;
        self.config.workspace_head = Some(candidate.to_owned());
        self.save_config()?;
        Ok(true)
    }

    pub fn random_token() -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::rng().random::<[u8; 24]>())
    }

    fn object_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid object hash: {hash}");
        }
        Ok(self.state.join("objects").join(&hash[..2]).join(&hash[2..]))
    }

    fn exclude_from_git(&self) -> Result<()> {
        let git = self.root.join(".git");
        if !git.is_dir() {
            return Ok(());
        }
        let info = git.join("info");
        fs::create_dir_all(&info)?;
        let exclude = info.join("exclude");
        let existing = fs::read_to_string(&exclude).unwrap_or_default();
        if !existing.lines().any(|line| line.trim() == ".workmesh/") {
            let mut updated = existing;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(".workmesh/\n");
            atomic_write(&exclude, updated.as_bytes())?;
        }
        Ok(())
    }
}

fn git_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        None
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("file has no parent directory")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".workmesh-tmp-{}", Uuid::new_v4().simple()));
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("securing {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("securing {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}
