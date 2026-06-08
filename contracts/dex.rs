// Proton DEX - Decentralized Exchange with Privacy
// Supports private order matching and encrypted order books

use proton_sdk::*;

#[contract]
mod proton_dex {
    use proton_sdk::prelude::*;

    #[state]
    pub struct ProtonDEX {
        pairs: Map<(Address, Address), TradingPair>,
        fees: u16, // Basis points (e.g., 30 = 0.3%)
        fee_recipient: Address,
        // Encrypted order book
        encrypted_orders: Map<Hash, EncryptedOrder>,
        // Nullifier set for order cancellation
        order_nullifiers: Set<Hash>,
    }

    #[derive(Clone, Debug)]
    pub struct TradingPair {
        token_a: Address,
        token_b: Address,
        reserve_a: u128,
        reserve_b: u128,
        total_liquidity: u128,
        // Encrypted cumulative volume
        encrypted_volume: Vec<u8>,
    }

    #[derive(Clone, Debug)]
    pub struct EncryptedOrder {
        owner: StealthAddress,
        encrypted_amount: Vec<u8>,
        encrypted_price: Vec<u8>,
        is_buy: bool,
        zk_proof: Vec<u8>,
        timestamp: u64,
    }

    #[event]
    pub struct PrivateSwap {
        #[indexed]
        pair: (Address, Address),
        #[indexed]
        nullifier: Hash,
        encrypted_amount_in: Vec<u8>,
        encrypted_amount_out: Vec<u8>,
    }

    #[event]
    pub struct LiquidityAdded {
        #[indexed]
        provider: Address,
        token_a: Address,
        token_b: Address,
        amount_a: u128,
        amount_b: u128,
        liquidity: u128,
    }

    #[event]
    pub struct EncryptedOrderPlaced {
        #[indexed]
        order_hash: Hash,
        #[indexed]
        pair: (Address, Address),
        is_buy: bool,
    }

    impl ProtonDEX {
        #[constructor]
        pub fn new(fees: u16, fee_recipient: Address) -> Self {
            Self {
                pairs: Map::new(),
                fees,
                fee_recipient,
                encrypted_orders: Map::new(),
                order_nullifiers: Set::new(),
            }
        }

        /// Create trading pair
        #[public]
        pub fn create_pair(&mut self, token_a: Address, token_b: Address) {
            require!(
                token_a != token_b,
                "Invalid pair: same token"
            );

            let pair_key = self.sort_pair(token_a, token_b);

            require!(
                !self.pairs.contains_key(&pair_key),
                "Pair already exists"
            );

            let pair = TradingPair {
                token_a: pair_key.0,
                token_b: pair_key.1,
                reserve_a: 0,
                reserve_b: 0,
                total_liquidity: 0,
                encrypted_volume: vec![],
            };

            self.pairs.insert(pair_key, pair);
        }

        /// Add liquidity (public)
        #[public]
        pub fn add_liquidity(
            &mut self,
            token_a: Address,
            token_b: Address,
            amount_a: u128,
            amount_b: u128,
        ) -> u128 {
            let pair_key = self.sort_pair(token_a, token_b);
            let mut pair = self.pairs.get(&pair_key).unwrap();

            // Transfer tokens to contract
            transfer_from(token_a, caller(), self_address(), amount_a);
            transfer_from(token_b, caller(), self_address(), amount_b);

            let liquidity = if pair.total_liquidity == 0 {
                // First liquidity provider
                sqrt(amount_a * amount_b)
            } else {
                let liquidity_a = (amount_a * pair.total_liquidity) / pair.reserve_a;
                let liquidity_b = (amount_b * pair.total_liquidity) / pair.reserve_b;
                min(liquidity_a, liquidity_b)
            };

            pair.reserve_a += amount_a;
            pair.reserve_b += amount_b;
            pair.total_liquidity += liquidity;

            self.pairs.insert(pair_key, pair);

            // Mint LP tokens
            mint_lp_token(caller(), liquidity);

            emit!(LiquidityAdded {
                provider: caller(),
                token_a: pair_key.0,
                token_b: pair_key.1,
                amount_a,
                amount_b,
                liquidity,
            });

            liquidity
        }

