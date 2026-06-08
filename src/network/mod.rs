use crate::types::*;
use crate::consensus::*;
use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, timeout};
use tracing::{info, warn, error, debug};
use futures::StreamExt;
use libp2p::{
    identity, PeerId, Swarm, SwarmBuilder,
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    kad::{self, store::MemoryStore},
    mdns,
    quic,
    noise,
    yamux,
    Multiaddr, Transport,
};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use serde::{Serialize, Deserialize};

/// Network message types
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NetworkMessage {
    // Consensus messages
    NewView { height: u64, round: u64, validator: Address },
    Prepare { block_hash: Hash, height: u64, round: u64, validator: Address, signature: Vec<u8> },
    PreCommit { block_hash: Hash, height: u64, round: u64, validator: Address, signature: Vec<u8> },
    Commit { block_hash: Hash, height: u64, round: u64, validator: Address, signature: Vec<u8> },

    // Block propagation
    BlockProposal { block: Block, qc: Option<QuorumCertificate> },
    BlockRequest { height: u64, shard_id: u16 },
    BlockResponse { block: Block },

    // Transaction propagation
    Transaction { tx: Transaction },
    TransactionBatch { txs: Vec<Transaction>, shard_id: u16 },

    // Cross-chain messages
    CrossChainTx { tx: CrossChainTransaction },
    CrossChainProof { tx_id: Hash, proof: Vec<u8> },

    // State sync
    StateRequest { shard_id: u16, block_height: u64 },
    StateResponse { state_root: Hash, accounts: Vec<(Address, Account)> },

    // Peer discovery
    PeerInfo { peer_id: String, address: String, shard_id: u16, validator: Option<Address> },
    Ping { timestamp: u64 },
    Pong { timestamp: u64 },
}

/// Network behavior combining multiple protocols
#[derive(NetworkBehaviour)]
pub struct ProtonNetworkBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<MemoryStore>,
    pub mdns: mdns::tokio::Behaviour,
}

/// P2P Network node
pub struct NetworkNode {
    local_peer_id: PeerId,
    swarm: RwLock<Swarm<ProtonNetworkBehaviour>>,
    connected_peers: RwLock<HashSet<PeerId>>,
    validator_peers: RwLock<HashMap<Address, PeerId>>,
    message_tx: mpsc::Sender<NetworkMessage>,
    message_rx: RwLock<mpsc::Receiver<NetworkMessage>>,
    my_address: Address,
    shard_id: u16,
}

impl NetworkNode {
    pub async fn new(
        my_address: Address,
        shard_id: u16,
        listen_addr: &str,
    ) -> Result<Self, String> {
        // Create identity
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        info!("Local peer id: {}", local_peer_id);

        // Setup gossipsub
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validation_mode(ValidationMode::Strict)
            .message_id_fn(|msg| {
                let hash = Hash::new(&msg.data);
                gossipsub::MessageId::from(hash.to_hex())
            })
            .build()
            .map_err(|e| format!("Gossipsub config error: {:?}", e))?;

        let gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        ).map_err(|e| format!("Gossipsub error: {:?}", e))?;

        // Setup Kademlia
        let store = MemoryStore::new(local_peer_id);
        let kademlia = kad::Behaviour::new(local_peer_id, store);

        // Setup mDNS
        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
            .map_err(|e| format!("mDNS error: {:?}", e))?;

        let behaviour = ProtonNetworkBehaviour {
            gossipsub,
            kademlia,
            mdns,
        };

        // Setup QUIC transport
        let quic_config = quic::Config::default();
        let transport = quic::tokio::Transport::new(quic_config)
            .map(|(peer_id, conn), _| (peer_id, StreamMuxerBox::new(conn)))
            .boxed();

        let swarm = SwarmBuilder::with_tokio_executor(transport, behaviour, local_peer_id)
            .build();

        let (message_tx, message_rx) = mpsc::channel(1000);

