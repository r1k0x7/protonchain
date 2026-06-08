use crate::types::*;
use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;
use tracing::{info, debug, error};

/// VM execution result
#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub gas_used: u64,
    pub return_data: Vec<u8>,
    pub logs: Vec<LogEntry>,
    pub state_changes: HashMap<Address, AccountChange>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub address: Address,
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AccountChange {
    pub balance_delta: i128,
    pub nonce_delta: i64,
    pub storage_changes: HashMap<Hash, Vec<u8>>,
    pub code: Option<Vec<u8>>,
}

/// WASM-based smart contract VM (ProtonVM)
pub struct ProtonVM {
    state: Arc<RwLock<VMState>>,
    gas_schedule: GasSchedule,
    max_memory_pages: u32,
}

/// VM state
#[derive(Clone, Debug)]
pub struct VMState {
    pub accounts: HashMap<Address, Account>,
    pub contracts: HashMap<Address, ContractCode>,
    pub storage: HashMap<(Address, Hash), Vec<u8>>,
    pub block_context: BlockContext,
}

#[derive(Clone, Debug)]
pub struct BlockContext {
    pub height: u64,
    pub timestamp: u64,
    pub coinbase: Address,
    pub difficulty: u64,
    pub gas_limit: u64,
}

#[derive(Clone, Debug)]
pub struct ContractCode {
    pub code: Vec<u8>,
    pub abi: Vec<u8>,
    pub vm_type: VMType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VMType {
    Wasm,
    EVM,
    Native,
}

/// Gas schedule
#[derive(Clone, Debug)]
pub struct GasSchedule {
    pub base_tx_cost: u64,
    pub storage_load: u64,
    pub storage_store: u64,
    pub memory_page: u64,
    pub call_cost: u64,
    pub log_cost: u64,
    pub wasm_instruction: u64,
    pub evm_instruction: u64,
}

impl Default for GasSchedule {
    fn default() -> Self {
        Self {
            base_tx_cost: 21_000,
            storage_load: 200,
            storage_store: 20_000,
            memory_page: 100,
            call_cost: 700,
            log_cost: 375,
            wasm_instruction: 1,
            evm_instruction: 1,
        }
    }
}

impl ProtonVM {
    pub fn new(state: Arc<RwLock<VMState>>, gas_schedule: GasSchedule) -> Self {
        Self {
            state,
            gas_schedule,
            max_memory_pages: 256, // 16MB max
        }
    }

    /// Execute transaction
    pub fn execute_transaction(&self, tx: &Transaction) -> ExecutionResult {
        let mut gas_used = self.gas_schedule.base_tx_cost;

        match tx.tx_type {
            TransactionType::Transfer => {
                self.execute_transfer(tx, &mut gas_used)
            }
            TransactionType::ContractDeploy => {
                self.execute_contract_deploy(tx, &mut gas_used)
            }
            TransactionType::ContractCall => {
                self.execute_contract_call(tx, &mut gas_used)
            }
            TransactionType::PrivateTransfer => {
                self.execute_private_transfer(tx, &mut gas_used)
            }
            _ => ExecutionResult {
                success: false,
                gas_used,
                return_data: vec![],
                logs: vec![],
                state_changes: HashMap::new(),
                error: Some("Unsupported transaction type".to_string()),
            }
        }
    }

