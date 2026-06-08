use crate::types::*;
use std::collections::{HashMap, VecDeque, BTreeMap};
use std::sync::Arc;
use parking_lot::{RwLock, Mutex};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, Instant};
use tracing::{info, warn, error, debug};
use secp256k1::{Secp256k1, Message, PublicKey, SecretKey, Signature};
use rand::seq::SliceRandom;

/// Consensus phases
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsensusPhase {
    NewView,
    Prepare,
    PreCommit,
    Commit,
    Decide,
}

/// Quorum certificate
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub block_hash: Hash,
    pub height: u64,
    pub round: u64,
    pub phase: ConsensusPhase,
    pub signatures: Vec<(Address, Vec<u8>)>,
    pub timestamp: u64,
}

impl QuorumCertificate {
    pub fn verify(&self, validator_set: &ValidatorSet) -> bool {
        let threshold = validator_set.total_stake() * 2 / 3;
        let mut stake_weight = 0u128;

        for (addr, sig) in &self.signatures {
            if let Some(validator) = validator_set.get_validator(addr) {
                if validator.verify_signature(&self.block_hash, sig) {
                    stake_weight += validator.stake;
                }
            }
        }

        stake_weight >= threshold
    }
}

/// Validator info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Validator {
    pub address: Address,
    pub public_key: Vec<u8>,
    pub stake: u128,
    pub commission: u8,
    pub uptime: f64,
    pub last_active: u64,
}

impl Validator {
    pub fn verify_signature(&self, hash: &Hash, signature: &[u8]) -> bool {
        let secp = Secp256k1::new();
        if let Ok(pubkey) = PublicKey::from_slice(&self.public_key) {
            if let Ok(msg) = Message::from_slice(hash.as_bytes()) {
                if let Ok(sig) = Signature::from_compact(signature) {
                    return secp.verify_ecdsa(&msg, &sig, &pubkey).is_ok();
                }
            }
        }
        false
    }
}

/// Validator set management
pub struct ValidatorSet {
    validators: RwLock<HashMap<Address, Validator>>,
    total_stake: RwLock<u128>,
}

impl ValidatorSet {
    pub fn new() -> Self {
        Self {
            validators: RwLock::new(HashMap::new()),
            total_stake: RwLock::new(0),
        }
    }

    pub fn add_validator(&self, validator: Validator) {
        let mut validators = self.validators.write();
        let mut total = self.total_stake.write();
        *total += validator.stake;
        validators.insert(validator.address, validator);
    }

    pub fn get_validator(&self, address: &Address) -> Option<Validator> {
        self.validators.read().get(address).cloned()
    }

    pub fn total_stake(&self) -> u128 {
        *self.total_stake.read()
    }

    pub fn get_leader(&self, round: u64) -> Option<Address> {
        let validators = self.validators.read();
        let mut sorted: Vec<_> = validators.values().collect();
        sorted.sort_by_key(|v| v.address);

        if sorted.is_empty() {
            return None;
        }

        let index = (round as usize) % sorted.len();
        Some(sorted[index].address)
    }

    pub fn get_random_committee(&self, round: u64, size: usize) -> Vec<Address> {
        let validators = self.validators.read();
        let mut addresses: Vec<_> = validators.keys().cloned().collect();

        // Deterministic shuffle based on round
        let mut seed = [0u8; 32];
        seed[..8].copy_from_slice(&round.to_le_bytes());
        let mut rng = rand::rngs::StdRng::from_seed(seed);
        addresses.shuffle(&mut rng);

        addresses.into_iter().take(size).collect()
    }

    pub fn validator_count(&self) -> usize {
        self.validators.read().len()
    }
}

/// HotStuff consensus engine
pub struct HotStuffConsensus {
    config: ChainConfig,
    validator_set: Arc<ValidatorSet>,
    current_height: RwLock<u64>,
    current_round: RwLock<u64>,
    phase: RwLock<ConsensusPhase>,

    // Block storage
    blocks: RwLock<BTreeMap<u64, Block>>,
    qcs: RwLock<BTreeMap<u64, QuorumCertificate>>,

    // Pending transactions
    mempool: Arc<Mempool>,

    // Channels
    block_tx: mpsc::Sender<Block>,
    block_rx: Mutex<mpsc::Receiver<Block>>,

    // Node identity
    my_address: Address,
    secret_key: SecretKey,

    // Metrics
    blocks_proposed: RwLock<u64>,
    blocks_finalized: RwLock<u64>,
    avg_latency_ms: RwLock<u64>,
}

/// Mempool for pending transactions
pub struct Mempool {
    transactions: RwLock<HashMap<Hash, Transaction>>,
    by_nonce: RwLock<BTreeMap<(Address, u64), Hash>>,
    by_gas_price: RwLock<BTreeMap<u128, Vec<Hash>>>,
    max_size: usize,
}