        Ok(Self {
            local_peer_id,
            swarm: RwLock::new(swarm),
            connected_peers: RwLock::new(HashSet::new()),
            validator_peers: RwLock::new(HashMap::new()),
            message_tx,
            message_rx: RwLock::new(message_rx),
            my_address,
            shard_id,
        })
    }

    pub async fn start(&self) -> Result<(), String> {
        let mut swarm = self.swarm.write();

        // Listen on address
        let listen_addr: Multiaddr = "/ip4/0.0.0.0/udp/0/quic-v1".parse()
            .map_err(|e| format!("Invalid address: {:?}", e))?;
        swarm.listen_on(listen_addr)
            .map_err(|e| format!("Listen error: {:?}", e))?;

        // Subscribe to topics
        let block_topic = IdentTopic::new("proton-blocks");
        let tx_topic = IdentTopic::new("proton-transactions");
        let consensus_topic = IdentTopic::new("proton-consensus");
        let crosschain_topic = IdentTopic::new("proton-crosschain");

        swarm.behaviour_mut().gossipsub.subscribe(&block_topic)
            .map_err(|e| format!("Subscribe error: {:?}", e))?;
        swarm.behaviour_mut().gossipsub.subscribe(&tx_topic)
            .map_err(|e| format!("Subscribe error: {:?}", e))?;
        swarm.behaviour_mut().gossipsub.subscribe(&consensus_topic)
            .map_err(|e| format!("Subscribe error: {:?}", e))?;
        swarm.behaviour_mut().gossipsub.subscribe(&crosschain_topic)
            .map_err(|e| format!("Subscribe error: {:?}", e))?;

        info!("Network node started, listening on QUIC");
        Ok(())
    }

    /// Handle network events
    pub async fn run_event_loop(&self) {
        let mut swarm = self.swarm.write();

        loop {
            match swarm.select_next_some().await {
                SwarmEvent::Behaviour(ProtonNetworkBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { propagation_source, message, .. }
                )) => {
                    debug!("Received message from {}", propagation_source);

                    if let Ok(msg) = bincode::deserialize::<NetworkMessage>(&message.data) {
                        self.handle_message(msg).await;
                    }
                }
                SwarmEvent::Behaviour(ProtonNetworkBehaviourEvent::Mdns(
                    mdns::Event::Discovered(list)
                )) => {
                    for (peer_id, multiaddr) in list {
                        debug!("mDNS discovered peer: {} at {}", peer_id, multiaddr);
                        self.connected_peers.write().insert(peer_id);
                    }
                }
                SwarmEvent::Behaviour(ProtonNetworkBehaviourEvent::Kademlia(
                    kad::Event::OutboundQueryProgressed { result, .. }
                )) => {
                    match result {
                        kad::QueryResult::GetProviders(_) => {
                            debug!("Kademlia provider query completed");
                        }
                        _ => {}
                    }
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!("Listening on {}", address);
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Connected to {}", peer_id);
                    self.connected_peers.write().insert(peer_id);
                }
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    warn!("Disconnected from {}", peer_id);
                    self.connected_peers.write().remove(&peer_id);
                }
                _ => {}
            }
        }
    }

    async fn handle_message(&self, msg: NetworkMessage) {
        match msg {
            NetworkMessage::Transaction { tx } => {
                debug!("Received transaction: {:?}", tx.hash());
                // Add to mempool
            }
            NetworkMessage::BlockProposal { block, qc } => {
                debug!("Received block proposal at height {}", block.header.height);
                // Validate and process block
            }
            NetworkMessage::CrossChainTx { tx } => {
                debug!("Received cross-chain tx: {:?}", tx.tx_id);
                // Process cross-chain transaction
            }
            _ => {}
        }
    }

    /// Broadcast message to all peers
    pub fn broadcast(&self, msg: NetworkMessage) -> Result<(), String> {
        let topic = match &msg {
            NetworkMessage::BlockProposal { .. } => IdentTopic::new("proton-blocks"),
            NetworkMessage::Transaction { .. } | NetworkMessage::TransactionBatch { .. } => {
                IdentTopic::new("proton-transactions")
            }
            NetworkMessage::Prepare { .. } | NetworkMessage::PreCommit { .. } | 
            NetworkMessage::Commit { .. } | NetworkMessage::NewView { .. } => {
                IdentTopic::new("proton-consensus")
            }
            NetworkMessage::CrossChainTx { .. } | NetworkMessage::CrossChainProof { .. } => {
                IdentTopic::new("proton-crosschain")
            }
            _ => IdentTopic::new("proton-general"),
        };

        let data = bincode::serialize(&msg)
            .map_err(|e| format!("Serialization error: {:?}", e))?;

        let mut swarm = self.swarm.write();
        swarm.behaviour_mut().gossipsub.publish(topic, data)
            .map_err(|e| format!("Publish error: {:?}", e))?;

        Ok(())
    }

    /// Send message to specific peer
    pub fn send_to_peer(&self, peer_id: PeerId, msg: NetworkMessage) -> Result<(), String> {
        // In real implementation, use direct messaging
        self.broadcast(msg)
    }

    /// Get connected peer count
    pub fn peer_count(&self) -> usize {
        self.connected_peers.read().len()
    }

    /// Register validator peer
    pub fn register_validator(&self, address: Address, peer_id: PeerId) {
        self.validator_peers.write().insert(address, peer_id);
    }

    /// Get network stats
    pub fn get_stats(&self) -> NetworkStats {
        NetworkStats {
            peer_count: self.peer_count(),
            validator_count: self.validator_peers.read().len(),
            is_connected: self.peer_count() > 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NetworkStats {
    pub peer_count: usize,
    pub validator_count: usize,
    pub is_connected: bool,
}

/// High-performance transaction broadcaster
pub struct TxBroadcaster {
    network: Arc<NetworkNode>,
    batch_size: usize,
    batch_timeout_ms: u64,
    pending_txs: RwLock<Vec<Transaction>>,
}

impl TxBroadcaster {
    pub fn new(network: Arc<NetworkNode>, batch_size: usize, batch_timeout_ms: u64) -> Self {
        Self {
            network,
            batch_size,
            batch_timeout_ms,
            pending_txs: RwLock::new(Vec::new()),
        }
    }

    pub fn queue_transaction(&self, tx: Transaction) {
        let mut pending = self.pending_txs.write();
        pending.push(tx);

        if pending.len() >= self.batch_size {
            self.flush();
        }
    }

    pub fn flush(&self) {
        let mut pending = self.pending_txs.write();
        if pending.is_empty() {
            return;
        }

        let batch = std::mem::take(&mut *pending);
        let shard_id = batch.first().map(|tx| tx.shard_id).unwrap_or(0);

        let msg = NetworkMessage::TransactionBatch {
            txs: batch,
            shard_id,
        };

        let _ = self.network.broadcast(msg);
    }

    pub fn start_batch_timer(&self) -> tokio::task::JoinHandle<()> {
        let broadcaster = Arc::new(self.clone());
        let timeout_ms = self.batch_timeout_ms;

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(timeout_ms));

            loop {
                interval.tick().await;
                broadcaster.flush();
            }
        })
    }
}

