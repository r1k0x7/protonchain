// Proton Staking - Proof of Stake with Privacy

use proton_sdk::*;

#[contract]
mod proton_staking {
    use proton_sdk::prelude::*;

    #[state]
    pub struct ProtonStaking {
        validators: Map<Address, ValidatorInfo>,
        delegations: Map<(Address, Address), u128>, // (delegator, validator) -> amount
        total_staked: u128,
        min_stake: u128,
        commission_base: u16, // Basis points
        epoch: u64,
        unbonding_period: u64, // Blocks
        unbonding_queue: Vec<UnbondingEntry>,
        // Privacy: encrypted delegation amounts
        encrypted_delegations: Map<Hash, Vec<u8>>,
    }

    #[derive(Clone, Debug)]
    pub struct ValidatorInfo {
        owner: Address,
        stake: u128,
        commission: u16,
        is_active: bool,
        total_delegated: u128,
        uptime: u64,
        blocks_proposed: u64,
        // Privacy
        encrypted_rewards: Vec<u8>,
    }

    #[derive(Clone, Debug)]
    pub struct UnbondingEntry {
        delegator: Address,
        validator: Address,
        amount: u128,
        unlock_height: u64,
    }

    #[event]
    pub struct Staked {
        #[indexed]
        delegator: Address,
        #[indexed]
        validator: Address,
        amount: u128,
    }

    #[event]
    pub struct Unstaked {
        #[indexed]
        delegator: Address,
        #[indexed]
        validator: Address,
        amount: u128,
        unlock_height: u64,
    }

    #[event]
    pub struct ValidatorRegistered {
        #[indexed]
        validator: Address,
        commission: u16,
    }

    #[event]
    pub struct RewardsDistributed {
        #[indexed]
        epoch: u64,
        total_rewards: u128,
    }

    impl ProtonStaking {
        #[constructor]
        pub fn new(min_stake: u128, commission_base: u16, unbonding_period: u64) -> Self {
            Self {
                validators: Map::new(),
                delegations: Map::new(),
                total_staked: 0,
                min_stake,
                commission_base,
                epoch: 0,
                unbonding_period,
                unbonding_queue: Vec::new(),
                encrypted_delegations: Map::new(),
            }
        }

        /// Register as validator
        #[public]
        pub fn register_validator(&mut self, commission: u16) {
            require!(
                commission <= 10000,
                "Commission must be <= 100%"
            );

            let validator = caller();

            require!(
                !self.validators.contains_key(&validator),
                "Already registered"
            );

            let info = ValidatorInfo {
                owner: validator,
                stake: 0,
                commission: commission.max(self.commission_base),
                is_active: false,
                total_delegated: 0,
                uptime: 0,
                blocks_proposed: 0,
                encrypted_rewards: vec![],
            };

            self.validators.insert(validator, info);

            emit!(ValidatorRegistered {
                validator,
                commission,
            });
        }

        /// Stake tokens (public)
        #[public]
        pub fn stake(&mut self, validator: Address, amount: u128) {
            require!(amount >= self.min_stake, "Amount below minimum stake");

            let delegator = caller();
            let mut validator_info = self.validators.get(&validator)
                .expect("Validator not found");

            // Transfer tokens to staking contract
            transfer_from(PROTON_TOKEN, delegator, self_address(), amount);

            // Update delegation
            let key = (delegator, validator);
            let current = self.delegations.get(&key).unwrap_or(0);
            self.delegations.insert(key, current + amount);

            // Update validator
            validator_info.stake += amount;
            validator_info.total_delegated += amount;
            if validator_info.stake >= self.min_stake {
                validator_info.is_active = true;
            }

            self.validators.insert(validator, validator_info);
            self.total_staked += amount;

            emit!(Staked {
                delegator,
                validator,
                amount,
            });
        }

