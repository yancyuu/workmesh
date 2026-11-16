use std::collections::BTreeSet;
use std::io::{self, Write};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::Utc;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, TransportAddr};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::model::{
    Hello, HelloAck, Inventory, ObjectBatch, ObjectRequest, Peer, SyncReport, Ticket, WireAddress,
    WireObject,
};
use crate::store::Store;
use crate::{ALPN, FORMAT_VERSION, MAX_FRAME_SIZE};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const SYNC_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn share(
    store: &mut Store,
    direct_only: bool,
    confirm_pairing: bool,
    ticket_out: Option<&std::path::Path>,
    qr: bool,
) -> Result<SyncReport> {
    let endpoint = bind_endpoint(store, direct_only).await?;
    let address = current_address(&endpoint, direct_only).await?;
    let pairing = store
        .config()
        .pairing
        .clone()
        .context("missing pairing grant")?;
    let ticket = Ticket {
        version: FORMAT_VERSION,
        repo_id: store.config().repo_id.clone(),
        checkpoint: store.local_head().context("no checkpoint to share")?,
        address,
        token: pairing.token,
        direct_only,
    };
    let encoded = encode_ticket(&ticket)?;
    let command = format!("workmesh join {encoded}");
    println!("{command}");
    if let Some(path) = ticket_out {
        write_ticket_file(path, &encoded)?;
        println!("Ticket written to {}; delete it after use.", path.display());
    }
    if qr {
        println!("{}", qrcode::QrCode::new(command)?.render::<char>().build());
        println!("Scan this QR code only over a trusted path; it can request one workspace join.");
    }
    println!("Waiting for one peer; press Ctrl-C to cancel.");
    let report = accept_loop(store, &endpoint, direct_only, true, confirm_pairing).await;
    endpoint.close().await;
    report
}

pub async fn serve(store: &mut Store, direct_only: bool) -> Result<()> {
    let endpoint = bind_endpoint(store, direct_only).await?;
    println!("Serving {} as {}", store.config().repo_id, endpoint.id());
    println!("Press Ctrl-C to stop.");
    let result = accept_loop(store, &endpoint, direct_only, false, false).await;
    endpoint.close().await;
    result.map(|_| ())
}

pub async fn daemon_loop(store: &mut Store, interval: Duration) -> Result<()> {
    let direct_only = {
        let peers = &store.config().peers;
        !peers.is_empty() && peers.values().all(|peer| peer.direct_only)
    };
    let endpoint = bind_endpoint(store, direct_only).await?;
    let local_endpoint_id = endpoint.id().to_string();
    eprintln!("Daemon ready as {local_endpoint_id}");
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let incoming = incoming.context("endpoint closed")?;
                let connection = incoming.await.context("failed to accept peer")?;
                match timeout(SYNC_TIMEOUT, run_server(store, &endpoint, &connection, direct_only, false)).await {
                    Ok(Ok(report)) => {
                        connection.close(0u32.into(), b"done");
                        if let Some(head) = &report.remote_head
                            && store.apply_fast_forward(head)?
                        {
                            eprintln!("Fast-forwarded workspace to {}", &head[..12.min(head.len())]);
                        }
                    }
                    Ok(Err(error)) => eprintln!("Incoming sync failed: {error:#}"),
                    Err(_) => eprintln!("Incoming sync timed out"),
                }
            }
            _ = ticker.tick() => {
                if let Some(after) = store.checkpoint_if_changed()? {
                    eprintln!("Auto checkpoint {}", &after[..12.min(after.len())]);
                }
                let peers = store.config().peers.clone();
                for (id, peer) in peers {
                    if local_endpoint_id > id {
                        continue;
                    }
                    match sync_peer_on_endpoint(store, &endpoint, &peer.address, &peer.token, direct_only || peer.direct_only).await {
                        Ok(report) => {
                            eprintln!("Synced with {}", &id[..12.min(id.len())]);
                            if let Some(head) = report.remote_head
                                && store.apply_fast_forward(&head)?
                            {
                                eprintln!("Fast-forwarded workspace to {}", &head[..12.min(head.len())]);
                            }
                        }
                        Err(error) => eprintln!("Sync retry with {} failed: {error:#}", &id[..12.min(id.len())]),
                    }
                }
            }
        }
    }
}

