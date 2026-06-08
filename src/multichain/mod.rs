use crate::types::*;
use crate::consensus::*;
use crate::vm::*;
use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque, BTreeMap};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{info, warn, error, debug};
use crossbeam::channel;

/// Shard configuration
#[derive(Clone, Debug)]
pub struct ShardConfig {
    pub shard_id: u16,
    pub validators: Vec<Address>,
    pub consensus: Arc<HotStuffConsensus>,
    pub vm: Arc<ProtonVM>,
    pub cross_chain_enabled: bool,
}

/// Shard manager - manages multiple shards
pub struct ShardManager {
    shards: RwLock<HashMap<u16, Shard>>,
    cross_chain_router: Arc<CrossChainRouter>,
    config: ChainConfig,
    total_tx_processed: RwLock<u64>,
}

/// Individual shard
pub struct Shard {
    pub config: ShardConfig,
    pub state: Arc<RwLock<ShardState>>,
    pub block_height: RwLock<u64>,
    pub pending_cross_txs: RwLock<VecDeque<CrossChainTransaction>>,
}

#[derive(Clone, Debug)]
pub struct ShardState {
    pub accounts: HashMap<Address, Account>,
    pub nonce_tracker: HashMap<Address, u64>,
    pub last_block_hash: Hash,
    pub cross_chain_nonce: u64,
}

/// Cross-chain transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrossChainTransaction {
    pub tx_id: Hash,
    pub source_shard: u16,
    pub target_shard: u16,
    pub sender: Address,
    pub receiver: Address,
    pub amount: u128,
    pub nonce: u64,
    pub status: CrossChainStatus,
    pub proof: Option<Vec<u8>>,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossChainStatus {
    Pending,
    SourceConfirmed,
    InTransit,
    TargetConfirmed,
    Failed,
}

/// Cross-chain router
pub struct CrossChainRouter {
    pending_txs: RwLock<HashMap<Hash, CrossChainTransaction>>,
    completed_txs: RwLock<BTreeMap<u64, Hash>>, // by timestamp
    shard_channels: RwLock<HashMap<u16, mpsc::Sender<CrossChainTransaction>>>,
    relay_validators: RwLock<Vec<Address>>,
}

impl CrossChainRouter {
    pub fn new() -> Self {
        Self {
            pending_txs: RwLock::new(HashMap::new()),
            completed_txs: RwLock::new(BTreeMap::new()),
            shard_channels: RwLock::new(HashMap::new()),
            relay_validators: RwLock::new(Vec::new()),
        }
    }

    pub fn register_shard(&self, shard_id: u16, sender: mpsc::Sender<CrossChainTransaction>) {
        self.shard_channels.write().insert(shard_id, sender);
    }

    pub fn submit_cross_tx(&self, tx: CrossChainTransaction) -> Result<(), String> {
        let mut pending = self.pending_txs.write();

        if pending.contains_key(&tx.tx_id) {
            return Err("Transaction already pending".to_string());
        }

        pending.insert(tx.tx_id.clone(), tx.clone());

        // Route to target shard
        let channels = self.shard_channels.read();
        if let Some(sender) = channels.get(&tx.target_shard) {
            let _ = sender.try_send(tx);
        }

        Ok(())
    }

    pub fn confirm_tx(&self, tx_id: &Hash, proof: Vec<u8>) -> Result<(), String> {
        let mut pending = self.pending_txs.write();

        if let Some(tx) = pending.get_mut(tx_id) {
            tx.status = CrossChainStatus::TargetConfirmed;
            tx.proof = Some(proof);

            let mut completed = self.completed_txs.write();
            completed.insert(tx.timestamp, tx_id.clone());

            pending.remove(tx_id);

            info!("Cross-chain transaction confirmed: {}", tx_id);
            Ok(())
        } else {
            Err("Transaction not found".to_string())
        }
    }

    pub fn get_pending(&self) -> Vec<CrossChainTransaction> {
        self.pending_txs.read().values().cloned().collect()
    }

