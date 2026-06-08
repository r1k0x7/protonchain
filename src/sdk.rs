// Proton SDK - Development toolkit for Proton Chain smart contracts

pub mod prelude {
    pub use super::*;
    pub use serde::{Serialize, Deserialize};
}

// Core types re-exported for contracts
pub use crate::types::*;

/// Contract macro (placeholder - would be proc_macro in real implementation)
pub use proton_macro::contract;

/// Storage operations
pub mod storage {
    use super::*;

    pub fn storage_get(key: &[u8]) -> Option<Vec<u8>> {
        // Call host function to read storage
        unsafe { host_storage_read(key.as_ptr(), key.len()) }
    }

    pub fn storage_set(key: &[u8], value: &[u8]) {
        // Call host function to write storage
        unsafe { host_storage_write(key.as_ptr(), key.len(), value.as_ptr(), value.len()) }
    }

    pub fn storage_remove(key: &[u8]) {
        unsafe { host_storage_delete(key.as_ptr(), key.len()) }
    }

    extern "C" {
        fn host_storage_read(key_ptr: *const u8, key_len: usize) -> Option<Vec<u8>>;
        fn host_storage_write(key_ptr: *const u8, key_len: usize, val_ptr: *const u8, val_len: usize);
        fn host_storage_delete(key_ptr: *const u8, key_len: usize);
    }
}

/// Context operations
pub mod context {
    use super::*;

    pub fn caller() -> Address {
        unsafe {
            let mut addr = [0u8; 20];
            host_caller(addr.as_mut_ptr());
            Address(addr)
        }
    }

    pub fn self_address() -> Address {
        unsafe {
            let mut addr = [0u8; 20];
            host_self_address(addr.as_mut_ptr());
            Address(addr)
        }
    }

    pub fn current_timestamp() -> u64 {
        unsafe { host_timestamp() }
    }

    pub fn current_block_height() -> u64 {
        unsafe { host_block_height() }
    }

    pub fn caller_is_owner() -> bool {
        unsafe { host_is_owner() }
    }

    pub fn is_protocol_address(addr: Address) -> bool {
        unsafe { host_is_protocol(addr.as_bytes().as_ptr()) }
    }

    pub fn chain_id() -> u64 {
        unsafe { host_chain_id() }
    }

    pub fn random_bytes(len: usize) -> Vec<u8> {
        unsafe {
            let mut bytes = vec![0u8; len];
            host_random(bytes.as_mut_ptr(), len);
            bytes
        }
    }

    extern "C" {
        fn host_caller(addr_ptr: *mut u8);
        fn host_self_address(addr_ptr: *mut u8);
        fn host_timestamp() -> u64;
        fn host_block_height() -> u64;
        fn host_is_owner() -> bool;
        fn host_is_protocol(addr_ptr: *const u8) -> bool;
        fn host_chain_id() -> u64;
        fn host_random(buf_ptr: *mut u8, len: usize);
    }
}

/// Token operations
pub mod token {
    use super::*;

    pub const PROTON_TOKEN: Address = Address([0u8; 20]); // Native token address

    pub fn transfer(token: Address, to: Address, amount: u128) {
        unsafe {
            host_transfer(
                token.as_bytes().as_ptr(),
                to.as_bytes().as_ptr(),
                amount,
            )
        }
    }

    pub fn transfer_from(token: Address, from: Address, to: Address, amount: u128) {
        unsafe {
            host_transfer_from(
                token.as_bytes().as_ptr(),
                from.as_bytes().as_ptr(),
                to.as_bytes().as_ptr(),
                amount,
            )
        }
    }

    pub fn balance_of(token: Address, account: Address) -> u128 {
        unsafe {
            host_balance(token.as_bytes().as_ptr(), account.as_bytes().as_ptr())
        }
    }

    pub fn mint_wrapped_tokens(token: Address, to: Address, amount: u128) {
        unsafe {
            host_mint(token.as_bytes().as_ptr(), to.as_bytes().as_ptr(), amount)
        }
    }

    pub fn private_transfer(token: Address, from: Address, to: Address, encrypted_amount: Vec<u8>) {
        unsafe {
            host_private_transfer(
                token.as_bytes().as_ptr(),
                from.as_bytes().as_ptr(),
                to.as_bytes().as_ptr(),
                encrypted_amount.as_ptr(),
                encrypted_amount.len(),
            )
        }
    }

    pub fn lock_private_assets(token: Address, from: Address, commitment: Hash) {
        unsafe {
            host_lock_private(
                token.as_bytes().as_ptr(),
                from.as_bytes().as_ptr(),
                commitment.as_bytes().as_ptr(),
            )
        }
    }

    extern "C" {
        fn host_transfer(token: *const u8, to: *const u8, amount: u128);
        fn host_transfer_from(token: *const u8, from: *const u8, to: *const u8, amount: u128);
        fn host_balance(token: *const u8, account: *const u8) -> u128;
        fn host_mint(token: *const u8, to: *const u8, amount: u128);
        fn host_private_transfer(token: *const u8, from: *const u8, to: *const u8, amount: *const u8, amount_len: usize);
        fn host_lock_private(token: *const u8, from: *const u8, commitment: *const u8);
    }
}

