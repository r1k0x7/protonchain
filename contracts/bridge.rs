// Proton Cross-Chain Bridge - Secure asset bridging with ZK proofs

use proton_sdk::*;

#[contract]
mod proton_bridge {
    use proton_sdk::prelude::*;

    #[state]
    pub struct ProtonBridge {
        // Supported chains
        supported_chains: Map<u64, ChainConfig>,
        // Wrapped assets per chain
        wrapped_assets: Map<(u64, Address), Address>, // (chain_id, remote_asset) -> local_asset
        // Locked assets per chain
        locked_assets: Map<(u64, Address), u128>, // (chain_id, asset) -> amount
        // Bridge validators (multisig)
        validators: Map<Address, ValidatorInfo>,
        validator_threshold: u16,
        // Pending cross-chain transfers
        pending_transfers: Map<Hash, PendingTransfer>,
        // Completed transfers (nullifier set)
        completed_transfers: Set<Hash>,
        // Merkle root for light client verification
        merkle_roots: Map<u64, Hash>, // chain_id -> merkle_root
        // ZK verification key
        zk_vk: Vec<u8>,
    }

    #[derive(Clone, Debug)]
    pub struct ChainConfig {
        chain_id: u64,
        bridge_contract: Address, // Remote bridge contract address
        is_active: bool,
        gas_limit: u64,
        confirmation_blocks: u64,
    }

    #[derive(Clone, Debug)]
    pub struct ValidatorInfo {
        address: Address,
        stake: u128,
        is_active: bool,
        last_signature: u64,
    }

    #[derive(Clone, Debug)]
    pub struct PendingTransfer {
        transfer_id: Hash,
        source_chain: u64,
        target_chain: u64,
        sender: Address,
        receiver: Address,
        asset: Address,
        amount: u128,
        signatures: Vec<(Address, Vec<u8>)>,
        status: TransferStatus,
        timestamp: u64,
        // Privacy
        encrypted_amount: Option<Vec<u8>>,
        stealth_receiver: Option<StealthAddress>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum TransferStatus {
        Pending,
        SourceConfirmed,
        SignaturesComplete,
        Executed,
        Failed,
    }

    #[event]
    pub struct TransferInitiated {
        #[indexed]
        transfer_id: Hash,
        source_chain: u64,
        target_chain: u64,
        sender: Address,
        receiver: Address,
        asset: Address,
        amount: u128,
    }

    #[event]
    pub struct TransferSigned {
        #[indexed]
        transfer_id: Hash,
        #[indexed]
        validator: Address,
        signature_count: u16,
    }

    #[event]
    pub struct TransferExecuted {
        #[indexed]
        transfer_id: Hash,
        target_chain: u64,
        receiver: Address,
        amount: u128,
    }

    #[event]
    pub struct ChainAdded {
        chain_id: u64,
        bridge_contract: Address,
    }

    impl ProtonBridge {
        #[constructor]
        pub fn new(validator_threshold: u16) -> Self {
            Self {
                supported_chains: Map::new(),
                wrapped_assets: Map::new(),
                locked_assets: Map::new(),
                validators: Map::new(),
                validator_threshold,
                pending_transfers: Map::new(),
                completed_transfers: Set::new(),
                merkle_roots: Map::new(),
                zk_vk: vec![],
            }
        }

        /// Add supported chain (governance)
        #[public]
        pub fn add_chain(
            &mut self,
            chain_id: u64,
            bridge_contract: Address,
            confirmation_blocks: u64,
        ) {
            require!(caller_is_owner(), "Only owner");
            require!(
                !self.supported_chains.contains_key(&chain_id),
                "Chain already added"
            );

            let config = ChainConfig {
                chain_id,
                bridge_contract,
                is_active: true,
                gas_limit: 500_000,
                confirmation_blocks,
            };

            self.supported_chains.insert(chain_id, config);

            emit!(ChainAdded {
                chain_id,
                bridge_contract,
            });
        }

        /// Register wrapped asset
        #[public]
        pub fn register_wrapped_asset(
            &mut self,
            chain_id: u64,
            remote_asset: Address,
            local_asset: Address,
        ) {
            require!(caller_is_owner(), "Only owner");

            let key = (chain_id, remote_asset);
            self.wrapped_assets.insert(key, local_asset);
        }

