// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";

/**
 * @title ProtonBridge
 * @dev Cross-chain bridge with ZK verification and multi-sig
 */
contract ProtonBridge is Ownable, ReentrancyGuard {

    // Chain configuration
    struct ChainConfig {
        uint256 chainId;
        address bridgeContract; // Remote bridge address
        bool isActive;
        uint256 gasLimit;
        uint256 confirmationBlocks;
    }

    mapping(uint256 => ChainConfig) public supportedChains;
    uint256[] public chainIds;

    // Wrapped assets
    mapping(bytes32 => address) public wrappedAssets; // keccak(chainId, remoteAsset) => localAsset

    // Locked assets
    mapping(bytes32 => uint256) public lockedAssets; // keccak(chainId, asset) => amount

    // Validators (multi-sig)
    struct Validator {
        address addr;
        uint256 stake;
        bool isActive;
        uint256 lastSignature;
    }

    mapping(address => Validator) public validators;
    address[] public validatorList;
    uint256 public validatorThreshold; // Minimum signatures needed
    uint256 public totalValidatorStake;

    // Pending transfers
    struct PendingTransfer {
        bytes32 transferId;
        uint256 sourceChain;
        uint256 targetChain;
        address sender;
        address receiver;
        address asset;
        uint256 amount;
        mapping(address => bool) signatures;
        uint256 signatureCount;
        TransferStatus status;
        uint256 timestamp;
        bytes32 encryptedAmount;
        bytes32 stealthReceiver;
    }

    mapping(bytes32 => PendingTransfer) public pendingTransfers;
    bytes32[] public pendingTransferIds;

    // Completed transfers
    mapping(bytes32 => bool) public completedTransfers;

    // Merkle roots for light client
    mapping(uint256 => bytes32) public merkleRoots;

    // ZK verifier
    address public zkVerifier;

    // Events
    event TransferInitiated(
        bytes32 indexed transferId,
        uint256 sourceChain,
        uint256 targetChain,
        address sender,
        address receiver,
        address asset,
        uint256 amount
    );

    event PrivateTransferInitiated(
        bytes32 indexed transferId,
        uint256 targetChain,
        bytes32 stealthReceiver
    );

    event TransferSigned(bytes32 indexed transferId, address indexed validator);
    event TransferExecuted(bytes32 indexed transferId, uint256 targetChain, address receiver, uint256 amount);
    event ChainAdded(uint256 chainId, address bridgeContract);
    event ValidatorAdded(address validator, uint256 stake);
    event MerkleRootUpdated(uint256 chainId, bytes32 newRoot);

    // Errors
    error ChainNotSupported();
    error ChainAlreadyAdded();
    error ValidatorAlreadyExists();
    error InsufficientSignatures();
    error TransferAlreadyCompleted();
    error TransferNotFound();
    error InvalidMerkleProof();
    error InvalidZKProof();
    error NotValidator();
    error ValidatorNotActive();
    error InsufficientLockedAssets();

    enum TransferStatus {
        Pending,
        SourceConfirmed,
        SignaturesComplete,
        Executed,
        Failed
    }

    constructor(uint256 _validatorThreshold, address _zkVerifier) Ownable(msg.sender) {
        validatorThreshold = _validatorThreshold;
        zkVerifier = _zkVerifier;
    }

    /**
     * @dev Add supported chain
     */
    function addChain(
        uint256 _chainId,
        address _bridgeContract,
        uint256 _confirmationBlocks
    ) external onlyOwner {
        if (supportedChains[_chainId].isActive) revert ChainAlreadyAdded();

        supportedChains[_chainId] = ChainConfig({
            chainId: _chainId,
            bridgeContract: _bridgeContract,
            isActive: true,
            gasLimit: 500000,
            confirmationBlocks: _confirmationBlocks
        });

        chainIds.push(_chainId);
        emit ChainAdded(_chainId, _bridgeContract);
    }

    /**
     * @dev Register wrapped asset
     */
    function registerWrappedAsset(
        uint256 _chainId,
        address _remoteAsset,
        address _localAsset
    ) external onlyOwner {
        bytes32 key = keccak256(abi.encodePacked(_chainId, _remoteAsset));
        wrappedAssets[key] = _localAsset;
    }

    /**
     * @dev Add validator
     */
    function addValidator(address _validator, uint256 _stake) external onlyOwner {
        if (validators[_validator].addr != address(0)) revert ValidatorAlreadyExists();

        validators[_validator] = Validator({
            addr: _validator,
            stake: _stake,
            isActive: true,
            lastSignature: 0
        });

        validatorList.push(_validator);
        totalValidatorStake += _stake;

        emit ValidatorAdded(_validator, _stake);
    }

    /**
     * @dev Initiate cross-chain transfer (lock assets)
     */
    function initiateTransfer(
        uint256 _targetChain,
        address _receiver,
        address _asset,
        uint256 _amount
    ) external nonReentrant returns (bytes32) {
        ChainConfig storage chain = supportedChains[_targetChain];
        if (!chain.isActive) revert ChainNotSupported();

        bytes32 transferId = keccak256(abi.encodePacked(
            _targetChain,
            msg.sender,
            _receiver,
            _asset,
            _amount,
            block.timestamp,
            block.number
        ));

        if (completedTransfers[transferId]) revert TransferAlreadyCompleted();

        // Lock assets
        require(
            IERC20(_asset).transferFrom(msg.sender, address(this), _amount),
            "Lock failed"
        );

        bytes32 lockKey = keccak256(abi.encodePacked(_targetChain, _asset));
        lockedAssets[lockKey] += _amount;

        // Create pending transfer
        PendingTransfer storage transfer = pendingTransfers[transferId];
        transfer.transferId = transferId;
        transfer.sourceChain = block.chainid;
        transfer.targetChain = _targetChain;
        transfer.sender = msg.sender;
        transfer.receiver = _receiver;
        transfer.asset = _asset;
        transfer.amount = _amount;
        transfer.signatureCount = 0;
        transfer.status = TransferStatus.Pending;
        transfer.timestamp = block.timestamp;

        pendingTransferIds.push(transferId);

        emit TransferInitiated(
            transferId,
            block.chainid,
            _targetChain,
            msg.sender,
            _receiver,
            _asset,
            _amount
        );

        return transferId;
    }

    /**
     * @dev Initiate private cross-chain transfer
     */
    function initiatePrivateTransfer(
        uint256 _targetChain,
        bytes32 _stealthReceiver,
        address _asset,
        bytes32 _encryptedAmount,
        bytes32 _amountCommitment,
        bytes32 _nullifier,
        bytes calldata _zkProof
    ) external nonReentrant returns (bytes32) {
        if (!supportedChains[_targetChain].isActive) revert ChainNotSupported();

        // Verify amount proof
        if (!verifyBridgeAmountProof(_asset, _amountCommitment, _zkProof)) {
            revert InvalidZKProof();
        }

        bytes32 transferId = keccak256(abi.encodePacked(
            _targetChain,
            msg.sender,
            _stealthReceiver,
            _asset,
            _encryptedAmount,
            block.timestamp
        ));

        // Lock assets (amount is hidden)
        bytes32 lockKey = keccak256(abi.encodePacked(_targetChain, _asset));
        lockedAssets[lockKey] += 1; // Track count, not amount

        PendingTransfer storage transfer = pendingTransfers[transferId];
        transfer.transferId = transferId;
        transfer.sourceChain = block.chainid;
        transfer.targetChain = _targetChain;
        transfer.sender = msg.sender;
        transfer.receiver = address(0); // Hidden
        transfer.asset = _asset;
        transfer.amount = 0; // Hidden
        transfer.signatureCount = 0;
        transfer.status = TransferStatus.Pending;
        transfer.timestamp = block.timestamp;
        transfer.encryptedAmount = _encryptedAmount;
        transfer.stealthReceiver = _stealthReceiver;

        pendingTransferIds.push(transferId);

        emit PrivateTransferInitiated(transferId, _targetChain, _stealthReceiver);

        return transferId;
    }

    /**
     * @dev Sign transfer (validator only)
     */
    function signTransfer(
        bytes32 _transferId,
        bytes calldata _signature,
        bytes calldata _merkleProof
    ) external nonReentrant {
        Validator storage validator = validators[msg.sender];
        if (validator.addr == address(0)) revert NotValidator();
        if (!validator.isActive) revert ValidatorNotActive();

        PendingTransfer storage transfer = pendingTransfers[_transferId];
        if (transfer.transferId == bytes32(0)) revert TransferNotFound();
        if (transfer.status == TransferStatus.Executed) revert TransferAlreadyCompleted();

        // Verify validator signature
        if (!verifyValidatorSignature(msg.sender, _transferId, _signature)) {
            revert InvalidZKProof();
        }

        // Verify merkle proof (light client)
        if (!verifyMerkleProof(_merkleProof, _transferId)) {
            revert InvalidMerkleProof();
        }

        // Record signature
        if (!transfer.signatures[msg.sender]) {
            transfer.signatures[msg.sender] = true;
            transfer.signatureCount++;
        }

        // Update status
        if (transfer.signatureCount >= validatorThreshold) {
            transfer.status = TransferStatus.SignaturesComplete;
        } else {
            transfer.status = TransferStatus.SourceConfirmed;
        }

        validator.lastSignature = block.timestamp;

        emit TransferSigned(_transferId, msg.sender);
    }

    /**
     * @dev Execute transfer on target chain (release/mint)
     */
    function executeTransfer(bytes32 _transferId) external nonReentrant {
        PendingTransfer storage transfer = pendingTransfers[_transferId];
        if (transfer.transferId == bytes32(0)) revert TransferNotFound();
        if (transfer.status != TransferStatus.SignaturesComplete) {
            revert InsufficientSignatures();
        }
        if (transfer.targetChain != block.chainid) {
            revert ChainNotSupported(); // Wrong chain
        }
        if (completedTransfers[_transferId]) revert TransferAlreadyCompleted();

        // Check if wrapped asset exists
        bytes32 wrappedKey = keccak256(abi.encodePacked(transfer.sourceChain, transfer.asset));
        address wrappedAsset = wrappedAssets[wrappedKey];

        if (wrappedAsset != address(0)) {
            // Mint wrapped tokens
            _mintWrapped(wrappedAsset, transfer.receiver, transfer.amount);
        } else {
            // Release locked tokens
            bytes32 lockKey = keccak256(abi.encodePacked(transfer.sourceChain, transfer.asset));
            if (lockedAssets[lockKey] < transfer.amount) revert InsufficientLockedAssets();
            lockedAssets[lockKey] -= transfer.amount;

            require(
                IERC20(transfer.asset).transfer(transfer.receiver, transfer.amount),
                "Release failed"
            );
        }

        transfer.status = TransferStatus.Executed;
        completedTransfers[_transferId] = true;

        emit TransferExecuted(
            _transferId,
            block.chainid,
            transfer.receiver,
            transfer.amount
        );
    }

    /**
     * @dev Update merkle root (light client)
     */
    function updateMerkleRoot(
        uint256 _chainId,
        bytes32 _newRoot,
        bytes calldata _zkProof
    ) external {
        if (!verifyMerkleUpdateProof(_chainId, _newRoot, _zkProof)) {
            revert InvalidZKProof();
        }

        merkleRoots[_chainId] = _newRoot;
        emit MerkleRootUpdated(_chainId, _newRoot);
    }

    /**
     * @dev Get wrapped asset
     */
    function getWrappedAsset(uint256 _chainId, address _remoteAsset) 
        external 
        view 
        returns (address) 
    {
        bytes32 key = keccak256(abi.encodePacked(_chainId, _remoteAsset));
        return wrappedAssets[key];
    }

    /**
     * @dev Get locked amount
     */
    function getLockedAmount(uint256 _chainId, address _asset) 
        external 
        view 
        returns (uint256) 
    {
        bytes32 key = keccak256(abi.encodePacked(_chainId, _asset));
        return lockedAssets[key];
    }

    /**
     * @dev Get transfer status
     */
    function getTransferStatus(bytes32 _transferId) 
        external 
        view 
        returns (TransferStatus) 
    {
        return pendingTransfers[_transferId].status;
    }

    /**
     * @dev Get validator count
     */
    function getValidatorCount() external view returns (uint256) {
        return validatorList.length;
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
     * @dev Get pending transfers
     */
    function getPendingTransfers() external view returns (bytes32[] memory) {
        return pendingTransferIds;
    }

    // Internal functions
    function _mintWrapped(address _wrappedAsset, address _to, uint256 _amount) internal {
        // Call wrapped token contract to mint
        (bool success, ) = _wrappedAsset.call(
            abi.encodeWithSelector(bytes4(keccak256("mint(address,uint256)")), _to, _amount)
        );
        require(success, "Mint failed");
    }

    function verifyValidatorSignature(
        address _validator,
        bytes32 _transferId,
        bytes calldata _signature
    ) internal view returns (bool) {
        // Verify ECDSA signature
        bytes32 message = keccak256(abi.encodePacked(_validator, _transferId));
        bytes32 ethSignedMessage = keccak256(
            abi.encodePacked("Ethereum Signed Message:
32", message)
        );

        (bytes32 r, bytes32 s, uint8 v) = _splitSignature(_signature);
        address signer = ecrecover(ethSignedMessage, v, r, s);

        return signer == _validator && validators[_validator].isActive;
    }

    function _splitSignature(bytes memory _sig) 
        internal 
        pure 
        returns (bytes32 r, bytes32 s, uint8 v) 
    {
        require(_sig.length == 65, "Invalid signature length");
        assembly {
            r := mload(add(_sig, 32))
            s := mload(add(_sig, 64))
            v := byte(0, mload(add(_sig, 96)))
        }
    }

    function verifyMerkleProof(bytes calldata _proof, bytes32 _transferId) 
        internal 
        view 
        returns (bool) 
    {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyMerkle.selector,
                _proof,
                _transferId
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    function verifyMerkleUpdateProof(
        uint256 _chainId,
        bytes32 _newRoot,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyMerkleUpdate.selector,
                _chainId,
                _newRoot,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    function verifyBridgeAmountProof(
        address _asset,
        bytes32 _commitment,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyBridgeAmount.selector,
                _asset,
                _commitment,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    // Admin functions
    function setValidatorThreshold(uint256 _newThreshold) external onlyOwner {
        validatorThreshold = _newThreshold;
    }

    function setZKVerifier(address _newVerifier) external onlyOwner {
        zkVerifier = _newVerifier;
    }

    function deactivateChain(uint256 _chainId) external onlyOwner {
        supportedChains[_chainId].isActive = false;
    }

    function deactivateValidator(address _validator) external onlyOwner {
        validators[_validator].isActive = false;
    }
}

interface IZKVerifier {
    function verifyMerkle(bytes calldata proof, bytes32 transferId) 
        external 
        view 
        returns (bool);

    function verifyMerkleUpdate(uint256 chainId, bytes32 newRoot, bytes calldata proof) 
        external 
        view 
        returns (bool);

    function verifyBridgeAmount(address asset, bytes32 commitment, bytes calldata proof) 
        external 
        view 
        returns (bool);
}