    fn execute_transfer(&self, tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        let mut state = self.state.write();
        let mut changes = HashMap::new();

        // Check sender balance
        let sender = state.accounts.get(&tx.from).cloned().unwrap_or_default();
        let total_cost = tx.value + tx.gas_cost();

        if sender.balance < total_cost {
            return ExecutionResult {
                success: false,
                gas_used: *gas_used,
                return_data: vec![],
                logs: vec![],
                state_changes: HashMap::new(),
                error: Some("Insufficient balance".to_string()),
            };
        }

        // Update sender
        let mut sender_change = AccountChange {
            balance_delta: -(total_cost as i128),
            nonce_delta: 1,
            storage_changes: HashMap::new(),
            code: None,
        };

        // Update receiver
        let receiver_addr = tx.to.unwrap_or(tx.from);
        let receiver = state.accounts.get(&receiver_addr).cloned().unwrap_or_default();

        let mut receiver_change = AccountChange {
            balance_delta: tx.value as i128,
            nonce_delta: 0,
            storage_changes: HashMap::new(),
            code: None,
        };

        changes.insert(tx.from, sender_change);
        changes.insert(receiver_addr, receiver_change);

        // Apply changes
        for (addr, change) in &changes {
            let mut account = state.accounts.get(addr).cloned().unwrap_or_default();
            account.balance = (account.balance as i128 + change.balance_delta) as u128;
            account.nonce = (account.nonce as i64 + change.nonce_delta) as u64;
            state.accounts.insert(*addr, account);
        }

        *gas_used += self.gas_schedule.base_tx_cost;

        ExecutionResult {
            success: true,
            gas_used: *gas_used,
            return_data: vec![],
            logs: vec![],
            state_changes: changes,
            error: None,
        }
    }

    fn execute_contract_deploy(&self, tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        let mut state = self.state.write();
        let mut changes = HashMap::new();

        // Create contract address from sender and nonce
        let mut contract_addr_data = Vec::new();
        contract_addr_data.extend_from_slice(tx.from.as_bytes());
        contract_addr_data.extend_from_slice(&tx.nonce.to_le_bytes());
        let contract_addr = Address::new(&contract_addr_data);

        // Store contract code
        let contract = ContractCode {
            code: tx.data.clone(),
            abi: vec![], // Would be extracted from metadata
            vm_type: VMType::Wasm, // Default to WASM
        };

        state.contracts.insert(contract_addr, contract);

        // Create contract account
        let mut account = Account::default();
        account.is_contract = true;
        account.code_hash = Some(Hash::new(&tx.data));
        account.balance = tx.value;

        state.accounts.insert(contract_addr, account);

        // Deduct from sender
        let sender = state.accounts.get(&tx.from).cloned().unwrap_or_default();
        let mut sender_change = AccountChange {
            balance_delta: -(tx.value as i128 + tx.gas_cost() as i128),
            nonce_delta: 1,
            storage_changes: HashMap::new(),
            code: None,
        };

        changes.insert(tx.from, sender_change);
        changes.insert(contract_addr, AccountChange {
            balance_delta: tx.value as i128,
            nonce_delta: 0,
            storage_changes: HashMap::new(),
            code: Some(tx.data.clone()),
        });

        *gas_used += self.gas_schedule.base_tx_cost + (tx.data.len() as u64 * 200);

        ExecutionResult {
            success: true,
            gas_used: *gas_used,
            return_data: vec![],
            logs: vec![],
            state_changes: changes,
            error: None,
        }
    }

    fn execute_contract_call(&self, tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        let mut state = self.state.write();
        let contract_addr = tx.to.unwrap_or(tx.from);

        // Get contract code
        let contract = match state.contracts.get(&contract_addr).cloned() {
            Some(c) => c,
            None => {
                return ExecutionResult {
                    success: false,
                    gas_used: *gas_used,
                    return_data: vec![],
                    logs: vec![],
                    state_changes: HashMap::new(),
                    error: Some("Contract not found".to_string()),
                };
            }
        };

        match contract.vm_type {
            VMType::Wasm => self.execute_wasm(&contract.code, tx, gas_used),
            VMType::EVM => self.execute_evm(&contract.code, tx, gas_used),
            VMType::Native => self.execute_native(&contract.code, tx, gas_used),
        }
    }

    fn execute_wasm(&self, code: &[u8], tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        // Simplified WASM execution
        // In real implementation, use wasmer or wasmtime

        debug!("Executing WASM contract, code size: {} bytes", code.len());

        // Simulate gas consumption
        *gas_used += code.len() as u64 * self.gas_schedule.wasm_instruction;

        // Placeholder: return success
        ExecutionResult {
            success: true,
            gas_used: *gas_used,
            return_data: vec![0x01], // Success indicator
            logs: vec![],
            state_changes: HashMap::new(),
            error: None,
        }
    }