        /// Add validator
        #[public]
        pub fn add_validator(&mut self, validator: Address, stake: u128) {
            require!(caller_is_owner(), "Only owner");

            let info = ValidatorInfo {
                address: validator,
                stake,
                is_active: true,
                last_signature: 0,
            };

            self.validators.insert(validator, info);
        }

        /// Initiate cross-chain transfer (lock assets)
        #[public]
        pub fn initiate_transfer(
            &mut self,
            target_chain: u64,
            receiver: Address,
            asset: Address,
            amount: u128,
        ) -> Hash {
            let chain = self.supported_chains.get(&target_chain)
                .expect("Chain not supported");
            require!(chain.is_active, "Chain not active");

            let sender = caller();
            let transfer_id = self.generate_transfer_id(
                target_chain,
                sender,
                receiver,
                asset,
                amount,
            );

            require!(
                !self.completed_transfers.contains(&transfer_id),
                "Transfer already completed"
            );

            // Lock assets in bridge
            transfer_from(asset, sender, self_address(), amount);

            let key = (target_chain, asset);
            let current = self.locked_assets.get(&key).unwrap_or(0);
            self.locked_assets.insert(key, current + amount);

            let transfer = PendingTransfer {
                transfer_id: transfer_id.clone(),
                source_chain: CHAIN_ID, // Current chain
                target_chain,
                sender,
                receiver,
                asset,
                amount,
                signatures: Vec::new(),
                status: TransferStatus::Pending,
                timestamp: current_timestamp(),
                encrypted_amount: None,
                stealth_receiver: None,
            };

            self.pending_transfers.insert(transfer_id.clone(), transfer);

            emit!(TransferInitiated {
                transfer_id: transfer_id.clone(),
                source_chain: CHAIN_ID,
                target_chain,
                sender,
                receiver,
                asset,
                amount,
            });

            transfer_id
        }

        /// Initiate private cross-chain transfer
        #[private]
        pub fn initiate_private_transfer(
            &mut self,
            target_chain: u64,
            stealth_receiver: StealthAddress,
            asset: Address,
            encrypted_amount: Vec<u8>,
            amount_commitment: Hash,
            zk_proof: Vec<u8>,
        ) -> Hash {
            require!(
                verify_bridge_amount_proof(&zk_proof, &asset, &amount_commitment),
                "Invalid amount proof"
            );

            let sender = caller();
            let transfer_id = Hash::new(&[
                &target_chain.to_le_bytes(),
                sender.as_bytes(),
                &encrypted_amount,
                &current_timestamp().to_le_bytes(),
            ].concat());

            // Lock assets (amount hidden)
            lock_private_assets(asset, sender, amount_commitment);

            let transfer = PendingTransfer {
                transfer_id: transfer_id.clone(),
                source_chain: CHAIN_ID,
                target_chain,
                sender,
                receiver: Address::new(&[0u8; 32]), // Hidden
                asset,
                amount: 0, // Hidden
                signatures: Vec::new(),
                status: TransferStatus::Pending,
                timestamp: current_timestamp(),
                encrypted_amount: Some(encrypted_amount),
                stealth_receiver: Some(stealth_receiver),
            };

            self.pending_transfers.insert(transfer_id.clone(), transfer);

            transfer_id
        }

        /// Sign transfer (validator only)
        #[public]
        pub fn sign_transfer(
            &mut self,
            transfer_id: Hash,
            signature: Vec<u8>,
            merkle_proof: Vec<u8>,
        ) {
            let validator = caller();
            let info = self.validators.get(&validator)
                .expect("Not a validator");
            require!(info.is_active, "Validator not active");

            let mut transfer = self.pending_transfers.get(&transfer_id)
                .expect("Transfer not found");

            require!(
                transfer.status == TransferStatus::Pending ||
                transfer.status == TransferStatus::SourceConfirmed,
                "Invalid transfer status"
            );

            // Verify signature
            require!(
                verify_validator_signature(&validator, &transfer_id, &signature),
                "Invalid signature"
            );

            // Verify merkle proof (light client verification)
            require!(
                verify_merkle_proof(&merkle_proof, &transfer_id),
                "Invalid merkle proof"
            );

            transfer.signatures.push((validator, signature));

            let sig_count = transfer.signatures.len() as u16;

            if sig_count >= self.validator_threshold {
                transfer.status = TransferStatus::SignaturesComplete;
            } else {
                transfer.status = TransferStatus::SourceConfirmed;
            }

            self.pending_transfers.insert(transfer_id.clone(), transfer);

            emit!(TransferSigned {
                transfer_id,
                validator,
                signature_count: sig_count,
            });
        }

