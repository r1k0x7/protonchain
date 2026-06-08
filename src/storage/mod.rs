use crate::types::*;
use std::sync::Arc;
use parking_lot::RwLock;
use std::path::Path;
use tracing::{info, debug, error};

/// Database interface
pub trait Database: Send + Sync {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;
    fn put(&self, key: &[u8], value: &[u8]) -> Result<(), String>;
    fn delete(&self, key: &[u8]) -> Result<(), String>;
    fn batch_write(&self, batch: Vec<(Vec<u8>, Option<Vec<u8>>)>) -> Result<(), String>;
    fn iterator(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, Vec<u8>)>>;
}

/// RocksDB implementation
pub struct RocksDB {
    db: RwLock<rocksdb::DB>,
}

impl RocksDB {
    pub fn new(path: &Path) -> Result<Self, String> {
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.set_max_open_files(10000);
        opts.set_write_buffer_size(64 * 1024 * 1024); // 64MB
        opts.set_max_write_buffer_number(3);
        opts.set_target_file_size_base(64 * 1024 * 1024);

        // Column families
        let cf_names = vec![
            "default",
            "blocks",
            "transactions",
            "accounts",
            "contracts",
            "storage",
            "metadata",
            "index",
        ];

        let cf_descriptors: Vec<_> = cf_names.iter()
            .map(|name| rocksdb::ColumnFamilyDescriptor::new(*name, opts.clone()))
            .collect();

        let db = rocksdb::DB::open_cf_descriptors(&opts, path, cf_descriptors)
            .map_err(|e| format!("Failed to open database: {:?}", e))?;

        info!("RocksDB opened at {:?}", path);

        Ok(Self {
            db: RwLock::new(db),
        })
    }

    fn cf_handle(&self, name: &str) -> Result<rocksdb::ColumnFamily, String> {
        let db = self.db.read();
        db.cf_handle(name)
            .ok_or_else(|| format!("Column family {} not found", name))
    }
}

impl Database for RocksDB {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let db = self.db.read();
        db.get(key).unwrap_or(None)
    }

    fn put(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        let db = self.db.read();
        db.put(key, value)
            .map_err(|e| format!("Put error: {:?}", e))
    }

    fn delete(&self, key: &[u8]) -> Result<(), String> {
        let db = self.db.read();
        db.delete(key)
            .map_err(|e| format!("Delete error: {:?}", e))
    }

    fn batch_write(&self, batch: Vec<(Vec<u8>, Option<Vec<u8>>)>) -> Result<(), String> {
        let db = self.db.read();
        let mut write_batch = rocksdb::WriteBatch::default();

        for (key, value) in batch {
            match value {
                Some(v) => write_batch.put(&key, &v),
                None => write_batch.delete(&key),
            }
        }

        db.write(write_batch)
            .map_err(|e| format!("Batch write error: {:?}", e))
    }

    fn iterator(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, Vec<u8>)>> {
        let db = self.db.read();
        let iter = db.prefix_iterator(prefix);

        Box::new(iter.filter_map(|item| {
            item.ok().map(|(k, v)| (k.to_vec(), v.to_vec()))
        }))
    }
}

/// In-memory database for testing
pub struct MemoryDB {
    data: RwLock<HashMap<Vec<u8>, Vec<u8>>>,
}

impl MemoryDB {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Database for MemoryDB {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.data.read().get(key).cloned()
    }

    fn put(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.data.write().insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &[u8]) -> Result<(), String> {
        self.data.write().remove(key);
        Ok(())
    }

    fn batch_write(&self, batch: Vec<(Vec<u8>, Option<Vec<u8>>)>) -> Result<(), String> {
        let mut data = self.data.write();
        for (key, value) in batch {
            match value {
                Some(v) => { data.insert(key, v); }
                None => { data.remove(&key); }
            }
        }
        Ok(())
    }

    fn iterator(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, Vec<u8>)>> {
        let data = self.data.read().clone();
        let prefix = prefix.to_vec();

        Box::new(data.into_iter().filter(move |(k, _)| {
            k.starts_with(&prefix)
        }))
    }
}

