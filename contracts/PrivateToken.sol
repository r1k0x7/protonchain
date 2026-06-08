// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";

/**
 * @title ProtonPrivateToken
 * @dev Private ERC20 token with ZK-proof based confidential transfers
 * Compatible with Proton Chain EVM runtime
 */
contract ProtonPrivateToken is ERC20, Ownable, ReentrancyGuard {

    // ZK Verification contract address
    address public zkVerifier;

    // Merkle tree root for private balances
    bytes32 public merkleRoot;

    // Nullifier set to prevent double spending
    mapping(bytes32 => bool) public nullifierSpent;

    // Encrypted balance commitments
    mapping(address => bytes32) public balanceCommitments;

    // View key registry (optional)
    mapping(address => bytes32) public viewKeys;

    // Events
    event PrivateTransfer(
        bytes32 indexed nullifier,
        bytes32 indexed commitment,
        bytes32 encryptedAmount
    );

    event PublicMint(address indexed to, uint256 amount);
    event PublicBurn(address indexed from, uint256 amount);
    event MerkleRootUpdated(bytes32 newRoot);
    event ViewKeyRegistered(address indexed account);

    // Errors
    error InvalidZKProof();
    error NullifierAlreadySpent();
    error InvalidMerkleRoot();
    error InsufficientPublicBalance();
    error OnlyPrivateTransfersAllowed();

    modifier validZKProof(
        bytes32 _nullifier,
        bytes32 _commitment,
        bytes32 _root,
        bytes calldata _proof
    ) {
        if (!verifyProof(_nullifier, _commitment, _root, _proof)) {
            revert InvalidZKProof();
        }
        _;
    }

    constructor(
        string memory _name,
        string memory _symbol,
        address _zkVerifier
    ) ERC20(_name, _symbol) Ownable(msg.sender) {
        zkVerifier = _zkVerifier;
        merkleRoot = bytes32(0);
    }

    /**
     * @dev Public mint - visible to all
     */
    function publicMint(address _to, uint256 _amount) external onlyOwner {
        _mint(_to, _amount);
        emit PublicMint(_to, _amount);
    }

    /**
     * @dev Public burn
     */
    function publicBurn(uint256 _amount) external {
        if (balanceOf(msg.sender) < _amount) {
            revert InsufficientPublicBalance();
        }
        _burn(msg.sender, _amount);
        emit PublicBurn(msg.sender, _amount);
    }

    /**
     * @dev Private transfer using ZK proof
     * Hides sender, receiver, and amount
     */
    function privateTransfer(
        bytes32 _nullifier,
        bytes32 _commitment,
        bytes32 _root,
        bytes32 _encryptedAmount,
        bytes calldata _proof
    ) external nonReentrant validZKProof(_nullifier, _commitment, _root, _proof) {

        // Check nullifier not spent
        if (nullifierSpent[_nullifier]) {
            revert NullifierAlreadySpent();
        }

        // Verify merkle root
        if (_root != merkleRoot) {
            revert InvalidMerkleRoot();
        }

        // Mark nullifier as spent
        nullifierSpent[_nullifier] = true;

        // Update merkle root with new commitment
        merkleRoot = keccak256(abi.encodePacked(merkleRoot, _commitment));

        emit PrivateTransfer(_nullifier, _commitment, _encryptedAmount);
    }

    /**
     * @dev Batch private transfers for efficiency
     */
    function batchPrivateTransfer(
        bytes32[] calldata _nullifiers,
        bytes32[] calldata _commitments,
        bytes32 _root,
        bytes32[] calldata _encryptedAmounts,
        bytes calldata _proof
    ) external nonReentrant {
        require(
            _nullifiers.length == _commitments.length &&
            _commitments.length == _encryptedAmounts.length,
            "Array length mismatch"
        );

        // Verify batch proof
        if (!verifyBatchProof(_nullifiers, _commitments, _root, _proof)) {
            revert InvalidZKProof();
        }

        for (uint i = 0; i < _nullifiers.length; i++) {
            if (nullifierSpent[_nullifiers[i]]) {
                revert NullifierAlreadySpent();
            }
            nullifierSpent[_nullifiers[i]] = true;

            emit PrivateTransfer(
                _nullifiers[i],
                _commitments[i],
                _encryptedAmounts[i]
            );
        }

        // Update merkle root
        bytes32 newRoot = _root;
        for (uint i = 0; i < _commitments.length; i++) {
            newRoot = keccak256(abi.encodePacked(newRoot, _commitments[i]));
        }
        merkleRoot = newRoot;
    }

    /**
     * @dev Register view key for balance decryption
     */
    function registerViewKey(bytes32 _viewKey) external {
        viewKeys[msg.sender] = _viewKey;
        emit ViewKeyRegistered(msg.sender);
    }

    /**
     * @dev Update balance commitment (called by ZK verifier)
     */
    function updateBalanceCommitment(
        address _account,
        bytes32 _newCommitment
    ) external {
        require(msg.sender == zkVerifier, "Only ZK verifier");
        balanceCommitments[_account] = _newCommitment;
    }

    /**
     * @dev Update merkle root (governance)
     */
    function updateMerkleRoot(bytes32 _newRoot) external onlyOwner {
        merkleRoot = _newRoot;
        emit MerkleRootUpdated(_newRoot);
    }

    /**
     * @dev Update ZK verifier address
     */
    function setZKVerifier(address _newVerifier) external onlyOwner {
        zkVerifier = _newVerifier;
    }

    /**
     * @dev Verify ZK proof (calls verifier contract)
     */
    function verifyProof(
        bytes32 _nullifier,
        bytes32 _commitment,
        bytes32 _root,
        bytes calldata _proof
    ) public view returns (bool) {
        // Call external verifier contract
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyTransfer.selector,
                _nullifier,
                _commitment,
                _root,
                _proof
            )
        );

        if (!success) return false;
        return abi.decode(result, (bool));
    }

    function verifyBatchProof(
        bytes32[] calldata _nullifiers,
        bytes32[] calldata _commitments,
        bytes32 _root,
        bytes calldata _proof
    ) public view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyBatchTransfer.selector,
                _nullifiers,
                _commitments,
                _root,
                _proof
            )
        );

        if (!success) return false;
        return abi.decode(result, (bool));
    }

    /**
     * @dev Get encrypted balance (requires view key)
     */
    function getEncryptedBalance(address _account) external view returns (bytes32) {
        return balanceCommitments[_account];
    }

    /**
     * @dev Check if nullifier has been spent
     */
    function isNullifierSpent(bytes32 _nullifier) external view returns (bool) {
        return nullifierSpent[_nullifier];
    }

    /**
     * @dev Get total supply (public)
     */
    function getTotalSupply() external view returns (uint256) {
        return totalSupply();
    }
}

interface IZKVerifier {
    function verifyTransfer(
        bytes32 nullifier,
        bytes32 commitment,
        bytes32 root,
        bytes calldata proof
    ) external view returns (bool);

    function verifyBatchTransfer(
        bytes32[] calldata nullifiers,
        bytes32[] calldata commitments,
        bytes32 root,
        bytes calldata proof
    ) external view returns (bool);
}