        /// Private stake - hides delegation amount
        #[private]
        pub fn private_stake(
            &mut self,
            validator: Address,
            encrypted_amount: Vec<u8>,
            zk_proof: Vec<u8>,
            commitment: Hash,
            nullifier: Hash,
        ) {
            require!(
                verify_stake_proof(&zk_proof, &validator, &commitment, &nullifier),
                "Invalid stake proof"
            );

            let delegator = caller();
            let key = (delegator, validator);

            // Store encrypted delegation
            self.encrypted_delegations.insert(commitment, encrypted_amount);

            // Update validator (encrypted)
            let mut validator_info = self.validators.get(&validator)
                .expect("Validator not found");
            validator_info.is_active = true;
            self.validators.insert(validator, validator_info);

            emit!(Staked {
                delegator,
                validator,
                amount: 0, // Hidden
            });
        }

        /// Unstake tokens
        #[public]
        pub fn unstake(&mut self, validator: Address, amount: u128) {
            let delegator = caller();
            let key = (delegator, validator);

            let current = self.delegations.get(&key).unwrap_or(0);
            require!(current >= amount, "Insufficient stake");

            // Update delegation
            if current == amount {
                self.delegations.remove(&key);
            } else {
                self.delegations.insert(key, current - amount);
            }

            // Update validator
            let mut validator_info = self.validators.get(&validator).unwrap();
            validator_info.stake -= amount;
            validator_info.total_delegated -= amount;

            if validator_info.stake < self.min_stake {
                validator_info.is_active = false;
            }

            self.validators.insert(validator, validator_info);
            self.total_staked -= amount;

            // Add to unbonding queue
            let unlock_height = current_block_height() + self.unbonding_period;
            self.unbonding_queue.push(UnbondingEntry {
                delegator,
                validator,
                amount,
                unlock_height,
            });

            emit!(Unstaked {
                delegator,
                validator,
                amount,
                unlock_height,
            });
        }

        /// Process unbonding (called at end of each block)
        #[public]
        pub fn process_unbonding(&mut self) {
            let current_height = current_block_height();
            let mut to_remove = Vec::new();

            for (i, entry) in self.unbonding_queue.iter().enumerate() {
                if entry.unlock_height <= current_height {
                    // Return tokens to delegator
                    transfer(PROTON_TOKEN, entry.delegator, entry.amount);
                    to_remove.push(i);
                }
            }

            // Remove processed entries (in reverse order)
            for i in to_remove.into_iter().rev() {
                self.unbonding_queue.remove(i);
            }
        }

        /// Distribute rewards (called by protocol)
        #[public]
        pub fn distribute_rewards(&mut self, rewards: Map<Address, u128>) {
            require!(is_protocol_address(caller()), "Only protocol can distribute");

            let mut total = 0u128;

            for (validator, reward) in rewards.iter() {
                if let Some(mut info) = self.validators.get(validator) {
                    // Calculate validator commission
                    let commission = (reward * info.commission as u128) / 10000;
                    let delegator_reward = reward - commission;

                    // Add to validator stake (auto-compound)
                    info.stake += commission;
                    total += reward;

                    self.validators.insert(*validator, info);

                    // Distribute to delegators (simplified)
                    // In real implementation, would iterate all delegators
                }
            }

            self.epoch += 1;

            emit!(RewardsDistributed {
                epoch: self.epoch,
                total_rewards: total,
            });
        }

        /// Get validator info
        #[view]
        pub fn get_validator(&self, validator: Address) -> Option<ValidatorInfo> {
            self.validators.get(&validator)
        }

        /// Get delegation amount
        #[view]
        pub fn get_delegation(&self, delegator: Address, validator: Address) -> u128 {
            self.delegations.get(&(delegator, validator)).unwrap_or(0)
        }

        /// Get total staked
        #[view]
        pub fn get_total_staked(&self) -> u128 {
            self.total_staked
        }

        /// Get active validators
        #[view]
        pub fn get_active_validators(&self) -> Vec<Address> {
            self.validators
                .iter()
                .filter(|(_, info)| info.is_active)
                .map(|(addr, _)| *addr)
                .collect()
        }

        /// Get validator set for consensus (sorted by stake)
        #[view]
        pub fn get_validator_set(&self, count: usize) -> Vec<(Address, u128)> {
            let mut validators: Vec<_> = self.validators
                .iter()
                .filter(|(_, info)| info.is_active)
                .map(|(addr, info)| (*addr, info.stake))
                .collect();

            validators.sort_by(|a, b| b.1.cmp(&a.1));
            validators.into_iter().take(count).collect()
        }
    }
}