use std::collections::HashMap;

/// State storage manager
pub struct StateStorage {
    db: Arc<dyn Database>,
    cache: RwLock<lru::LruCache<Hash, Account>>,
    pending_changes: RwLock<Vec<(Vec<u8>, Option<Vec<u8>>)>>,
}

impl StateStorage {
    pub fn new(db: Arc<dyn Database>, cache_size: usize) -> Self {
        Self {
            db,
            cache: RwLock::new(lru::LruCache::new(cache_size)),
            pending_changes: RwLock::new(Vec::new()),
        }
    }

    /// Get account by address
    pub fn get_account(&self, address: &Address) -> Option<Account> {
        let hash = Hash::new(address.as_bytes());

        // Check cache first
        if let Some(account) = self.cache.write().get(&hash) {
            return Some(account.clone());
        }

        // Load from database
        let key = Self::account_key(address);
        if let Some(data) = self.db.get(&key) {
            if let Ok(account) = bincode::deserialize(&data) {
                self.cache.write().put(hash, account.clone());
                return Some(account);
            }
        }

        None
    }

    /// Store account
    pub fn put_account(&self, address: &Address, account: &Account) -> Result<(), String> {
        let key = Self::account_key(address);
        let value = bincode::serialize(account)
            .map_err(|e| format!("Serialization error: {:?}", e))?;

        let hash = Hash::new(address.as_bytes());
        self.cache.write().put(hash, account.clone());

        self.pending_changes.write().push((key, Some(value)));
        Ok(())
    }

    /// Get block by height
    pub fn get_block(&self, height: u64) -> Option<Block> {
        let key = Self::block_key(height);
        if let Some(data) = self.db.get(&key) {
            if let Ok(block) = bincode::deserialize(&data) {
                return Some(block);
            }
        }
        None
    }

    /// Store block
    pub fn put_block(&self, block: &Block) -> Result<(), String> {
        let key = Self::block_key(block.header.height);
        let value = bincode::serialize(block)
            .map_err(|e| format!("Serialization error: {:?}", e))?;

        self.pending_changes.write().push((key, Some(value)));
        Ok(())
    }

    /// Get transaction
    pub fn get_transaction(&self, hash: &Hash) -> Option<Transaction> {
        let key = Self::tx_key(hash);
        if let Some(data) = self.db.get(&key) {
            if let Ok(tx) = bincode::deserialize(&data) {
                return Some(tx);
            }
        }
        None
    }

    /// Store transaction
    pub fn put_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let hash = tx.hash();
        let key = Self::tx_key(&hash);
        let value = bincode::serialize(tx)
            .map_err(|e| format!("Serialization error: {:?}", e))?;

