// Proton NFT - Private NFT with Stealth Ownership

use proton_sdk::*;

#[contract]
mod proton_nft {
    use proton_sdk::prelude::*;

    #[state]
    pub struct ProtonNFT {
        name: String,
        symbol: String,
        tokens: Map<u256, TokenData>,
        // Public ownership mapping (can be null for private tokens)
        owners: Map<u256, Address>,
        // Private ownership using stealth addresses
        private_owners: Map<u256, StealthAddress>,
        // Encrypted metadata
        encrypted_metadata: Map<u256, Vec<u8>>,
        approvals: Map<u256, Address>,
        operator_approvals: Map<(Address, Address), bool>,
        total_supply: u256,
        // Nullifier set for private transfers
        nullifiers: Set<Hash>,
    }

    #[derive(Clone, Debug)]
    pub struct TokenData {
        token_id: u256,
        public_metadata: String,
        encrypted_metadata_hash: Hash,
        is_private: bool,
        created_at: u64,
    }

    #[event]
    pub struct Transfer {
        #[indexed]
        from: Option<Address>,
        #[indexed]
        to: Option<Address>,
        #[indexed]
        token_id: u256,
    }

    #[event]
    pub struct PrivateTransfer {
        #[indexed]
        token_id: u256,
        #[indexed]
        nullifier: Hash,
        new_stealth: StealthAddress,
    }

    #[event]
    pub struct Minted {
        #[indexed]
        token_id: u256,
        #[indexed]
        owner: Address,
        is_private: bool,
    }

    #[event]
    pub struct Approval {
        #[indexed]
        owner: Address,
        #[indexed]
        approved: Address,
        #[indexed]
        token_id: u256,
    }

    impl ProtonNFT {
        #[constructor]
        pub fn new(name: String, symbol: String) -> Self {
            Self {
                name,
                symbol,
                tokens: Map::new(),
                owners: Map::new(),
                private_owners: Map::new(),
                encrypted_metadata: Map::new(),
                approvals: Map::new(),
                operator_approvals: Map::new(),
                total_supply: 0.into(),
                nullifiers: Set::new(),
            }
        }

        /// Mint public NFT
        #[public]
        pub fn mint(&mut self, to: Address, metadata: String) -> u256 {
            require!(caller_is_owner(), "Only owner can mint");

            let token_id = self.total_supply + 1;
            self.total_supply = token_id;

            let token = TokenData {
                token_id,
                public_metadata: metadata,
                encrypted_metadata_hash: Hash::zero(),
                is_private: false,
                created_at: current_timestamp(),
            };

            self.tokens.insert(token_id, token);
            self.owners.insert(token_id, to);

            emit!(Minted {
                token_id,
                owner: to,
                is_private: false,
            });

            token_id
        }

        /// Mint private NFT with encrypted metadata
        #[private]
        pub fn mint_private(
            &mut self,
            to_stealth: StealthAddress,
            encrypted_metadata: Vec<u8>,
            metadata_hash: Hash,
            zk_proof: Vec<u8>,
        ) -> u256 {
            require!(caller_is_owner(), "Only owner can mint");
            require!(
                verify_metadata_proof(&zk_proof, &metadata_hash, &encrypted_metadata),
                "Invalid metadata proof"
            );

            let token_id = self.total_supply + 1;
            self.total_supply = token_id;

            let token = TokenData {
                token_id,
                public_metadata: "".to_string(),
                encrypted_metadata_hash: metadata_hash,
                is_private: true,
                created_at: current_timestamp(),
            };

            self.tokens.insert(token_id, token);
            self.private_owners.insert(token_id, to_stealth);
            self.encrypted_metadata.insert(token_id, encrypted_metadata);

            emit!(Minted {
                token_id,
                owner: Address::new(&[0u8; 32]), // Hidden
                is_private: true,
            });

            token_id
        }

        /// Public transfer
        #[public]
        pub fn transfer(&mut self, from: Address, to: Address, token_id: u256) {
            require!(self.is_approved_or_owner(from, token_id), "Not approved or owner");

            let token = self.tokens.get(&token_id).expect("Token not found");
            require!(!token.is_private, "Use private_transfer for private tokens");

            self.owners.insert(token_id, to);
            self.approvals.remove(&token_id);

            emit!(Transfer {
                from: Some(from),
                to: Some(to),
                token_id,
            });
        }

        /// Private transfer with stealth address
        #[private]
        pub fn private_transfer(
            &mut self,
            token_id: u256,
            to_stealth: StealthAddress,
            proof: ZKProof,
            nullifier: Hash,
            new_commitment: Hash,
        ) {
            let token = self.tokens.get(&token_id).expect("Token not found");
            require!(token.is_private, "Token is not private");

            // Verify ownership proof
            let current_stealth = self.private_owners.get(&token_id).unwrap();
            require!(
                verify_ownership_proof(&proof, &token_id, &current_stealth, &nullifier),
                "Invalid ownership proof"
            );

            // Check nullifier not spent
            require!(!self.nullifiers.contains(&nullifier), "Token already transferred");
            self.nullifiers.insert(nullifier);

            // Update stealth owner
            self.private_owners.insert(token_id, to_stealth);

            emit!(PrivateTransfer {
                token_id,
                nullifier,
                new_stealth: to_stealth,
            });
        }

        /// Approve operator
        #[public]
        pub fn approve(&mut self, to: Address, token_id: u256) {
            let owner = self.owner_of(token_id).unwrap();
            require!(owner == caller(), "Not owner");

            self.approvals.insert(token_id, to);

            emit!(Approval {
                owner,
                approved: to,
                token_id,
            });
        }

        /// Set approval for all
        #[public]
        pub fn set_approval_for_all(&mut self, operator: Address, approved: bool) {
            let key = (caller(), operator);
            self.operator_approvals.insert(key, approved);
        }

        /// Get owner (public tokens only)
        #[view]
        pub fn owner_of(&self, token_id: u256) -> Option<Address> {
            self.owners.get(&token_id).cloned()
        }

        /// Get token metadata (public)
        #[view]
        pub fn get_metadata(&self, token_id: u256) -> String {
            let token = self.tokens.get(&token_id).expect("Token not found");
            token.public_metadata.clone()
        }

        /// Get encrypted metadata (only for owner with view key)
        #[view]
        pub fn get_encrypted_metadata(&self, token_id: u256) -> Option<Vec<u8>> {
            self.encrypted_metadata.get(&token_id).cloned()
        }

        /// Check if token is private
        #[view]
        pub fn is_private(&self, token_id: u256) -> bool {
            self.tokens.get(&token_id)
                .map(|t| t.is_private)
                .unwrap_or(false)
        }

        /// Get total supply
        #[view]
        pub fn get_total_supply(&self) -> u256 {
            self.total_supply
        }

        /// Check if address is approved or owner
        fn is_approved_or_owner(&self, spender: Address, token_id: u256) -> bool {
            let owner = self.owner_of(token_id);
            if owner == Some(spender) {
                return true;
            }

            if self.approvals.get(&token_id) == Some(&spender) {
                return true;
            }

            let key = (owner.unwrap_or(Address::new(&[0u8; 32])), spender);
            self.operator_approvals.get(&key).unwrap_or(false)
        }
    }
}