pub async fn join(store: &mut Store, ticket: &Ticket) -> Result<SyncReport> {
    if ticket.version != FORMAT_VERSION {
        bail!("unsupported pairing ticket version: {}", ticket.version);
    }
    if store.config().repo_id != ticket.repo_id {
        bail!("pairing ticket belongs to a different repository");
    }
    let peer = Peer {
        device_id: String::new(),
        address: ticket.address.clone(),
        token: ticket.token.clone(),
        head: Some(ticket.checkpoint.clone()),
        event_head: None,
        last_sync: None,
        direct_only: ticket.direct_only,
    };
    store.set_peer(ticket.address.id.clone(), peer)?;
    sync_peer(store, &ticket.address, &ticket.token, ticket.direct_only).await
}

pub async fn sync_peer(
    store: &mut Store,
    address: &WireAddress,
    token: &str,
    direct_only: bool,
) -> Result<SyncReport> {
    let endpoint = bind_endpoint(store, direct_only).await?;
    let result = sync_peer_on_endpoint(store, &endpoint, address, token, direct_only).await;
    endpoint.close().await;
    result
}

pub async fn sync_peer_on_endpoint(
    store: &mut Store,
    endpoint: &Endpoint,
    address: &WireAddress,
    token: &str,
    direct_only: bool,
) -> Result<SyncReport> {
    let remote = address.to_endpoint_addr()?;
    let connection = timeout(CONNECT_TIMEOUT, endpoint.connect(remote, ALPN))
        .await
        .context("timed out connecting to peer")??;
    let result = timeout(
        SYNC_TIMEOUT,
        run_client(store, endpoint, &connection, address, token, direct_only),
    )
    .await
    .context("synchronization timed out")?;
    connection.close(0u32.into(), b"done");
    result
}

pub fn decode_ticket(value: &str) -> Result<Ticket> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .context("invalid pairing ticket encoding")?;
    serde_json::from_slice(&bytes).context("invalid pairing ticket")
}

fn encode_ticket(ticket: &Ticket) -> Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(ticket)?))
}

fn write_ticket_file(path: &std::path::Path, ticket: &str) -> Result<()> {
    use std::fs::OpenOptions;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create ticket file {}", path.display()))?;
    writeln!(file, "{ticket}")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

async fn bind_endpoint(store: &mut Store, direct_only: bool) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0)
        .secret_key(store.secret_key()?)
        .alpns(vec![ALPN.to_vec()]);
    if direct_only {
        builder = builder
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup();
        if let Some(port) = store.config().listen_port {
            let address = std::net::SocketAddr::from(([0, 0, 0, 0], port));
            builder = builder.bind_addr(address)?;
        }
    }
    let endpoint = builder
        .bind()
        .await
        .context("failed to bind P2P endpoint")?;
    if direct_only && store.config().listen_port.is_none() {
        let port = endpoint
            .addr()
            .addrs
            .iter()
            .find_map(|address| match address {
                TransportAddr::Ip(socket) => Some(socket.port()),
                _ => None,
            })
            .context("read direct listen port")?;
        store.config_mut().listen_port = Some(port);
        store.save_config()?;
    }
    Ok(endpoint)
}

async fn current_address(endpoint: &Endpoint, direct_only: bool) -> Result<WireAddress> {
    if !direct_only
        && timeout(Duration::from_secs(20), endpoint.online())
            .await
            .is_err()
    {
        eprintln!("Relay unavailable; advertising direct addresses only.");
    }
    WireAddress::from_endpoint_addr(endpoint.addr())
}

async fn accept_loop(
    store: &mut Store,
    endpoint: &Endpoint,
    direct_only: bool,
    once: bool,
    confirm_pairing: bool,
) -> Result<SyncReport> {
    loop {
        let incoming = endpoint.accept().await.context("endpoint closed")?;
        let connection = incoming.await.context("failed to accept peer")?;
        match timeout(
            SYNC_TIMEOUT,
            run_server(store, endpoint, &connection, direct_only, confirm_pairing),
        )
        .await
        {
            Ok(Ok(report)) => {
                connection.close(0u32.into(), b"done");
                if once {
                    return Ok(report);
                }
            }
            Ok(Err(error)) => eprintln!("Peer sync failed: {error:#}"),
            Err(_) => eprintln!("Peer sync timed out"),
        }
    }
}

