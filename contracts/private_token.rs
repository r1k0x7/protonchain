// Private Token Contract for Proton Chain
// Implements confidential transfers using ZK proofs and stealth addresses

use proton_sdk::*;

#[contract]
mod private_token {
    use proton_sdk::prelude::*;

    /// Contract state
    #[state]
    pub struct PrivateToken {
        name: String,
        symbol: String,
        decimals: u8,
        total_supply: u128,
        // Merkle tree root for private balances
        merkle_root: Hash,
        // Nullifier set to prevent double spending
        nullifiers: Set<Hash>,
        // View key for decryption
        view_key: Vec<u8>,
    }

    #[event]
    pub struct PrivateTransfer {
        #[indexed]
        nullifier: Hash,
        #[indexed]
        commitment: Hash,
        encrypted_amount: Vec<u8>,
    }

    #[event]
    pub struct PublicMint {
        #[indexed]
        to: Address,
        amount: u128,
    }

    impl PrivateToken {
        #[constructor]
        pub fn new(name: String, symbol: String, decimals: u8) -> Self {
            Self {
                name,
                symbol,
                decimals,
                total_supply: 0,
                merkle_root: Hash::zero(),
                nullifiers: Set::new(),
                view_key: vec![],
            }
        }

        /// Public mint - visible to all
        #[public]
        pub fn mint(&mut self, to: Address, amount: u128) {
            require!(caller_is_owner(), "Only owner can mint");

            self.total_supply += amount;

            // Update public balance
            let mut balance = self.get_public_balance(to);
            balance += amount;
            storage_set(&balance_key(to), &balance.to_le_bytes());

            emit!(PublicMint { to, amount });
        }

        /// Private transfer - hides amount, sender, receiver
        #[private]
        pub fn private_transfer(
            &mut self,
            proof: ZKProof,
            nullifier: Hash,
            commitment: Hash,
            root: Hash,
            encrypted_amount: Vec<u8>,
            stealth_address: StealthAddress,
        ) {
            // Verify ZK proof
            require!(
                verify_zk_proof(&proof, &nullifier, &commitment, &root),
                "Invalid ZK proof"
            );

            // Check nullifier not spent
            require!(
                !self.nullifiers.contains(&nullifier),
                "Nullifier already spent"
            );

            // Verify merkle root
            require!(root == self.merkle_root, "Invalid merkle root");

            // Mark nullifier as spent
            self.nullifiers.insert(nullifier);

            // Update merkle root with new commitment
            self.merkle_root = update_merkle_root(&self.merkle_root, &commitment);

            emit!(PrivateTransfer {
                nullifier,
                commitment,
                encrypted_amount,
            });
        }

        /// Get public balance (for non-private accounts)
        #[view]
        pub fn get_public_balance(&self, account: Address) -> u128 {
            let key = balance_key(account);
            let data = storage_get(&key).unwrap_or_default();
            if data.len() >= 16 {
                u128::from_le_bytes(data[..16].try_into().unwrap())
            } else {
                0
            }
        }

        /// Get total supply
        #[view]
        pub fn get_total_supply(&self) -> u128 {
            self.total_supply
        }

        /// Get token name
        #[view]
        pub fn get_name(&self) -> String {
            self.name.clone()
        }

        /// Get token symbol
        #[view]
        pub fn get_symbol(&self) -> String {
            self.symbol.clone()
        }

        fn balance_key(address: Address) -> Vec<u8> {
            let mut key = b"bal:".to_vec();
            key.extend_from_slice(address.as_bytes());
            key
        }
    }
}