    pub fn get_stats(&self) -> CrossChainStats {
        CrossChainStats {
            pending_count: self.pending_txs.read().len(),
            completed_count: self.completed_txs.read().len(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CrossChainStats {
    pub pending_count: usize,
    pub completed_count: usize,
}

impl ShardManager {
    pub fn new(config: ChainConfig, router: Arc<CrossChainRouter>) -> Self {
        Self {
            shards: RwLock::new(HashMap::new()),
            cross_chain_router: router,
            config,
            total_tx_processed: RwLock::new(0),
        }
    }

    /// Create and initialize shards
    pub fn initialize_shards(&self, validator_set: Arc<ValidatorSet>) {
        let mut shards = self.shards.write();

        for shard_id in 0..self.config.shard_count {
            // Select validators for this shard
            let committee = validator_set.get_random_committee(shard_id as u64, 16);

            let shard_state = Arc::new(RwLock::new(ShardState {
                accounts: HashMap::new(),
                nonce_tracker: HashMap::new(),
                last_block_hash: Hash::zero(),
                cross_chain_nonce: 0,
            }));

            // Create VM state for this shard
            let vm_state = Arc::new(RwLock::new(VMState {
                accounts: HashMap::new(),
                contracts: HashMap::new(),
                storage: HashMap::new(),
                block_context: BlockContext {
                    height: 0,
                    timestamp: 0,
                    coinbase: Address::new(&[0u8; 32]),
                    difficulty: 1,
                    gas_limit: self.config.max_block_gas,
                },
            }));

            let vm = Arc::new(ProtonVM::new(vm_state, GasSchedule::default()));

            let shard_config = ShardConfig {
                shard_id,
                validators: committee,
                consensus: Arc::new(HotStuffConsensus::new(
                    self.config.clone(),
                    validator_set.clone(),
                    Address::new(&[shard_id as u8; 32]),
                    // Secret key placeholder
                    secp256k1::SecretKey::from_slice(&[1u8; 32]).unwrap(),
                )),
                vm,
                cross_chain_enabled: self.config.cross_chain_enabled,
            };

            let shard = Shard {
                config: shard_config,
                state: shard_state,
                block_height: RwLock::new(0),
                pending_cross_txs: RwLock::new(VecDeque::new()),
            };

            shards.insert(shard_id, shard);
        }

        info!("Initialized {} shards", self.config.shard_count);
    }

    /// Route transaction to appropriate shard
    pub fn route_transaction(&self, tx: Transaction) -> Result<u16, String> {
        let shard_id = if tx.shard_id > 0 && tx.shard_id < self.config.shard_count {
            tx.shard_id
        } else {
            // Determine shard by address hash
            let addr_hash = tx.from.hash();
            let shard_id = (u16::from_le_bytes([addr_hash.as_bytes()[0], addr_hash.as_bytes()[1]]) 
                % self.config.shard_count) as u16;
            shard_id
        };

        let shards = self.shards.read();
        if let Some(shard) = shards.get(&shard_id) {
            // Add to shard mempool
            shard.config.consensus.get_mempool().add_transaction(tx)?;
            Ok(shard_id)
        } else {
            Err("Shard not found".to_string())
        }
    }

    /// Process cross-chain transaction
    pub fn process_cross_chain_tx(&self, tx: CrossChainTransaction) -> Result<(), String> {
        // Lock on source shard
        let shards = self.shards.read();

        if let Some(source_shard) = shards.get(&tx.source_shard) {
            let mut state = source_shard.state.write();

            // Verify sender has sufficient balance
            let sender = state.accounts.get(&tx.sender).cloned().unwrap_or_default();
            if sender.balance < tx.amount {
                return Err("Insufficient balance for cross-chain transfer".to_string());
            }

            // Deduct from source
            let mut sender_account = sender;
            sender_account.balance -= tx.amount;
            state.accounts.insert(tx.sender, sender_account);

            // Submit to cross-chain router
            let mut tx_with_status = tx.clone();
            tx_with_status.status = CrossChainStatus::SourceConfirmed;
            self.cross_chain_router.submit_cross_tx(tx_with_status)?;

            info!("Cross-chain tx submitted: {} -> shard {}", tx.tx_id, tx.target_shard);
        }

        Ok(())
    }

    /// Receive cross-chain transaction on target shard
    pub fn receive_cross_chain_tx(&self, tx: CrossChainTransaction) -> Result<(), String> {
        let shards = self.shards.read();

        if let Some(target_shard) = shards.get(&tx.target_shard) {
            let mut state = target_shard.state.write();

            // Credit receiver on target shard
            let receiver = state.accounts.get(&tx.receiver).cloned().unwrap_or_default();
            let mut receiver_account = receiver;
            receiver_account.balance += tx.amount;
            state.accounts.insert(tx.receiver, receiver_account);

            // Confirm completion
            self.cross_chain_router.confirm_tx(&tx.tx_id, vec![1])?;

            info!("Cross-chain tx completed: {} on shard {}", tx.tx_id, tx.target_shard);
        }

        Ok(())
    }

    /// Get shard info
    pub fn get_shard_info(&self, shard_id: u16) -> Option<ShardInfo> {
        let shards = self.shards.read();
        shards.get(&shard_id).map(|shard| ShardInfo {
            shard_id,
            block_height: *shard.block_height.read(),
            validator_count: shard.config.validators.len(),
            mempool_size: shard.config.consensus.get_mempool().size(),
            pending_cross_txs: shard.pending_cross_txs.read().len(),
        })
    }

    /// Get all shard stats
    pub fn get_all_shard_stats(&self) -> Vec<ShardInfo> {
        let shards = self.shards.read();
        shards.iter().map(|(id, shard)| ShardInfo {
            shard_id: *id,
            block_height: *shard.block_height.read(),
            validator_count: shard.config.validators.len(),
            mempool_size: shard.config.consensus.get_mempool().size(),
            pending_cross_txs: shard.pending_cross_txs.read().len(),
        }).collect()
    }

    /// Get total TPS across all shards
    pub fn get_total_tps(&self) -> f64 {
        let total_tx = *self.total_tx_processed.read();
        // Simplified: assume 1 second measurement window
        total_tx as f64
    }

    pub fn get_cross_chain_stats(&self) -> CrossChainStats {
        self.cross_chain_router.get_stats()
    }
}

#[derive(Clone, Debug)]
pub struct ShardInfo {
    pub shard_id: u16,
    pub block_height: u64,
    pub validator_count: usize,
    pub mempool_size: usize,
    pub pending_cross_txs: usize,
}

/// Shard synchronization
pub struct ShardSync {
    shard_manager: Arc<ShardManager>,
    sync_interval_ms: u64,
}

impl ShardSync {
    pub fn new(shard_manager: Arc<ShardManager>, sync_interval_ms: u64) -> Self {
        Self {
            shard_manager,
            sync_interval_ms,
        }
    }

    pub fn start_sync(&self) -> tokio::task::JoinHandle<()> {
        let shard_manager = self.shard_manager.clone();
        let interval_ms = self.sync_interval_ms;

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(interval_ms));

            loop {
                interval.tick().await;

                // Sync cross-chain transactions
                let pending = shard_manager.get_cross_chain_stats().pending_count;
                if pending > 0 {
                    debug!("Syncing {} pending cross-chain transactions", pending);
                }

                // Sync shard states
                let stats = shard_manager.get_all_shard_stats();
                for stat in stats {
                    if stat.pending_cross_txs > 0 {
                        debug!("Shard {} has {} pending cross txs", stat.shard_id, stat.pending_cross_txs);
                    }
                }
            }
        })
    }
}

/// Adaptive sharding - dynamically adjust shard count based on load
pub struct AdaptiveSharding {
    shard_manager: Arc<ShardManager>,
    load_threshold: f64,
    target_tps_per_shard: u64,
}

impl AdaptiveSharding {
    pub fn new(shard_manager: Arc<ShardManager>, target_tps: u64) -> Self {
        Self {
            shard_manager,
            load_threshold: 0.8,
            target_tps_per_shard: target_tps,
        }
    }

    pub fn should_split_shard(&self, shard_id: u16) -> bool {
        if let Some(info) = self.shard_manager.get_shard_info(shard_id) {
            let current_tps = info.mempool_size as f64 / 10.0; // Simplified estimation
            let load = current_tps / self.target_tps_per_shard as f64;
            load > self.load_threshold
        } else {
            false
        }
    }

    pub fn should_merge_shards(&self, shard_ids: &[u16]) -> bool {
        let total_load: f64 = shard_ids.iter()
            .filter_map(|id| self.shard_manager.get_shard_info(*id))
            .map(|info| info.mempool_size as f64 / 10.0)
            .sum();

        total_load / shard_ids.len() as f64 < self.target_tps_per_shard as f64 * 0.3
    }
  }
  