/// ZK verification helpers
pub mod zk {
    use super::*;

    pub fn verify_zk_proof(proof: &ZKProof, nullifier: &Hash, commitment: &Hash, root: &Hash) -> bool {
        unsafe {
            host_verify_zk(
                proof.proof_data.as_ptr(),
                proof.proof_data.len(),
                nullifier.as_bytes().as_ptr(),
                commitment.as_bytes().as_ptr(),
                root.as_bytes().as_ptr(),
            )
        }
    }

    pub fn verify_balance_proof(proof: &[u8], account: &Address, encrypted_amount: &[u8]) -> bool {
        unsafe {
            host_verify_balance(
                proof.as_ptr(),
                proof.len(),
                account.as_bytes().as_ptr(),
                encrypted_amount.as_ptr(),
                encrypted_amount.len(),
            )
        }
    }

    pub fn verify_swap_proof(proof: &ZKProof, pair: &(Address, Address), nullifier: &Hash, encrypted_amount: &[u8]) -> bool {
        unsafe {
            host_verify_swap(
                proof.proof_data.as_ptr(),
                proof.proof_data.len(),
                pair.0.as_bytes().as_ptr(),
                pair.1.as_bytes().as_ptr(),
                nullifier.as_bytes().as_ptr(),
                encrypted_amount.as_ptr(),
                encrypted_amount.len(),
            )
        }
    }

    pub fn verify_order_proof(proof: &[u8], order_hash: &Hash, stealth: &StealthAddress) -> bool {
        unsafe {
            host_verify_order(
                proof.as_ptr(),
                proof.len(),
                order_hash.as_bytes().as_ptr(),
                stealth.ephemeral_pubkey.as_ptr(),
            )
        }
    }

    pub fn verify_stake_proof(proof: &[u8], validator: &Address, commitment: &Hash, nullifier: &Hash) -> bool {
        unsafe {
            host_verify_stake(
                proof.as_ptr(),
                proof.len(),
                validator.as_bytes().as_ptr(),
                commitment.as_bytes().as_ptr(),
                nullifier.as_bytes().as_ptr(),
            )
        }
    }

    pub fn verify_metadata_proof(proof: &[u8], metadata_hash: &Hash, encrypted_metadata: &[u8]) -> bool {
        unsafe {
            host_verify_metadata(
                proof.as_ptr(),
                proof.len(),
                metadata_hash.as_bytes().as_ptr(),
                encrypted_metadata.as_ptr(),
                encrypted_metadata.len(),
            )
        }
    }

    pub fn verify_ownership_proof(proof: &ZKProof, token_id: &u256, current_stealth: &StealthAddress, nullifier: &Hash) -> bool {
        unsafe {
            host_verify_nft_ownership(
                proof.proof_data.as_ptr(),
                proof.proof_data.len(),
                token_id.to_le_bytes().as_ptr(),
                current_stealth.ephemeral_pubkey.as_ptr(),
                nullifier.as_bytes().as_ptr(),
            )
        }
    }

    pub fn verify_bridge_amount_proof(proof: &[u8], asset: &Address, commitment: &Hash) -> bool {
        unsafe {
            host_verify_bridge(
                proof.as_ptr(),
                proof.len(),
                asset.as_bytes().as_ptr(),
                commitment.as_bytes().as_ptr(),
            )
        }
    }

    pub fn verify_merkle_proof(proof: &[u8], transfer_id: &Hash) -> bool {
        unsafe {
            host_verify_merkle(
                proof.as_ptr(),
                proof.len(),
                transfer_id.as_bytes().as_ptr(),
            )
        }
    }

    pub fn verify_merkle_update_proof(proof: &[u8], chain_id: u64, new_root: &Hash) -> bool {
        unsafe {
            host_verify_merkle_update(
                proof.as_ptr(),
                proof.len(),
                chain_id,
                new_root.as_bytes().as_ptr(),
            )
        }
    }

    pub fn update_merkle_root(current: &Hash, commitment: &Hash) -> Hash {
        let mut data = Vec::new();
        data.extend_from_slice(current.as_bytes());
        data.extend_from_slice(commitment.as_bytes());
        Hash::new(&data)
    }