        /// Execute transfer on target chain (release/mint assets)
        #[public]
        pub fn execute_transfer(&mut self, transfer_id: Hash) {
            let mut transfer = self.pending_transfers.get(&transfer_id)
                .expect("Transfer not found");

            require!(
                transfer.status == TransferStatus::SignaturesComplete,
                "Insufficient signatures"
            );
            require!(
                transfer.target_chain == CHAIN_ID,
                "Wrong chain"
            );

            // Check if already completed
            require!(
                !self.completed_transfers.contains(&transfer_id),
                "Already completed"
            );

            // Determine if we need to mint wrapped tokens or release locked tokens
            let key = (transfer.source_chain, transfer.asset);
            if let Some(wrapped) = self.wrapped_assets.get(&key) {
                // Mint wrapped tokens
                mint_wrapped_tokens(wrapped, transfer.receiver, transfer.amount);
            } else {
                // Release locked tokens
                let locked_key = (transfer.source_chain, transfer.asset);
                let current = self.locked_assets.get(&locked_key).unwrap_or(0);
                require!(current >= transfer.amount, "Insufficient locked assets");
                self.locked_assets.insert(locked_key, current - transfer.amount);

                transfer(transfer.asset, transfer.receiver, transfer.amount);
            }

            transfer.status = TransferStatus::Executed;
            self.completed_transfers.insert(transfer_id.clone());
            self.pending_transfers.remove(&transfer_id);

            emit!(TransferExecuted {
                transfer_id,
                target_chain: CHAIN_ID,
                receiver: transfer.receiver,
                amount: transfer.amount,
            });
        }

        /// Update merkle root (light client)
        #[public]
        pub fn update_merkle_root(&mut self, chain_id: u64, new_root: Hash, zk_proof: Vec<u8>) {
            require!(
                verify_merkle_update_proof(&zk_proof, chain_id, &new_root),
                "Invalid merkle proof"
            );

            self.merkle_roots.insert(chain_id, new_root);
        }

        /// Get wrapped asset address
        #[view]
        pub fn get_wrapped_asset(&self, chain_id: u64, remote_asset: Address) -> Option<Address> {
            self.wrapped_assets.get(&(chain_id, remote_asset)).cloned()
        }

        /// Get locked amount
        #[view]
        pub fn get_locked_amount(&self, chain_id: u64, asset: Address) -> u128 {
            self.locked_assets.get(&(chain_id, asset)).unwrap_or(0)
        }

        /// Get transfer status
        #[view]
        pub fn get_transfer_status(&self, transfer_id: Hash) -> Option<TransferStatus> {
            self.pending_transfers.get(&transfer_id).map(|t| t.status.clone())
        }

        /// Get validator count
        #[view]
        pub fn get_validator_count(&self) -> usize {
            self.validators.len()
        }

        /// Generate unique transfer ID
        fn generate_transfer_id(
            &self,
            target_chain: u64,
            sender: Address,
            receiver: Address,
            asset: Address,
            amount: u128,
        ) -> Hash {
            let mut data = Vec::new();
            data.extend_from_slice(&target_chain.to_le_bytes());
            data.extend_from_slice(sender.as_bytes());
            data.extend_from_slice(receiver.as_bytes());
            data.extend_from_slice(asset.as_bytes());
            data.extend_from_slice(&amount.to_le_bytes());
            data.extend_from_slice(&current_timestamp().to_le_bytes());
            data.extend_from_slice(&random_bytes(32));

            Hash::new(&data)
        }
    }
}