impl Mempool {
    pub fn new(max_size: usize) -> Self {
        Self {
            transactions: RwLock::new(HashMap::new()),
            by_nonce: RwLock::new(BTreeMap::new()),
            by_gas_price: RwLock::new(BTreeMap::new()),
            max_size,
        }
    }

    pub fn add_transaction(&self, tx: Transaction) -> Result<(), String> {
        let mut transactions = self.transactions.write();

        if transactions.len() >= self.max_size {
            return Err("Mempool full".to_string());
        }

        let hash = tx.hash();
        let key = (tx.from.clone(), tx.nonce);

        transactions.insert(hash.clone(), tx.clone());

        let mut by_nonce = self.by_nonce.write();
        by_nonce.insert(key, hash.clone());

        let mut by_gas = self.by_gas_price.write();
        by_gas.entry(tx.gas_price).or_default().push(hash);

        Ok(())
    }

    pub fn get_transactions(&self, limit: usize, min_gas_price: u128) -> Vec<Transaction> {
        let by_gas = self.by_gas_price.read();
        let transactions = self.transactions.read();

        let mut result = Vec::new();

        for (price, hashes) in by_gas.iter().rev() {
            if *price < min_gas_price {
                break;
            }
            for hash in hashes {
                if let Some(tx) = transactions.get(hash) {
                    result.push(tx.clone());
                    if result.len() >= limit {
                        return result;
                    }
                }
            }
        }

        result
    }

    pub fn remove_transactions(&self, hashes: &[Hash]) {
        let mut transactions = self.transactions.write();
        let mut by_nonce = self.by_nonce.write();
        let mut by_gas = self.by_gas_price.write();

        for hash in hashes {
            if let Some(tx) = transactions.remove(hash) {
                by_nonce.remove(&(tx.from, tx.nonce));
                if let Some(entry) = by_gas.get_mut(&tx.gas_price) {
                    entry.retain(|h| h != hash);
                    if entry.is_empty() {
                        by_gas.remove(&tx.gas_price);
                    }
                }
            }
        }
    }

    pub fn size(&self) -> usize {
        self.transactions.read().len()
    }
}