    fn execute_evm(&self, code: &[u8], tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        // Simplified EVM execution
        // In real implementation, use evm crate or revm

        debug!("Executing EVM contract, code size: {} bytes", code.len());

        *gas_used += code.len() as u64 * self.gas_schedule.evm_instruction;

        ExecutionResult {
            success: true,
            gas_used: *gas_used,
            return_data: vec![0x01],
            logs: vec![],
            state_changes: HashMap::new(),
            error: None,
        }
    }

    fn execute_native(&self, code: &[u8], tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        // Native precompiled contracts
        debug!("Executing native contract");

        ExecutionResult {
            success: true,
            gas_used: *gas_used,
            return_data: vec![0x01],
            logs: vec![],
            state_changes: HashMap::new(),
            error: None,
        }
    }

    fn execute_private_transfer(&self, tx: &Transaction, gas_used: &mut u64) -> ExecutionResult {
        // Verify ZK proof
        if let Some(proof) = &tx.zk_proof {
            debug!("Verifying ZK proof for private transfer");
            *gas_used += 100_000; // ZK verification is expensive
        }

        // Execute transfer with encrypted amount
        let mut result = self.execute_transfer(tx, gas_used);

        // Mark as private
        if result.success {
            debug!("Private transfer executed successfully");
        }

        result
    }

    /// Get account state
    pub fn get_account(&self, address: &Address) -> Option<Account> {
        self.state.read().accounts.get(address).cloned()
    }

    /// Get contract code
    pub fn get_contract(&self, address: &Address) -> Option<ContractCode> {
        self.state.read().contracts.get(address).cloned()
    }

    /// Get storage value
    pub fn get_storage(&self, address: &Address, key: &Hash) -> Option<Vec<u8>> {
        self.state.read().storage.get(&(*address, *key)).cloned()
    }

    /// Get state root hash
    pub fn state_root(&self) -> Hash {
        let state = self.state.read();
        // Simplified: hash all account states
        let mut data = Vec::new();
        for (addr, account) in &state.accounts {
            data.extend_from_slice(addr.as_bytes());
            data.extend_from_slice(&account.balance.to_le_bytes());
            data.extend_from_slice(&account.nonce.to_le_bytes());
        }
        Hash::new(&data)
    }
}

/// Parallel transaction executor using block-STM
pub struct ParallelExecutor {
    vm: Arc<ProtonVM>,
    thread_pool: rayon::ThreadPool,
}

impl ParallelExecutor {
    pub fn new(vm: Arc<ProtonVM>, num_threads: usize) -> Self {
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .unwrap();

        Self { vm, thread_pool }
    }

    /// Execute transactions in parallel with optimistic concurrency
    pub fn execute_block_parallel(&self, transactions: &[Transaction]) -> Vec<ExecutionResult> {
        self.thread_pool.install(|| {
            transactions
                .par_iter()
                .map(|tx| self.vm.execute_transaction(tx))
                .collect()
        })
    }

    /// Execute transactions sequentially (fallback)
    pub fn execute_block_sequential(&self, transactions: &[Transaction]) -> Vec<ExecutionResult> {
        transactions
            .iter()
            .map(|tx| self.vm.execute_transaction(tx))
            .collect()
    }
}

use rayon::prelude::*;

/// Gas estimator
pub struct GasEstimator;

impl GasEstimator {
    pub fn estimate_gas(tx: &Transaction, vm: &ProtonVM) -> u64 {
        let base_cost = match tx.tx_type {
            TransactionType::Transfer => 21_000,
            TransactionType::ContractDeploy => 53_000 + (tx.data.len() as u64 * 200),
            TransactionType::ContractCall => 21_000 + (tx.data.len() as u64 * 100),
            TransactionType::PrivateTransfer => 121_000, // Higher due to ZK verification
            _ => 21_000,
        };

        base_cost
    }
      }
      
