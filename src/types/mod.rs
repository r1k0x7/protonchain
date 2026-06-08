use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// 32-byte hash used throughout the system
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hash([u8; 32]);

impl Hash {
    pub fn new(data: &[u8]) -> Self {
        let mut hasher = Sha3_256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        Self(hash)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn zero() -> Self {
        Self([0u8; 32])
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.to_hex())
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Address type (20 bytes like Ethereum but with Proton prefix)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address([u8; 20]);

impl Address {
    pub fn new(pubkey: &[u8]) -> Self {
        let hash = Hash::new(pubkey);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash.as_bytes()[12..32]);
        Self(addr)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    pub fn to_proton_address(&self) -> String {
        format!("proton_{}", bs58::encode(self.0).into_string())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self.to_proton_address())
    }
}

/// Stealth address for privacy
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StealthAddress {
    pub ephemeral_pubkey: [u8; 32],
    pub encrypted_view_tag: [u8; 32],
}

impl StealthAddress {
    pub fn derive(view_key: &[u8], spend_key: &[u8], nonce: u64) -> Self {
        let mut hasher = Sha3_256::new();
        hasher.update(view_key);
        hasher.update(&nonce.to_le_bytes());
        let mut ephemeral = [0u8; 32];
        ephemeral.copy_from_slice(&hasher.finalize());

        let mut hasher2 = Sha3_256::new();
        hasher2.update(spend_key);
        hasher2.update(&ephemeral);
        let mut view_tag = [0u8; 32];
        view_tag.copy_from_slice(&hasher2.finalize());

        Self {
            ephemeral_pubkey: ephemeral,
            encrypted_view_tag: view_tag,
        }
    }
}

/// Transaction types
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionType {
    Transfer,
    ContractDeploy,
    ContractCall,
    Stake,
    Unstake,
    CrossChain,
    PrivateTransfer,
}

/// Transaction structure
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub tx_type: TransactionType,
    pub nonce: u64,
    pub from: Address,
    pub to: Option<Address>,
    pub value: u128,
    pub gas_price: u128,
    pub gas_limit: u64,
    pub data: Vec<u8>,
    pub shard_id: u16,
    pub timestamp: u64,
    pub signature: Vec<u8>,
    // Privacy fields
    pub stealth_address: Option<StealthAddress>,
    pub zk_proof: Option<Vec<u8>>,
    pub encrypted_amount: Option<Vec<u8>>,
}

impl Transaction {
    pub fn hash(&self) -> Hash {
        let encoded = bincode::serialize(self).unwrap_or_default();
        Hash::new(&encoded)
    }

    pub fn is_private(&self) -> bool {
        self.tx_type == TransactionType::PrivateTransfer || 
        self.zk_proof.is_some()
    }

    pub fn gas_cost(&self) -> u128 {
        self.gas_price * self.gas_limit as u128
    }
}

/// Block header
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockHeader {
    pub version: u32,
    pub prev_hash: Hash,
    pub merkle_root: Hash,
    pub state_root: Hash,
    pub shard_id: u16,
    pub height: u64,
    pub timestamp: u64,
    pub validator: Address,
    pub signature: Vec<u8>,
    pub tx_count: u32,
    pub gas_used: u64,
    pub extra_data: Vec<u8>,
}

impl BlockHeader {
    pub fn hash(&self) -> Hash {
        let encoded = bincode::serialize(self).unwrap_or_default();
        Hash::new(&encoded)
    }
}

/// Full block
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    pub shard_proofs: Vec<ShardProof>,
}

/// Shard proof for cross-chain validation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardProof {
    pub shard_id: u16,
    pub block_hash: Hash,
    pub state_root: Hash,
    pub signature: Vec<u8>,
}

/// Account state
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Account {
    pub nonce: u64,
    pub balance: u128,
    pub code_hash: Option<Hash>,
    pub storage_root: Hash,
    pub validator_stake: u128,
    pub is_contract: bool,
    // Privacy
    pub view_key: Option<Vec<u8>>,
    pub spend_key: Option<Vec<u8>>,
}

/// Chain configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainConfig {
    pub chain_id: u64,
    pub shard_count: u16,
    pub block_time_ms: u64,
    pub min_gas_price: u128,
    pub max_block_gas: u64,
    pub validator_count: u32,
    pub epoch_length: u64,
    pub privacy_enabled: bool,
    pub cross_chain_enabled: bool,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            chain_id: 1337,
            shard_count: 64,
            block_time_ms: 300,
            min_gas_price: 1,
            max_block_gas: 50_000_000,
            validator_count: 128,
            epoch_length: 1000,
            privacy_enabled: true,
            cross_chain_enabled: true,
        }
    }
}

/// Current timestamp in milliseconds
pub fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Merkle tree implementation
pub struct MerkleTree;

impl MerkleTree {
    pub fn root(leaves: &[Hash]) -> Hash {
        if leaves.is_empty() {
            return Hash::zero();
        }

        let mut current = leaves.to_vec();
        while current.len() > 1 {
            let mut next = Vec::new();
            for chunk in current.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { left };
                let mut combined = Vec::with_capacity(64);
                combined.extend_from_slice(left.as_bytes());
                combined.extend_from_slice(right.as_bytes());
                next.push(Hash::new(&combined));
            }
            current = next;
        }
        current[0].clone()
    }
  }
  