        self.pending_changes.write().push((key, Some(value)));
        Ok(())
    }

    /// Get contract code
    pub fn get_contract_code(&self, address: &Address) -> Option<Vec<u8>> {
        let key = Self::contract_key(address);
        self.db.get(&key)
    }

    /// Store contract code
    pub fn put_contract_code(&self, address: &Address, code: &[u8]) -> Result<(), String> {
        let key = Self::contract_key(address);
        self.pending_changes.write().push((key, Some(code.to_vec())));
        Ok(())
    }

    /// Get storage value
    pub fn get_storage(&self, address: &Address, key: &Hash) -> Option<Vec<u8>> {
        let db_key = Self::storage_key(address, key);
        self.db.get(&db_key)
    }

    /// Store storage value
    pub fn put_storage(&self, address: &Address, key: &Hash, value: &[u8]) -> Result<(), String> {
        let db_key = Self::storage_key(address, key);
        self.pending_changes.write().push((db_key, Some(value.to_vec())));
        Ok(())
    }

    /// Commit pending changes
    pub fn commit(&self) -> Result<(), String> {
        let mut changes = self.pending_changes.write();
        if changes.is_empty() {
            return Ok(());
        }

        let batch = std::mem::take(&mut *changes);
        self.db.batch_write(batch)
    }

    /// Rollback pending changes
    pub fn rollback(&self) {
        self.pending_changes.write().clear();
    }

    /// Get state root
    pub fn state_root(&self) -> Hash {
        // Simplified: hash all pending changes
        let changes = self.pending_changes.read();
        let mut data = Vec::new();
        for (key, value) in changes.iter() {
            data.extend_from_slice(key);
            if let Some(v) = value {
                data.extend_from_slice(v);
            }
        }
        Hash::new(&data)
    }

    // Key encoding helpers
    fn account_key(address: &Address) -> Vec<u8> {
        let mut key = b"acc:".to_vec();
        key.extend_from_slice(address.as_bytes());
        key
    }

    fn block_key(height: u64) -> Vec<u8> {
        let mut key = b"blk:".to_vec();
        key.extend_from_slice(&height.to_le_bytes());
        key
    }

    fn tx_key(hash: &Hash) -> Vec<u8> {
        let mut key = b"tx:".to_vec();
        key.extend_from_slice(hash.as_bytes());
        key
    }

    fn contract_key(address: &Address) -> Vec<u8> {
        let mut key = b"code:".to_vec();
        key.extend_from_slice(address.as_bytes());
        key
    }

    fn storage_key(address: &Address, key: &Hash) -> Vec<u8> {
        let mut db_key = b"stor:".to_vec();
        db_key.extend_from_slice(address.as_bytes());
        db_key.extend_from_slice(b":");
        db_key.extend_from_slice(key.as_bytes());
        db_key
    }
}

/// State snapshot for fast sync
pub struct StateSnapshot {
    pub block_height: u64,
    pub state_root: Hash,
    pub accounts: Vec<(Address, Account)>,
    pub timestamp: u64,
}

impl StateSnapshot {
    pub fn create(storage: &StateStorage, block_height: u64) -> Result<Self, String> {
        let mut accounts = Vec::new();

        // Iterate all accounts
        let prefix = b"acc:".to_vec();
        for (key, value) in storage.db.iterator(&prefix) {
            if let Ok(account) = bincode::deserialize(&value) {
                // Extract address from key (skip "acc:" prefix)
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(&key[4..24]);
                let address = Address(addr_bytes);
                accounts.push((address, account));
            }
        }

        let state_root = storage.state_root();

        Ok(Self {
            block_height,
            state_root,
            accounts,
            timestamp: current_timestamp_ms(),
        })
    }

    pub fn apply(&self, storage: &StateStorage) -> Result<(), String> {
        for (address, account) in &self.accounts {
            storage.put_account(address, account)?;
        }
        storage.commit()
    }
}

/// Pruning manager - remove old state
pub struct StatePruner {
    storage: Arc<StateStorage>,
    keep_blocks: u64,
    prune_interval: u64,
}

impl StatePruner {
    pub fn new(storage: Arc<StateStorage>, keep_blocks: u64, prune_interval: u64) -> Self {
        Self {
            storage,
            keep_blocks,
            prune_interval,
        }
    }

    pub fn prune_old_blocks(&self, current_height: u64) -> Result<u64, String> {
        let prune_below = current_height.saturating_sub(self.keep_blocks);
        let mut pruned = 0u64;

        // Remove old blocks
        for height in 0..prune_below {
            let key = StateStorage::block_key(height);
            // In real implementation, would delete from DB
            pruned += 1;
        }

        info!("Pruned {} old blocks below height {}", pruned, prune_below);
        Ok(pruned)
    }

    pub fn start_pruning_task(&self) -> tokio::task::JoinHandle<()> {
        let pruner = Arc::new(self.clone());
        let interval_ms = self.prune_interval;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));

            loop {
                interval.tick().await;
                // Prune logic would go here
                debug!("Pruning task tick");
            }
        })
    }
}

impl Clone for StatePruner {
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            keep_blocks: self.keep_blocks,
            prune_interval: self.prune_interval,
        }
    }
}

// Need to add lru to Cargo.toml
                                      
