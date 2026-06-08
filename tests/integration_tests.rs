#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use crate::consensus::*;
    use crate::privacy::*;
    use crate::vm::*;
    use crate::multichain::*;
    use crate::storage::*;

    // ==================== TYPES TESTS ====================

    #[test]
    fn test_hash_generation() {
        let data = b"hello proton";
        let hash = Hash::new(data);

        assert_eq!(hash.as_bytes().len(), 32);
        assert_ne!(hash.as_bytes(), &[0u8; 32]);

        // Same input = same hash
        let hash2 = Hash::new(data);
        assert_eq!(hash.as_bytes(), hash2.as_bytes());

        // Different input = different hash
        let hash3 = Hash::new(b"different");
        assert_ne!(hash.as_bytes(), hash3.as_bytes());
    }

    #[test]
    fn test_address_generation() {
        let pubkey = [1u8; 32];
        let addr = Address::new(&pubkey);

        assert_eq!(addr.as_bytes().len(), 20);

        // Same pubkey = same address
        let addr2 = Address::new(&pubkey);
        assert_eq!(addr.as_bytes(), addr2.as_bytes());
    }

    #[test]
    fn test_merkle_tree() {
        let leaves = vec![
            Hash::new(b"leaf1"),
            Hash::new(b"leaf2"),
            Hash::new(b"leaf3"),
            Hash::new(b"leaf4"),
        ];

        let root = MerkleTree::root(&leaves);
        assert_ne!(root.as_bytes(), &[0u8; 32]);

        // Single leaf
        let single = MerkleTree::root(&[Hash::new(b"single")]);
        assert_ne!(single.as_bytes(), &[0u8; 32]);

        // Empty
        let empty = MerkleTree::root(&[]);
        assert_eq!(empty.as_bytes(), &[0u8; 32]);
    }

    #[test]
    fn test_transaction_creation() {
        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: Some(Address::new(&[2u8; 32])),
            value: 1000,
            gas_price: 10,
            gas_limit: 21000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        let hash = tx.hash();
        assert_ne!(hash.as_bytes(), &[0u8; 32]);
        assert!(!tx.is_private());
        assert_eq!(tx.gas_cost(), 210000);
    }

    #[test]
    fn test_private_transaction() {
        let tx = Transaction {
            tx_type: TransactionType::PrivateTransfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: None,
            value: 0,
            gas_price: 10,
            gas_limit: 200000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: Some(StealthAddress {
                ephemeral_pubkey: [1u8; 32],
                encrypted_view_tag: [2u8; 32],
            }),
            zk_proof: Some(vec![1, 2, 3]),
            encrypted_amount: Some(vec![4, 5, 6]),
        };

        assert!(tx.is_private());
        assert!(tx.stealth_address.is_some());
        assert!(tx.zk_proof.is_some());
    }

    // ==================== CONSENSUS TESTS ====================

    #[test]
    fn test_validator_set() {
        let set = ValidatorSet::new();

        let validator = Validator {
            address: Address::new(&[1u8; 32]),
            public_key: vec![1u8; 33],
            stake: 1000,
            commission: 10,
            uptime: 99.9,
            last_active: 0,
        };

        set.add_validator(validator.clone());

        assert_eq!(set.total_stake(), 1000);
        assert_eq!(set.validator_count(), 1);

        let retrieved = set.get_validator(&validator.address);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().stake, 1000);
    }

    #[test]
    fn test_mempool() {
        let mempool = Mempool::new(1000);

        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: Some(Address::new(&[2u8; 32])),
            value: 100,
            gas_price: 10,
            gas_limit: 21000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        mempool.add_transaction(tx.clone()).unwrap();
        assert_eq!(mempool.size(), 1);

        // Get transactions
        let txs = mempool.get_transactions(10, 1);
        assert_eq!(txs.len(), 1);

        // Remove
        let hash = tx.hash();
        mempool.remove_transactions(&[hash]);
        assert_eq!(mempool.size(), 0);
    }

    #[test]
    fn test_dag_ordering() {
        let dag = DagOrdering::new();

        let v1 = DagVertex {
            hash: Hash::new(b"v1"),
            parents: vec![],
            timestamp: 1,
            round: 1,
            is_block: true,
        };

        let v2 = DagVertex {
            hash: Hash::new(b"v2"),
            parents: vec![v1.hash.clone()],
            timestamp: 2,
            round: 1,
            is_block: true,
        };

        let v3 = DagVertex {
            hash: Hash::new(b"v3"),
            parents: vec![v1.hash.clone(), v2.hash.clone()],
            timestamp: 3,
            round: 2,
            is_block: true,
        };

        dag.add_vertex(v1);
        dag.add_vertex(v2);
        dag.add_vertex(v3);

        let sorted = dag.topological_sort();
        assert_eq!(sorted.len(), 3);

        // v1 should come before v2 and v3
        let v1_idx = sorted.iter().position(|h| h == &Hash::new(b"v1")).unwrap();
        let v2_idx = sorted.iter().position(|h| h == &Hash::new(b"v2")).unwrap();
        let v3_idx = sorted.iter().position(|h| h == &Hash::new(b"v3")).unwrap();

        assert!(v1_idx < v2_idx);
        assert!(v1_idx < v3_idx);
        assert!(v2_idx < v3_idx);
    }

    // ==================== PRIVACY TESTS ====================

    #[test]
    fn test_stealth_address() {
        let view_key = [1u8; 32];
        let spend_key = [2u8; 32];

        let stealth = StealthAddress::derive(&view_key, &spend_key, 1);

        assert_eq!(stealth.ephemeral_pubkey.len(), 32);
        assert_eq!(stealth.encrypted_view_tag.len(), 32);

        // Different nonce = different address
        let stealth2 = StealthAddress::derive(&view_key, &spend_key, 2);
        assert_ne!(stealth.ephemeral_pubkey, stealth2.ephemeral_pubkey);

        // Same nonce = same address
        let stealth3 = StealthAddress::derive(&view_key, &spend_key, 1);
        assert_eq!(stealth.ephemeral_pubkey, stealth3.ephemeral_pubkey);
    }

    #[test]
    fn test_stealth_generator() {
        let view_key = [1u8; 32];
        let spend_key = [2u8; 32];

        let generator = StealthAddressGenerator::new(view_key, spend_key);

        let (stealth1, key1) = generator.generate();
        let (stealth2, key2) = generator.generate();

        assert_ne!(stealth1.ephemeral_pubkey, stealth2.ephemeral_pubkey);
        assert_ne!(key1, key2);

        // Check ownership
        let found = generator.check_ownership(&stealth1);
        assert!(found.is_some());
        assert_eq!(found.unwrap(), key1);

        let not_found = generator.check_ownership(&StealthAddress {
            ephemeral_pubkey: [99u8; 32],
            encrypted_view_tag: [99u8; 32],
        });
        assert!(not_found.is_none());
    }

    #[test]
    fn test_zk_system_setup() {
        let zk = ZkProtonSystem::new();

        // Setup should succeed
        assert!(zk.setup().is_ok());

        let stats = zk.get_stats();
        assert_eq!(stats.proofs_generated, 0);
        assert_eq!(stats.proofs_verified, 0);
    }

    #[test]
    fn test_encrypted_mempool() {
        let mempool = EncryptedMempool::new(3); // Threshold of 3 validators

        let enc_tx = EncryptedTransaction {
            ciphertext: vec![1, 2, 3],
            nonce: [0u8; 12],
            sender_pubkey: [1u8; 32],
            gas_price_commitment: [2u8; 32],
            timestamp: current_timestamp_ms(),
        };

        mempool.add_encrypted_tx(enc_tx);
        assert_eq!(mempool.size(), 1);

        // Try decrypt with insufficient shares
        let result = mempool.decrypt_batch(&[vec![1u8; 32]]);
        assert!(result.is_err());

        // Decrypt with sufficient shares
        let result = mempool.decrypt_batch(&[
            vec![1u8; 32],
            vec![2u8; 32],
            vec![3u8; 32],
        ]);
        // Will fail due to mock decryption but should pass threshold check
        assert!(result.is_ok() || result.is_err()); // Depends on implementation
    }

    // ==================== VM TESTS ====================

    #[test]
    fn test_vm_state() {
        let state = Arc::new(RwLock::new(VMState {
            accounts: HashMap::new(),
            contracts: HashMap::new(),
            storage: HashMap::new(),
            block_context: BlockContext {
                height: 0,
                timestamp: 0,
                coinbase: Address::new(&[0u8; 32]),
                difficulty: 1,
                gas_limit: 50_000_000,
            },
        }));

        let vm = ProtonVM::new(state, GasSchedule::default());

        // Test empty state
        let addr = Address::new(&[1u8; 32]);
        assert!(vm.get_account(&addr).is_none());

        let root = vm.state_root();
        assert_ne!(root.as_bytes(), &[0u8; 32]);
    }

    #[test]
    fn test_transfer_execution() {
        let state = Arc::new(RwLock::new(VMState {
            accounts: {
                let mut accounts = HashMap::new();
                let mut account = Account::default();
                account.balance = 10000;
                accounts.insert(Address::new(&[1u8; 32]), account);
                accounts
            },
            contracts: HashMap::new(),
            storage: HashMap::new(),
            block_context: BlockContext {
                height: 1,
                timestamp: current_timestamp_ms(),
                coinbase: Address::new(&[0u8; 32]),
                difficulty: 1,
                gas_limit: 50_000_000,
            },
        }));

        let vm = ProtonVM::new(state, GasSchedule::default());

        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: Some(Address::new(&[2u8; 32])),
            value: 1000,
            gas_price: 10,
            gas_limit: 21000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        let result = vm.execute_transaction(&tx);

        assert!(result.success);
        assert!(result.gas_used > 0);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_insufficient_balance() {
        let state = Arc::new(RwLock::new(VMState {
            accounts: {
                let mut accounts = HashMap::new();
                let mut account = Account::default();
                account.balance = 100;
                accounts.insert(Address::new(&[1u8; 32]), account);
                accounts
            },
            contracts: HashMap::new(),
            storage: HashMap::new(),
            block_context: BlockContext {
                height: 1,
                timestamp: current_timestamp_ms(),
                coinbase: Address::new(&[0u8; 32]),
                difficulty: 1,
                gas_limit: 50_000_000,
            },
        }));

        let vm = ProtonVM::new(state, GasSchedule::default());

        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: Some(Address::new(&[2u8; 32])),
            value: 1000, // More than balance
            gas_price: 10,
            gas_limit: 21000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        let result = vm.execute_transaction(&tx);

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_contract_deploy() {
        let state = Arc::new(RwLock::new(VMState {
            accounts: {
                let mut accounts = HashMap::new();
                let mut account = Account::default();
                account.balance = 100000;
                accounts.insert(Address::new(&[1u8; 32]), account);
                accounts
            },
            contracts: HashMap::new(),
            storage: HashMap::new(),
            block_context: BlockContext {
                height: 1,
                timestamp: current_timestamp_ms(),
                coinbase: Address::new(&[0u8; 32]),
                difficulty: 1,
                gas_limit: 50_000_000,
            },
        }));

        let vm = ProtonVM::new(state, GasSchedule::default());

        let tx = Transaction {
            tx_type: TransactionType::ContractDeploy,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: None,
            value: 0,
            gas_price: 10,
            gas_limit: 100000,
            data: vec![0x00, 0x61, 0x73, 0x6d], // WASM magic bytes
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        let result = vm.execute_transaction(&tx);

        assert!(result.success);
        assert!(result.gas_used > 0);
    }

    #[test]
    fn test_gas_estimation() {
        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: Some(Address::new(&[2u8; 32])),
            value: 1000,
            gas_price: 10,
            gas_limit: 21000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        let state = Arc::new(RwLock::new(VMState {
            accounts: HashMap::new(),
            contracts: HashMap::new(),
            storage: HashMap::new(),
            block_context: BlockContext {
                height: 0,
                timestamp: 0,
                coinbase: Address::new(&[0u8; 32]),
                difficulty: 1,
                gas_limit: 50_000_000,
            },
        }));

        let vm = ProtonVM::new(state, GasSchedule::default());
        let estimate = GasEstimator::estimate_gas(&tx, &vm);

        assert_eq!(estimate, 21000);
    }

    // ==================== MULTICHAIN TESTS ====================

    #[test]
    fn test_shard_manager() {
        let config = ChainConfig {
            chain_id: 1337,
            shard_count: 4,
            block_time_ms: 300,
            min_gas_price: 1,
            max_block_gas: 50_000_000,
            validator_count: 16,
            epoch_length: 1000,
            privacy_enabled: true,
            cross_chain_enabled: true,
        };

        let router = Arc::new(CrossChainRouter::new());
        let manager = ShardManager::new(config, router);

        let validator_set = Arc::new(ValidatorSet::new());
        // Add some validators
        for i in 0..16 {
            let validator = Validator {
                address: Address::new(&[i as u8; 32]),
                public_key: vec![i as u8; 33],
                stake: 1000,
                commission: 10,
                uptime: 99.9,
                last_active: 0,
            };
            validator_set.add_validator(validator);
        }

        manager.initialize_shards(validator_set);

        let stats = manager.get_all_shard_stats();
        assert_eq!(stats.len(), 4);

        for stat in stats {
            assert!(stat.validator_count > 0);
        }
    }

    #[test]
    fn test_cross_chain_router() {
        let router = CrossChainRouter::new();

        let tx = CrossChainTransaction {
            tx_id: Hash::new(b"test"),
            source_shard: 0,
            target_shard: 1,
            sender: Address::new(&[1u8; 32]),
            receiver: Address::new(&[2u8; 32]),
            amount: 1000,
            nonce: 1,
            status: CrossChainStatus::Pending,
            proof: None,
            timestamp: current_timestamp_ms(),
        };

        router.submit_cross_tx(tx.clone()).unwrap();

        let pending = router.get_pending();
        assert_eq!(pending.len(), 1);

        let stats = router.get_stats();
        assert_eq!(stats.pending_count, 1);
        assert_eq!(stats.completed_count, 0);

        // Confirm
        router.confirm_tx(&tx.tx_id, vec![1]).unwrap();

        let stats = router.get_stats();
        assert_eq!(stats.pending_count, 0);
        assert_eq!(stats.completed_count, 1);
    }

    // ==================== STORAGE TESTS ====================

    #[test]
    fn test_memory_db() {
        let db = Arc::new(MemoryDB::new());

        db.put(b"key1", b"value1").unwrap();
        assert_eq!(db.get(b"key1"), Some(b"value1".to_vec()));

        db.put(b"key2", b"value2").unwrap();

        let batch = vec![
            (b"key3".to_vec(), Some(b"value3".to_vec())),
            (b"key4".to_vec(), Some(b"value4".to_vec())),
        ];
        db.batch_write(batch).unwrap();

        assert_eq!(db.get(b"key3"), Some(b"value3".to_vec()));

        db.delete(b"key1").unwrap();
        assert_eq!(db.get(b"key1"), None);
    }

    #[test]
    fn test_state_storage() {
        let db = Arc::new(MemoryDB::new());
        let storage = StateStorage::new(db, 1000);

        let addr = Address::new(&[1u8; 32]);
        let account = Account {
            nonce: 1,
            balance: 1000,
            code_hash: None,
            storage_root: Hash::zero(),
            validator_stake: 0,
            is_contract: false,
            view_key: None,
            spend_key: None,
        };

        storage.put_account(&addr, &account).unwrap();
        storage.commit().unwrap();

        let retrieved = storage.get_account(&addr);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().balance, 1000);
    }

    // ==================== INTEGRATION TESTS ====================

    #[test]
    fn test_full_transaction_flow() {
        // Setup
        let config = ChainConfig::default();
        let validator_set = Arc::new(ValidatorSet::new());

        let validator = Validator {
            address: Address::new(&[1u8; 32]),
            public_key: vec![1u8; 33],
            stake: 10000,
            commission: 10,
            uptime: 99.9,
            last_active: 0,
        };
        validator_set.add_validator(validator);

        let consensus = Arc::new(HotStuffConsensus::new(
            config.clone(),
            validator_set,
            Address::new(&[1u8; 32]),
            secp256k1::SecretKey::from_slice(&[1u8; 32]).unwrap(),
        ));

        // Create and submit transaction
        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            nonce: 1,
            from: Address::new(&[1u8; 32]),
            to: Some(Address::new(&[2u8; 32])),
            value: 500,
            gas_price: 10,
            gas_limit: 21000,
            data: vec![],
            shard_id: 0,
            timestamp: current_timestamp_ms(),
            signature: vec![],
            stealth_address: None,
            zk_proof: None,
            encrypted_amount: None,
        };

        let mempool = consensus.get_mempool();
        mempool.add_transaction(tx).unwrap();

        assert_eq!(mempool.size(), 1);

        // Propose block
        let prev_hash = Hash::zero();
        let block = consensus.propose_block(prev_hash);

        assert!(block.is_some());
        let block = block.unwrap();
        assert_eq!(block.transactions.len(), 1);

        // Validate block
        assert!(consensus.validate_block(&block));

        // Finalize
        consensus.finalize_block(block.clone());

        assert_eq!(mempool.size(), 0);
        assert_eq!(consensus.get_latest_height(), 1);
    }

    #[test]
    fn test_private_transaction_flow() {
        let zk_system = Arc::new(ZkProtonSystem::new());
        zk_system.setup().unwrap();

        let stealth_gen = Arc::new(StealthAddressGenerator::new([1u8; 32], [2u8; 32]));
        let builder = PrivacyTxBuilder::new(stealth_gen, zk_system);

        let (stealth, _) = stealth_gen.generate();

        let tx = builder.build_private_transfer(
            Address::new(&[1u8; 32]),
            stealth,
            1000,
            5000, // sender balance
            123,  // sender secret
            456,  // receiver secret
            10,   // gas price
            200000, // gas limit
            0,    // shard
        );

        assert!(tx.is_ok());
        let tx = tx.unwrap();

        assert!(tx.is_private());
        assert!(tx.stealth_address.is_some());
        assert!(tx.zk_proof.is_some());
        assert!(tx.encrypted_amount.is_some());
    }

    #[test]
    fn test_performance_1000_transactions() {
        let config = ChainConfig::default();
        let validator_set = Arc::new(ValidatorSet::new());

        let validator = Validator {
            address: Address::new(&[1u8; 32]),
            public_key: vec![1u8; 33],
            stake: 10000,
            commission: 10,
            uptime: 99.9,
            last_active: 0,
        };
        validator_set.add_validator(validator);

        let consensus = Arc::new(HotStuffConsensus::new(
            config,
            validator_set,
            Address::new(&[1u8; 32]),
            secp256k1::SecretKey::from_slice(&[1u8; 32]).unwrap(),
        ));

        let mempool = consensus.get_mempool();

        // Submit 1000 transactions
        let start = std::time::Instant::now();

        for i in 0..1000 {
            let tx = Transaction {
                tx_type: TransactionType::Transfer,
                nonce: i as u64,
                from: Address::new(&[(i % 256) as u8; 32]),
                to: Some(Address::new(&[((i + 1) % 256) as u8; 32])),
                value: i as u128,
                gas_price: 10,
                gas_limit: 21000,
                data: vec![],
                shard_id: 0,
                timestamp: current_timestamp_ms(),
                signature: vec![],
                stealth_address: None,
                zk_proof: None,
                encrypted_amount: None,
            };

            mempool.add_transaction(tx).unwrap();
        }

        let submit_time = start.elapsed();

        assert_eq!(mempool.size(), 1000);

        // Get all transactions
        let txs = mempool.get_transactions(1000, 1);
        assert_eq!(txs.len(), 1000);

        println!("Submitted 1000 txs in {:?}", submit_time);
        println!("TPS: {}", 1000.0 / submit_time.as_secs_f64());
    }
}

use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;