impl Clone for TxBroadcaster {
    fn clone(&self) -> Self {
        Self {
            network: self.network.clone(),
            batch_size: self.batch_size,
            batch_timeout_ms: self.batch_timeout_ms,
            pending_txs: RwLock::new(Vec::new()),
        }
    }
}

/// Block synchronizer
pub struct BlockSync {
    network: Arc<NetworkNode>,
    known_blocks: RwLock<HashMap<u64, Hash>>,
    pending_requests: RwLock<HashSet<u64>>,
}

impl BlockSync {
    pub fn new(network: Arc<NetworkNode>) -> Self {
        Self {
            network,
            known_blocks: RwLock::new(HashMap::new()),
            pending_requests: RwLock::new(HashSet::new()),
        }
    }

    pub fn request_block(&self, height: u64, shard_id: u16) -> Result<(), String> {
        if self.known_blocks.read().contains_key(&height) {
            return Ok(()); // Already have this block
        }

        if !self.pending_requests.write().insert(height) {
            return Ok(()); // Already requested
        }

        let msg = NetworkMessage::BlockRequest { height, shard_id };
        self.network.broadcast(msg)
    }

    pub fn receive_block(&self, block: Block) {
        let height = block.header.height;
        self.known_blocks.write().insert(height, block.header.hash());
        self.pending_requests.write().remove(&height);

        info!("Received block {} with {} transactions", height, block.transactions.len());
    }

    pub fn get_sync_status(&self) -> SyncStatus {
        SyncStatus {
            known_blocks: self.known_blocks.read().len(),
            pending_requests: self.pending_requests.read().len(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SyncStatus {
    pub known_blocks: usize,
    pub pending_requests: usize,
}

use libp2p::core::muxing::StreamMuxerBox;
use futures::FutureExt;
          