        /// Private swap - hides trade details
        #[private]
        pub fn private_swap(
            &mut self,
            token_in: Address,
            token_out: Address,
            proof: ZKProof,
            nullifier: Hash,
            encrypted_amount_in: Vec<u8>,
            encrypted_amount_out: Vec<u8>,
            min_amount_out_commitment: Hash,
            zk_balance_proof: Vec<u8>,
        ) {
            let pair_key = self.sort_pair(token_in, token_out);
            let mut pair = self.pairs.get(&pair_key).unwrap();

            // Verify ZK proof for valid swap
            require!(
                verify_swap_proof(&proof, &pair_key, &nullifier, &encrypted_amount_in),
                "Invalid swap proof"
            );

            // Verify balance proof
            require!(
                verify_balance_proof(&zk_balance_proof, &caller(), &encrypted_amount_in),
                "Insufficient balance"
            );

            // Calculate output using constant product formula (within ZK circuit)
            // amount_out = (amount_in * reserve_out) / (reserve_in + amount_in)

            // Update reserves (encrypted)
            // In real implementation, this would update encrypted reserves

            // Transfer tokens (via stealth addresses)
            private_transfer(token_in, caller(), self_address(), encrypted_amount_in);
            private_transfer(token_out, self_address(), caller(), encrypted_amount_out);

            emit!(PrivateSwap {
                pair: pair_key,
                nullifier,
                encrypted_amount_in,
                encrypted_amount_out,
            });
        }

        /// Place encrypted order (limit order)
        #[private]
        pub fn place_encrypted_order(
            &mut self,
            token_a: Address,
            token_b: Address,
            encrypted_amount: Vec<u8>,
            encrypted_price: Vec<u8>,
            is_buy: bool,
            zk_proof: Vec<u8>,
            stealth: StealthAddress,
        ) -> Hash {
            let order_hash = Hash::new(&[
                &token_a.as_bytes()[..],
                &token_b.as_bytes()[..],
                &encrypted_amount,
                &encrypted_price,
                &[is_buy as u8],
                &current_timestamp().to_le_bytes(),
            ].concat());

            require!(
                verify_order_proof(&zk_proof, &order_hash, &stealth),
                "Invalid order proof"
            );

            let order = EncryptedOrder {
                owner: stealth,
                encrypted_amount,
                encrypted_price,
                is_buy,
                zk_proof,
                timestamp: current_timestamp(),
            };

            self.encrypted_orders.insert(order_hash.clone(), order);

            emit!(EncryptedOrderPlaced {
                order_hash: order_hash.clone(),
                pair: (token_a, token_b),
                is_buy,
            });

            order_hash
        }

        /// Get reserves (public view)
        #[view]
        pub fn get_reserves(&self, token_a: Address, token_b: Address) -> (u128, u128) {
            let pair_key = self.sort_pair(token_a, token_b);
            if let Some(pair) = self.pairs.get(&pair_key) {
                (pair.reserve_a, pair.reserve_b)
            } else {
                (0, 0)
            }
        }

        /// Calculate public swap amount (for non-private swaps)
        #[view]
        pub fn calculate_swap_amount(
            &self,
            token_in: Address,
            token_out: Address,
            amount_in: u128,
        ) -> u128 {
            let pair_key = self.sort_pair(token_in, token_out);
            let pair = self.pairs.get(&pair_key).unwrap();

            let amount_in_with_fee = amount_in * (10000 - self.fees as u128) / 10000;
            let numerator = amount_in_with_fee * pair.reserve_b;
            let denominator = pair.reserve_a + amount_in_with_fee;

            numerator / denominator
        }

        fn sort_pair(&self, a: Address, b: Address) -> (Address, Address) {
            if a.as_bytes() < b.as_bytes() {
                (a, b)
            } else {
                (b, a)
            }
        }
    }

    // Helper functions
    fn sqrt(x: u128) -> u128 {
        if x == 0 {
            return 0;
        }
        let mut z = x;
        let mut y = (z + 1) / 2;
        while y < z {
            z = y;
            y = (z + x / z) / 2;
        }
        z
    }

    fn min(a: u128, b: u128) -> u128 {
        if a < b { a } else { b }
    }
}