async fn run_client(
    store: &mut Store,
    endpoint: &Endpoint,
    connection: &iroh::endpoint::Connection,
    server_address: &WireAddress,
    token: &str,
    direct_only: bool,
) -> Result<SyncReport> {
    let (mut send, mut recv) = connection.open_bi().await?;
    let local_address = current_address(endpoint, direct_only).await?;
    send_json(
        &mut send,
        &Hello {
            version: FORMAT_VERSION,
            repo_id: store.config().repo_id.clone(),
            device_id: store.config().device_id.clone(),
            device_name: store.config().device_name.clone(),
            token: token.to_owned(),
            address: local_address,
            head: store.local_head(),
            event_head: store.event_head(),
        },
    )
    .await?;
    let ack: HelloAck = recv_json(&mut recv).await?;
    if ack.version != FORMAT_VERSION || ack.address.id != server_address.id {
        bail!("peer returned an invalid handshake");
    }
    store.set_peer(
        server_address.id.clone(),
        Peer {
            device_id: ack.device_id.clone(),
            address: ack.address.clone(),
            token: ack.session_token.clone(),
            head: None,
            event_head: ack.event_head.clone(),
            last_sync: None,
            direct_only,
        },
    )?;

    let local_inventory = Inventory {
        objects: store.list_objects()?,
        head: store.local_head(),
    };
    send_json(&mut send, &local_inventory).await?;
    let remote_inventory: Inventory = recv_json(&mut recv).await?;
    let wanted = missing(store, &remote_inventory.objects)?;
    send_json(&mut send, &ObjectRequest { hashes: wanted }).await?;
    let downloaded = receive_batches(store, &mut recv).await?;

    let request: ObjectRequest = recv_json(&mut recv).await?;
    let uploaded = send_batches(store, &mut send, &request.hashes).await?;
    send.finish()?;

    store.set_peer(
        server_address.id.clone(),
        Peer {
            device_id: ack.device_id,
            address: ack.address,
            token: ack.session_token,
            head: remote_inventory.head.clone(),
            event_head: ack.event_head,
            last_sync: Some(Utc::now()),
            direct_only,
        },
    )?;
    Ok(SyncReport {
        downloaded,
        uploaded,
        remote_head: remote_inventory.head,
    })
}

async fn run_server(
    store: &mut Store,
    endpoint: &Endpoint,
    connection: &iroh::endpoint::Connection,
    direct_only: bool,
    confirm_pairing: bool,
) -> Result<SyncReport> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let hello: Hello = recv_json(&mut recv).await?;
    let remote_id = connection.remote_id().to_string();
    if hello.version != FORMAT_VERSION
        || hello.repo_id != store.config().repo_id
        || hello.address.id != remote_id
    {
        bail!("peer handshake failed repository or identity validation");
    }
    let paired = store
        .config()
        .peers
        .get(&remote_id)
        .is_some_and(|peer| peer.token == hello.token);
    let pairing = store.config().pairing.as_ref().is_some_and(|grant| {
        let age = Utc::now()
            .signed_duration_since(grant.created_at)
            .num_seconds();
        grant.token == hello.token && (-60..=600).contains(&age)
    });
    if !paired && !pairing {
        bail!("peer authentication failed");
    }
    if pairing && confirm_pairing && !confirm_new_peer(&hello, &remote_id)? {
        bail!("pairing request rejected");
    }
    let session_token = if pairing {
        Store::random_token()
    } else {
        hello.token.clone()
    };
    store.set_peer(
        remote_id.clone(),
        Peer {
            device_id: hello.device_id,
            address: hello.address,
            token: session_token.clone(),
            head: hello.head,
            event_head: hello.event_head,
            last_sync: None,
            direct_only,
        },
    )?;

    send_json(
        &mut send,
        &HelloAck {
            version: FORMAT_VERSION,
            device_id: store.config().device_id.clone(),
            address: current_address(endpoint, direct_only).await?,
            head: store.local_head(),
            event_head: store.event_head(),
            session_token,
        },
    )
    .await?;
    let remote_inventory: Inventory = recv_json(&mut recv).await?;
    let local_inventory = Inventory {
        objects: store.list_objects()?,
        head: store.local_head(),
    };
    send_json(&mut send, &local_inventory).await?;

    let request: ObjectRequest = recv_json(&mut recv).await?;
    let uploaded = send_batches(store, &mut send, &request.hashes).await?;
    let wanted = missing(store, &remote_inventory.objects)?;
    send_json(&mut send, &ObjectRequest { hashes: wanted }).await?;
    let downloaded = receive_batches(store, &mut recv).await?;
    send.finish()?;

    if let Some(peer) = store.config_mut().peers.get_mut(&remote_id) {
        peer.head = remote_inventory.head.clone();
        peer.last_sync = Some(Utc::now());
        peer.direct_only = direct_only;
    }
    store.save_config()?;
    if let Some(head) = &remote_inventory.head {
        store.apply_fast_forward(head)?;
    }
    Ok(SyncReport {
        downloaded,
        uploaded,
        remote_head: remote_inventory.head,
    })
}