impl HotStuffConsensus {
    pub fn new(
        config: ChainConfig,
        validator_set: Arc<ValidatorSet>,
        my_address: Address,
        secret_key: SecretKey,
    ) -> Self {
        let (block_tx, block_rx) = mpsc::channel(100);

        Self {
            config,
            validator_set,
            current_height: RwLock::new(0),
            current_round: RwLock::new(0),
            phase: RwLock::new(ConsensusPhase::NewView),
            blocks: RwLock::new(BTreeMap::new()),
            qcs: RwLock::new(BTreeMap::new()),
            mempool: Arc::new(Mempool::new(100_000)),
            block_tx,
            block_rx: Mutex::new(block_rx),
            my_address,
            secret_key,
            blocks_proposed: RwLock::new(0),
            blocks_finalized: RwLock::new(0),
            avg_latency_ms: RwLock::new(0),
        }
    }

    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        let config = self.config.clone();
        let mempool = self.mempool.clone();
        let validator_set = self.validator_set.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(config.block_time_ms));

            loop {
                interval.tick().await;

                // Propose block if leader
                let round = 0u64; // Get from consensus state
                if let Some(leader) = validator_set.get_leader(round) {
                    // Propose logic here
                }
            }
        })
    }

    pub fn propose_block(&self, prev_hash: Hash) -> Option<Block> {
        let height = *self.current_height.read() + 1;
        let timestamp = current_timestamp_ms();

        // Get transactions from mempool
        let txs = self.mempool.get_transactions(1000, self.config.min_gas_price);

        if txs.is_empty() {
            return None;
        }

        // Calculate merkle root
        let tx_hashes: Vec<_> = txs.iter().map(|tx| tx.hash()).collect();
        let merkle_root = MerkleTree::root(&tx_hashes);

        // Create block header
        let header = BlockHeader {
            version: 1,
            prev_hash,
            merkle_root,
            state_root: Hash::zero(), // Updated after execution
            shard_id: 0, // Primary shard
            height,
            timestamp,
            validator: self.my_address,
            signature: vec![], // Will be signed
            tx_count: txs.len() as u32,
            gas_used: 0, // Updated after execution
            extra_data: vec![],
        };

        let block = Block {
            header,
            transactions: txs,
            shard_proofs: vec![],
        };

        Some(block)
    }

    pub fn validate_block(&self, block: &Block) -> bool {
        // Verify block hash
        let expected_hash = block.header.hash();

        // Verify previous block exists
        let blocks = self.blocks.read();
        if block.header.height > 0 {
            if !blocks.contains_key(&(block.header.height - 1)) {
                return false;
            }
        }

        // Verify merkle root
        let tx_hashes: Vec<_> = block.transactions.iter().map(|tx| tx.hash()).collect();
        let expected_merkle = MerkleTree::root(&tx_hashes);
        if expected_merkle != block.header.merkle_root {
            return false;
        }

        // Verify validator is leader
        let round = block.header.height; // Simplified
        if let Some(leader) = self.validator_set.get_leader(round) {
            if leader != block.header.validator {
                return false;
            }
        }

        // Verify signature
        let validator = self.validator_set.get_validator(&block.header.validator);
        if let Some(v) = validator {
            let hash = block.header.hash();
            if !v.verify_signature(&hash, &block.header.signature) {
                return false;
            }
        }

        true
    }

    pub fn finalize_block(&self, block: Block) {
        let height = block.header.height;

        // Store block
        self.blocks.write().insert(height, block.clone());

        // Remove transactions from mempool
        let tx_hashes: Vec<_> = block.transactions.iter().map(|tx| tx.hash()).collect();
        self.mempool.remove_transactions(&tx_hashes);

        // Update metrics
        *self.blocks_finalized.write() += 1;

        info!("Block finalized: height={}, txs={}", height, block.header.tx_count);
    }

    pub fn get_block(&self, height: u64) -> Option<Block> {
        self.blocks.read().get(&height).cloned()
    }

    pub fn get_latest_height(&self) -> u64 {
        *self.current_height.read()
    }

    pub fn get_mempool(&self) -> Arc<Mempool> {
        self.mempool.clone()
    }

    pub fn get_stats(&self) -> ConsensusStats {
        ConsensusStats {
            height: *self.current_height.read(),
            round: *self.current_round.read(),
            blocks_proposed: *self.blocks_proposed.read(),
            blocks_finalized: *self.blocks_finalized.read(),
            mempool_size: self.mempool.size(),
            avg_latency_ms: *self.avg_latency_ms.read(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConsensusStats {
    pub height: u64,
    pub round: u64,
    pub blocks_proposed: u64,
    pub blocks_finalized: u64,
    pub mempool_size: usize,
    pub avg_latency_ms: u64,
}

/// DAG-based ordering for parallel processing
pub struct DagOrdering {
    vertices: RwLock<HashMap<Hash, DagVertex>>,
    edges: RwLock<HashMap<Hash, Vec<Hash>>>,
}

#[derive(Clone, Debug)]
pub struct DagVertex {
    pub hash: Hash,
    pub parents: Vec<Hash>,
    pub timestamp: u64,
    pub round: u64,
    pub is_block: bool,
}

impl DagOrdering {
    pub fn new() -> Self {
        Self {
            vertices: RwLock::new(HashMap::new()),
            edges: RwLock::new(HashMap::new()),
        }
    }

    pub fn add_vertex(&self, vertex: DagVertex) {
        let hash = vertex.hash.clone();
        let parents = vertex.parents.clone();

        self.vertices.write().insert(hash.clone(), vertex);
        self.edges.write().insert(hash, parents);
    }

    pub fn get_ancestors(&self, hash: &Hash) -> Vec<Hash> {
        let edges = self.edges.read();
        let mut visited = HashMap::new();
        let mut result = Vec::new();
        let mut stack = vec![hash.clone()];

        while let Some(current) = stack.pop() {
            if visited.contains_key(&current) {
                continue;
            }
            visited.insert(current.clone(), true);
            result.push(current.clone());

            if let Some(parents) = edges.get(&current) {
                for parent in parents {
                    stack.push(parent.clone());
                }
            }
        }

        result
    }

    pub fn topological_sort(&self) -> Vec<Hash> {
        let vertices = self.vertices.read();
        let edges = self.edges.read();

        let mut in_degree: HashMap<Hash, usize> = HashMap::new();
        let mut adj: HashMap<Hash, Vec<Hash>> = HashMap::new();

        for (hash, vertex) in vertices.iter() {
            in_degree.entry(hash.clone()).or_insert(0);
            for parent in &vertex.parents {
                adj.entry(parent.clone()).or_default().push(hash.clone());
                *in_degree.entry(hash.clone()).or_insert(0) += 1;
            }
        }

        let mut queue: VecDeque<_> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(h, _)| h.clone())
            .collect();

        let mut result = Vec::new();

        while let Some(current) = queue.pop_front() {
            result.push(current.clone());

            if let Some(neighbors) = adj.get(&current) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }
        }

        result
    }
          }
                           
