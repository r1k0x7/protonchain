// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

/**
 * @title ProtonStaking
 * @dev Proof of Stake with private delegation support
 */
contract ProtonStaking is ReentrancyGuard, Ownable {

    IERC20 public protonToken;

    // Validator info
    struct Validator {
        address owner;
        uint256 stake;
        uint256 commission; // Basis points (e.g., 500 = 5%)
        bool isActive;
        uint256 totalDelegated;
        uint256 uptime;
        uint256 blocksProposed;
        uint256 rewardsAccumulated;
        bytes32 encryptedRewards;
    }

    mapping(address => Validator) public validators;
    mapping(address => bool) public isValidator;
    address[] public validatorList;

    // Delegations: delegator => validator => amount
    mapping(address => mapping(address => uint256)) public delegations;
    mapping(address => uint256) public totalDelegatedTo;

    // Private delegations (encrypted)
    mapping(address => mapping(address => bytes32)) public encryptedDelegations;
    mapping(bytes32 => bool) public privateDelegationNullifiers;

    // Unbonding
    struct UnbondingEntry {
        address delegator;
        address validator;
        uint256 amount;
        uint256 unlockHeight;
        bool processed;
    }

    UnbondingEntry[] public unbondingQueue;

    // Parameters
    uint256 public minStake = 10000e18; // 10,000 PROTON
    uint256 public commissionBase = 500; // 5% base
    uint256 public unbondingPeriod = 7 days; // 7 days in blocks (approx)
    uint256 public epoch = 0;

    // ZK verifier
    address public zkVerifier;

    // Events
    event ValidatorRegistered(address indexed validator, uint256 commission);
    event Staked(address indexed delegator, address indexed validator, uint256 amount);
    event PrivateStaked(
        address indexed delegator,
        address indexed validator,
        bytes32 commitment
    );
    event Unstaked(
        address indexed delegator,
        address indexed validator,
        uint256 amount,
        uint256 unlockHeight
    );
    event RewardsDistributed(uint256 indexed epoch, uint256 totalRewards);
    event UnbondingProcessed(address indexed delegator, uint256 amount);
    event ValidatorSlashed(address indexed validator, uint256 amount);

    // Errors
    error InsufficientStake();
    error ValidatorNotFound();
    error ValidatorAlreadyExists();
    error CommissionTooHigh();
    error UnbondingNotReady();
    error InvalidDelegation();
    error InvalidZKProof();

    constructor(address _protonToken, address _zkVerifier) Ownable(msg.sender) {
        protonToken = IERC20(_protonToken);
        zkVerifier = _zkVerifier;
    }

    /**
     * @dev Register as validator
     */
    function registerValidator(uint256 _commission) external nonReentrant {
        if (_commission > 10000) revert CommissionTooHigh();
        if (isValidator[msg.sender]) revert ValidatorAlreadyExists();

        validators[msg.sender] = Validator({
            owner: msg.sender,
            stake: 0,
            commission: _commission > commissionBase ? _commission : commissionBase,
            isActive: false,
            totalDelegated: 0,
            uptime: 0,
            blocksProposed: 0,
            rewardsAccumulated: 0,
            encryptedRewards: bytes32(0)
        });

        isValidator[msg.sender] = true;
        validatorList.push(msg.sender);

        emit ValidatorRegistered(msg.sender, _commission);
    }

    /**
     * @dev Stake tokens (public)
     */
    function stake(address _validator, uint256 _amount) external nonReentrant {
        if (!isValidator[_validator]) revert ValidatorNotFound();
        if (_amount < minStake && validators[_validator].stake == 0) revert InsufficientStake();

        Validator storage validator = validators[_validator];

        // Transfer tokens
        require(
            protonToken.transferFrom(msg.sender, address(this), _amount),
            "Transfer failed"
        );

        // Update delegation
        delegations[msg.sender][_validator] += _amount;
        totalDelegatedTo[_validator] += _amount;

        // Update validator
        validator.stake += _amount;
        validator.totalDelegated += _amount;
        if (validator.stake >= minStake) {
            validator.isActive = true;
        }

        emit Staked(msg.sender, _validator, _amount);
    }

    /**
     * @dev Private stake (encrypted amount)
     */
    function privateStake(
        address _validator,
        bytes32 _encryptedAmount,
        bytes32 _commitment,
        bytes32 _nullifier,
        bytes calldata _zkProof
    ) external nonReentrant {
        if (!isValidator[_validator]) revert ValidatorNotFound();
        if (privateDelegationNullifiers[_nullifier]) revert InvalidDelegation();

        // Verify ZK proof
        if (!verifyStakeProof(_validator, _commitment, _nullifier, _zkProof)) {
            revert InvalidZKProof();
        }

        // Mark nullifier
        privateDelegationNullifiers[_nullifier] = true;

        // Store encrypted delegation
        encryptedDelegations[msg.sender][_validator] = _encryptedAmount;

        // Update validator (activate without revealing amount)
        Validator storage validator = validators[_validator];
        validator.isActive = true;
        validator.totalDelegated += 1; // Increment count, not amount

        emit PrivateStaked(msg.sender, _validator, _commitment);
    }

    /**
     * @dev Unstake tokens
     */
    function unstake(address _validator, uint256 _amount) external nonReentrant {
        if (!isValidator[_validator]) revert ValidatorNotFound();
        if (delegations[msg.sender][_validator] < _amount) revert InvalidDelegation();

        Validator storage validator = validators[_validator];

        // Update delegation
        delegations[msg.sender][_validator] -= _amount;
        totalDelegatedTo[_validator] -= _amount;

        // Update validator
        validator.stake -= _amount;
        validator.totalDelegated -= _amount;

        if (validator.stake < minStake) {
            validator.isActive = false;
        }

        // Add to unbonding queue
        uint256 unlockHeight = block.timestamp + unbondingPeriod;
        unbondingQueue.push(UnbondingEntry({
            delegator: msg.sender,
            validator: _validator,
            amount: _amount,
            unlockHeight: unlockHeight,
            processed: false
        }));

        emit Unstaked(msg.sender, _validator, _amount, unlockHeight);
    }

    /**
     * @dev Process unbonding (can be called by anyone)
     */
    function processUnbonding() external nonReentrant {
        uint256 processed = 0;

        for (uint i = 0; i < unbondingQueue.length; i++) {
            UnbondingEntry storage entry = unbondingQueue[i];

            if (!entry.processed && block.timestamp >= entry.unlockHeight) {
                // Return tokens
                require(
                    protonToken.transfer(entry.delegator, entry.amount),
                    "Transfer failed"
                );

                entry.processed = true;
                processed++;

                emit UnbondingProcessed(entry.delegator, entry.amount);
            }
        }
    }

    /**
     * @dev Distribute rewards (called by protocol/governance)
     */
    function distributeRewards(
        address[] calldata _validators,
        uint256[] calldata _rewards
    ) external onlyOwner {
        require(_validators.length == _rewards.length, "Length mismatch");

        uint256 totalRewards = 0;

        for (uint i = 0; i < _validators.length; i++) {
            Validator storage validator = validators[_validators[i]];
            if (!validator.isActive) continue;

            uint256 commission = (_rewards[i] * validator.commission) / FEE_DENOMINATOR;
            uint256 delegatorReward = _rewards[i] - commission;

            // Add commission to validator stake (auto-compound)
            validator.stake += commission;
            validator.rewardsAccumulated += commission;

            // In production, would distribute delegatorReward to all delegators
            // Simplified: add to total rewards for later claim

            totalRewards += _rewards[i];
        }

        epoch++;
        emit RewardsDistributed(epoch, totalRewards);
    }

    /**
     * @dev Claim rewards
     */
    function claimRewards(address _validator) external nonReentrant {
        uint256 reward = calculateRewards(msg.sender, _validator);
        if (reward == 0) return;

        require(protonToken.transfer(msg.sender, reward), "Transfer failed");
    }

    /**
     * @dev Slash validator (governance/emergency)
     */
    function slashValidator(address _validator, uint256 _percentage) external onlyOwner {
        if (!isValidator[_validator]) revert ValidatorNotFound();
        require(_percentage <= 10000, "Invalid percentage");

        Validator storage validator = validators[_validator];
        uint256 slashAmount = (validator.stake * _percentage) / FEE_DENOMINATOR;

        validator.stake -= slashAmount;
        validator.isActive = false;

        // Burn or redistribute slashed amount
        // In production, would send to treasury or redistribute

        emit ValidatorSlashed(_validator, slashAmount);
    }

    /**
     * @dev Get delegation amount
     */
    function getDelegation(address _delegator, address _validator) 
        external 
        view 
        returns (uint256) 
    {
        return delegations[_delegator][_validator];
    }

    /**
     * @dev Get total staked amount
     */
    function getTotalStaked() external view returns (uint256) {
        uint256 total = 0;
        for (uint i = 0; i < validatorList.length; i++) {
            total += validators[validatorList[i]].stake;
        }
        return total;
    }

    /**
     * @dev Get active validators
     */
    function getActiveValidators() external view returns (address[] memory) {
        uint256 activeCount = 0;
        for (uint i = 0; i < validatorList.length; i++) {
            if (validators[validatorList[i]].isActive) activeCount++;
        }

        address[] memory active = new address[](activeCount);
        uint256 idx = 0;
        for (uint i = 0; i < validatorList.length; i++) {
            if (validators[validatorList[i]].isActive) {
                active[idx] = validatorList[i];
                idx++;
            }
        }
        return active;
    }

    /**
     * @dev Get validator set sorted by stake (for consensus)
     */
    function getValidatorSet(uint256 _count) 
        external 
        view 
        returns (address[] memory, uint256[] memory) 
    {
        address[] memory addrs = new address[](validatorList.length);
        uint256[] memory stakes = new uint256[](validatorList.length);

        for (uint i = 0; i < validatorList.length; i++) {
            addrs[i] = validatorList[i];
            stakes[i] = validators[validatorList[i]].stake;
        }

        // Simple bubble sort (use library in production)
        for (uint i = 0; i < stakes.length; i++) {
            for (uint j = i + 1; j < stakes.length; j++) {
                if (stakes[j] > stakes[i]) {
                    (stakes[i], stakes[j]) = (stakes[j], stakes[i]);
                    (addrs[i], addrs[j]) = (addrs[j], addrs[i]);
                }
            }
        }

        uint256 count = _count < addrs.length ? _count : addrs.length;
        address[] memory resultAddrs = new address[](count);
        uint256[] memory resultStakes = new uint256[](count);

        for (uint i = 0; i < count; i++) {
            resultAddrs[i] = addrs[i];
            resultStakes[i] = stakes[i];
        }

        return (resultAddrs, resultStakes);
    }

    /**
     * @dev Calculate rewards for a delegator
     */
    function calculateRewards(address _delegator, address _validator) 
        public 
        view 
        returns (uint256) 
    {
        // Simplified calculation
        // In production, would track reward per share
        uint256 delegation = delegations[_delegator][_validator];
        if (delegation == 0) return 0;

        Validator storage validator = validators[_validator];
        uint256 share = (delegation * 1e18) / validator.totalDelegated;

        return (share * validator.rewardsAccumulated) / 1e18;
    }

    function verifyStakeProof(
        address _validator,
        bytes32 _commitment,
        bytes32 _nullifier,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyStake.selector,
                _validator,
                _commitment,
                _nullifier,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    // Update parameters
    function setMinStake(uint256 _newMinStake) external onlyOwner {
        minStake = _newMinStake;
    }

    function setUnbondingPeriod(uint256 _newPeriod) external onlyOwner {
        unbondingPeriod = _newPeriod;
    }

    function setZKVerifier(address _newVerifier) external onlyOwner {
        zkVerifier = _newVerifier;
    }

    uint256 public constant FEE_DENOMINATOR = 10000;
}

interface IZKVerifier {
    function verifyStake(
        address validator,
        bytes32 commitment,
        bytes32 nullifier,
        bytes calldata proof
    ) external view returns (bool);
}
