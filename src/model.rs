use std::collections::BTreeMap;
use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub repo_id: String,
    pub device_id: String,
    pub device_name: String,
    pub secret_key: String,
    #[serde(default)]
    pub listen_port: Option<u16>,
    pub workspace_head: Option<String>,
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    pub pairing: Option<PairingGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub device_id: String,
    pub address: WireAddress,
    pub token: String,
    pub head: Option<String>,
    #[serde(default)]
    pub event_head: Option<String>,
    pub last_sync: Option<DateTime<Utc>>,
    #[serde(default)]
    pub direct_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingGrant {
    pub token: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireAddress {
    pub id: String,
    pub relay: Option<String>,
    #[serde(default)]
    pub addrs: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub version: u32,
    pub repo_id: String,
    pub checkpoint: String,
    pub address: WireAddress,
    pub token: String,
    pub direct_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    pub parents: Vec<String>,
    pub device_id: String,
    pub created_at: DateTime<Utc>,
    pub intent: Option<String>,
    pub note: Option<String>,
    pub agent: Option<String>,
    pub git_head: Option<String>,
    pub manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub version: u32,
    pub parents: Vec<String>,
    pub device_id: String,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub agent: Option<String>,
    pub text: String,
    pub checkpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub repo_id: String,
    pub device_id: String,
    pub token: String,
    pub address: WireAddress,
    pub head: Option<String>,
    pub event_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    pub version: u32,
    pub device_id: String,
    pub address: WireAddress,
    pub head: Option<String>,
    pub event_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub objects: Vec<String>,
    pub head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectRequest {
    pub hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectBatch {
    pub objects: Vec<WireObject>,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireObject {
    pub hash: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub downloaded: usize,
    pub uploaded: usize,
    pub remote_head: Option<String>,
}