    extern "C" {
        fn host_verify_zk(proof: *const u8, proof_len: usize, nullifier: *const u8, commitment: *const u8, root: *const u8) -> bool;
        fn host_verify_balance(proof: *const u8, proof_len: usize, account: *const u8, encrypted_amount: *const u8, amount_len: usize) -> bool;
        fn host_verify_swap(proof: *const u8, proof_len: usize, token_a: *const u8, token_b: *const u8, nullifier: *const u8, encrypted_amount: *const u8, amount_len: usize) -> bool;
        fn host_verify_order(proof: *const u8, proof_len: usize, order_hash: *const u8, stealth: *const u8) -> bool;
        fn host_verify_stake(proof: *const u8, proof_len: usize, validator: *const u8, commitment: *const u8, nullifier: *const u8) -> bool;
        fn host_verify_metadata(proof: *const u8, proof_len: usize, metadata_hash: *const u8, encrypted_metadata: *const u8, metadata_len: usize) -> bool;
        fn host_verify_nft_ownership(proof: *const u8, proof_len: usize, token_id: *const u8, stealth: *const u8, nullifier: *const u8) -> bool;
        fn host_verify_bridge(proof: *const u8, proof_len: usize, asset: *const u8, commitment: *const u8) -> bool;
        fn host_verify_merkle(proof: *const u8, proof_len: usize, transfer_id: *const u8) -> bool;
        fn host_verify_merkle_update(proof: *const u8, proof_len: usize, chain_id: u64, new_root: *const u8) -> bool;
    }
}

/// Event emission
pub mod events {
    pub fn emit<T: serde::Serialize>(event: T) {
        let data = serde_json::to_vec(&event).unwrap_or_default();
        unsafe {
            host_emit(data.as_ptr(), data.len());
        }
    }

    extern "C" {
        fn host_emit(data: *const u8, len: usize);
    }
}

/// Re-export commonly used items
pub use context::*;
pub use events::emit;
pub use storage::*;
pub use token::*;
pub use zk::*;

/// u256 type for large integers
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct u256([u64; 4]);

impl u256 {
    pub fn zero() -> Self {
        Self([0; 4])
    }

    pub fn one() -> Self {
        Self([1, 0, 0, 0])
    }

    pub fn to_le_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(32);
        for limb in &self.0 {
            bytes.extend_from_slice(&limb.to_le_bytes());
        }
        bytes
    }
}

impl std::ops::Add for u256 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        let mut result = [0u64; 4];
        let mut carry = 0u64;
        for i in 0..4 {
            let (sum, c1) = self.0[i].overflowing_add(rhs.0[i]);
            let (sum, c2) = sum.overflowing_add(carry);
            result[i] = sum;
            carry = if c1 || c2 { 1 } else { 0 };
        }
        Self(result)
    }
}

impl std::ops::Sub for u256 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        let mut result = [0u64; 4];
        let mut borrow = 0u64;
        for i in 0..4 {
            let (diff, b1) = self.0[i].overflowing_sub(rhs.0[i]);
            let (diff, b2) = diff.overflowing_sub(borrow);
            result[i] = diff;
            borrow = if b1 || b2 { 1 } else { 0 };
        }
        Self(result)
    }
}

impl From<u128> for u256 {
    fn from(value: u128) -> Self {
        let mut result = [0u64; 4];
        result[0] = value as u64;
        result[1] = (value >> 64) as u64;
        Self(result)
    }
}

/// Collection types for contract storage
pub struct Map<K, V> {
    _phantom: std::marker::PhantomData<(K, V)>,
}

impl<K, V> Map<K, V> {
    pub fn new() -> Self {
        Self { _phantom: std::marker::PhantomData }
    }

    pub fn get(&self, key: &K) -> Option<V> where K: serde::Serialize, V: serde::de::DeserializeOwned {
        let key_bytes = serde_json::to_vec(key).ok()?;
        let value_bytes = storage_get(&key_bytes)?;
        serde_json::from_slice(&value_bytes).ok()
    }

    pub fn insert(&mut self, key: K, value: V) where K: serde::Serialize, V: serde::Serialize {
        let key_bytes = serde_json::to_vec(&key).unwrap();
        let value_bytes = serde_json::to_vec(&value).unwrap();
        storage_set(&key_bytes, &value_bytes);
    }

    pub fn remove(&mut self, key: &K) where K: serde::Serialize {
        let key_bytes = serde_json::to_vec(key).unwrap();
        storage_remove(&key_bytes);
    }

    pub fn contains_key(&self, key: &K) -> bool where K: serde::Serialize {
        self.get(key).is_some()
    }

    pub fn len(&self) -> usize {
        // Would need to track size separately in real implementation
        0
    }

    pub fn iter(&self) -> Vec<(K, V)> where K: serde::Serialize + serde::de::DeserializeOwned, V: serde::de::DeserializeOwned {
        // Would need proper iteration in real implementation
        vec![]
    }
}

pub struct Set<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> Set<T> {
    pub fn new() -> Self {
        Self { _phantom: std::marker::PhantomData }
    }

    pub fn insert(&mut self, value: T) where T: serde::Serialize {
        let key = serde_json::to_vec(&value).unwrap();
        storage_set(&key, &[1]);
    }

    pub fn contains(&self, value: &T) -> bool where T: serde::Serialize {
        let key = serde_json::to_vec(value).unwrap();
        storage_get(&key).is_some()
    }

    pub fn remove(&mut self, value: &T) where T: serde::Serialize {
        let key = serde_json::to_vec(value).unwrap();
        storage_remove(&key);
    }
}

/// Assertion helpers
pub fn require(condition: bool, message: &str) {
    if !condition {
        panic!("{}", message);
    }
}

/// Re-export for convenience
pub use std::collections::HashMap;