fn confirm_new_peer(hello: &Hello, remote_id: &str) -> Result<bool> {
    println!("Pairing request:");
    println!("  Device: {} ({})", hello.device_name, hello.device_id);
    println!("  Endpoint: {remote_id}");
    print!("Allow this device to join this workspace? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

fn missing(store: &Store, remote: &[String]) -> Result<Vec<String>> {
    let local = store.list_objects()?.into_iter().collect::<BTreeSet<_>>();
    Ok(remote
        .iter()
        .filter(|hash| !local.contains(*hash))
        .cloned()
        .collect())
}

async fn send_batches(
    store: &Store,
    send: &mut iroh::endpoint::SendStream,
    hashes: &[String],
) -> Result<usize> {
    const BATCH_BYTES: usize = 8 * 1024 * 1024;
    let mut count = 0;
    let mut objects = Vec::new();
    let mut batch_size = 0;
    for hash in hashes {
        let bytes = store.get_object(hash)?;
        let data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        if !objects.is_empty() && batch_size + data_base64.len() > BATCH_BYTES {
            send_json(
                send,
                &ObjectBatch {
                    objects,
                    done: false,
                },
            )
            .await?;
            objects = Vec::new();
            batch_size = 0;
        }
        batch_size += data_base64.len();
        objects.push(WireObject {
            hash: hash.clone(),
            data_base64,
        });
        count += 1;
    }
    send_json(
        send,
        &ObjectBatch {
            objects,
            done: true,
        },
    )
    .await?;
    Ok(count)
}

async fn receive_batches(store: &Store, recv: &mut iroh::endpoint::RecvStream) -> Result<usize> {
    let mut count = 0;
    loop {
        let batch: ObjectBatch = recv_json(recv).await?;
        for object in batch.objects {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(object.data_base64)
                .context("invalid object encoding")?;
            store.put_verified_object(&object.hash, &bytes)?;
            count += 1;
        }
        if batch.done {
            return Ok(count);
        }
    }
}

async fn send_json<S, T>(stream: &mut S, value: &T) -> Result<()>
where
    S: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_SIZE {
        bail!("protocol frame is too large: {} bytes", bytes.len());
    }
    let length = u32::try_from(bytes.len()).context("protocol frame exceeds u32")?;
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn recv_json<S, T>(stream: &mut S) -> Result<T>
where
    S: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = stream.read_u32().await? as usize;
    if length > MAX_FRAME_SIZE {
        bail!("peer sent oversized protocol frame: {length} bytes");
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).context("invalid protocol message")
}

impl WireAddress {
    fn from_endpoint_addr(address: EndpointAddr) -> Result<Self> {
        let mut relay = None;
        let mut addrs = Vec::new();
        for transport in address.addrs {
            match transport {
                TransportAddr::Relay(url) => relay = Some(url.to_string()),
                TransportAddr::Ip(addr) => addrs.push(addr),
                _ => {}
            }
        }
        Ok(Self {
            id: address.id.to_string(),
            relay,
            addrs,
        })
    }

    fn to_endpoint_addr(&self) -> Result<EndpointAddr> {
        let mut addrs = BTreeSet::new();
        if let Some(relay) = &self.relay {
            addrs.insert(TransportAddr::Relay(RelayUrl::from_str(relay)?));
        }
        for addr in &self.addrs {
            addrs.insert(TransportAddr::Ip(*addr));
        }
        Ok(EndpointAddr {
            id: EndpointId::from_str(&self.id)?,
            addrs,
        })
    }
}
