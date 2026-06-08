pub mod types;
pub mod consensus;
pub mod privacy;
pub mod vm;
pub mod multichain;
pub mod network;
pub mod storage;

use types::*;
use consensus::*;
use privacy::*;
use vm::*;
use multichain::*;
use network::*;
use storage::*;

use std::sync::Arc;
use parking_lot::RwLock;
use tokio::runtime::Runtime;
use tracing::{info, warn, error};

/// Proton Chain node
pub struct ProtonNode {
    pub config: ChainConfig,
    pub consensus: Arc<HotStuffConsensus>,
    pub vm: Arc<ProtonVM>,
    pub shard_manager: Arc<ShardManager>,
    pub network: Arc<NetworkNode>,
    pub storage: Arc<StateStorage>,
    pub zk_system: Arc<ZkProtonSystem>,
    pub runtime: Runtime,
}

impl ProtonNode {
    pub fn new(config: ChainConfig, data_dir: &str) -> Result<Self, String> {
        let runtime = Runtime::new()
            .map_err(|e| format!("Failed to create runtime: {:?}", e))?;

        // Initialize storage
        let db_path = std::path::Path::new(data_dir).join("db");
        let db = Arc::new(RocksDB::new(&db_path)?);
        let storage = Arc::new(StateStorage::new(db, 100_000));

        // Initialize validator set
        let validator_set = Arc::new(ValidatorSet::new());

        // Initialize VM state
        let vm_state = Arc::new(RwLock::new(VMState {
            accounts: HashMap::new(),
            contracts: HashMap::new(),
            storage: HashMap::new(),
            block_context: BlockContext {
                height: 0,
                timestamp: 0,
                coinbase: Address::new(&[0u8; 32]),
                difficulty: 1,
                gas_limit: config.max_block_gas,
            },
        }));

        let vm = Arc::new(ProtonVM::new(vm_state, GasSchedule::default()));

        // Initialize ZK system
        let zk_system = Arc::new(ZkProtonSystem::new());
        zk_system.setup()?;

        // Initialize consensus
        let my_address = Address::new(&[1u8; 32]);
        let secret_key = secp256k1::SecretKey::from_slice(&[1u8; 32])
            .map_err(|e| format!("Invalid key: {:?}", e))?;

        let consensus = Arc::new(HotStuffConsensus::new(
            config.clone(),
            validator_set.clone(),
            my_address,
            secret_key,
        ));

        // Initialize cross-chain router
        let cross_chain_router = Arc::new(CrossChainRouter::new());

        // Initialize shard manager
        let shard_manager = Arc::new(ShardManager::new(config.clone(), cross_chain_router));
        shard_manager.initialize_shards(validator_set);

        // Initialize network (placeholder - would need async)
        let network = Arc::new(NetworkNode::new(my_address, 0, "0.0.0.0:0")
            .await
            .map_err(|e| format!("Network init failed: {:?}", e))?);

        info!("Proton Node initialized with {} shards", config.shard_count);

        Ok(Self {
            config,
            consensus,
            vm,
            shard_manager,
            network,
            storage,
            zk_system,
            runtime,
        })
    }

    pub fn start(&self) -> Result<(), String> {
        info!("Starting Proton Chain node...");

        // Start consensus
        let _consensus_handle = self.consensus.start();

        // Start network
        // self.network.start().await?;

        // Start shard sync
        let shard_sync = ShardSync::new(self.shard_manager.clone(), 1000);
        let _sync_handle = shard_sync.start_sync();

        info!("Proton Chain node started successfully");
        Ok(())
    }

    pub fn submit_transaction(&self, tx: Transaction) -> Result<Hash, String> {
        let hash = tx.hash();

        // Route to appropriate shard
        let shard_id = self.shard_manager.route_transaction(tx.clone())?;

        info!("Transaction {} routed to shard {}", hash, shard_id);

        Ok(hash)
    }

    pub fn get_balance(&self, address: &Address) -> u128 {
        self.vm.get_account(address)
            .map(|acc| acc.balance)
            .unwrap_or(0)
    }

    pub fn get_block(&self, height: u64) -> Option<Block> {
        self.consensus.get_block(height)
    }

    pub fn get_stats(&self) -> NodeStats {
        let consensus_stats = self.consensus.get_stats();
        let shard_stats = self.shard_manager.get_all_shard_stats();
        let total_tps = self.shard_manager.get_total_tps();

        NodeStats {
            height: consensus_stats.height,
            mempool_size: consensus_stats.mempool_size,
            shard_count: shard_stats.len(),
            total_tps,
            peer_count: self.network.peer_count(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NodeStats {
    pub height: u64,
    pub mempool_size: usize,
    pub shard_count: usize,
    pub total_tps: f64,
    pub peer_count: usize,
}

use std::collections::HashMap;
use std::path::Path;
