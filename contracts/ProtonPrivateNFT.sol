// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC721/ERC721.sol";
import "@openzeppelin/contracts/token/ERC721/extensions/ERC721Enumerable.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";

/**
 * @title ProtonPrivateNFT
 * @dev Private NFT with stealth ownership and encrypted metadata
 */
contract ProtonPrivateNFT is ERC721, ERC721Enumerable, Ownable, ReentrancyGuard {

    // Token data
    struct TokenData {
        uint256 tokenId;
        string publicMetadata;
        bytes32 encryptedMetadataHash;
        bool isPrivate;
        uint256 createdAt;
    }

    mapping(uint256 => TokenData) public tokenData;
    mapping(uint256 => address) public publicOwners;
    mapping(uint256 => bytes32) public stealthOwners; // Stealth address hash
    mapping(uint256 => bytes) public encryptedMetadata;

    // Nullifier set for private transfers
    mapping(bytes32 => bool) public nullifierSpent;

    // Approval tracking
    mapping(uint256 => address) public tokenApprovals;
    mapping(address => mapping(address => bool)) public operatorApprovals;

    // ZK verifier
    address public zkVerifier;

    // Counters
    uint256 public totalMinted;
    uint256 public constant MAX_SUPPLY = 100000;

    // Events
    event PrivateTransfer(
        uint256 indexed tokenId,
        bytes32 indexed nullifier,
        bytes32 newStealth
    );
    event MetadataEncrypted(uint256 indexed tokenId, bytes32 metadataHash);
    event ViewKeyRevealed(uint256 indexed tokenId, address indexed viewer);

    // Errors
    error TokenNotFound();
    error NotOwnerOrApproved();
    error NullifierAlreadySpent();
    error InvalidZKProof();
    error MaxSupplyReached();
    error TokenIsPrivate();
    error TokenIsPublic();

    constructor(
        string memory _name,
        string memory _symbol,
        address _zkVerifier
    ) ERC721(_name, _symbol) Ownable(msg.sender) {
        zkVerifier = _zkVerifier;
    }

    /**
     * @dev Mint public NFT
     */
    function publicMint(
        address _to,
        string memory _metadata
    ) external onlyOwner returns (uint256) {
        if (totalMinted >= MAX_SUPPLY) revert MaxSupplyReached();

        uint256 tokenId = ++totalMinted;

        tokenData[tokenId] = TokenData({
            tokenId: tokenId,
            publicMetadata: _metadata,
            encryptedMetadataHash: bytes32(0),
            isPrivate: false,
            createdAt: block.timestamp
        });

        publicOwners[tokenId] = _to;
        _safeMint(_to, tokenId);

        return tokenId;
    }

    /**
     * @dev Mint private NFT with encrypted metadata
     */
    function privateMint(
        bytes32 _stealthOwner,
        bytes calldata _encryptedMetadata,
        bytes32 _metadataHash,
        bytes calldata _zkProof
    ) external onlyOwner returns (uint256) {
        if (totalMinted >= MAX_SUPPLY) revert MaxSupplyReached();

        // Verify metadata proof
        if (!verifyMetadataProof(_metadataHash, _encryptedMetadata, _zkProof)) {
            revert InvalidZKProof();
        }

        uint256 tokenId = ++totalMinted;

        tokenData[tokenId] = TokenData({
            tokenId: tokenId,
            publicMetadata: "",
            encryptedMetadataHash: _metadataHash,
            isPrivate: true,
            createdAt: block.timestamp
        });

        stealthOwners[tokenId] = _stealthOwner;
        encryptedMetadata[tokenId] = _encryptedMetadata;

        // Mint to zero address (ownership is via stealth)
        _safeMint(address(0), tokenId);

        emit MetadataEncrypted(tokenId, _metadataHash);

        return tokenId;
    }

    /**
     * @dev Public transfer
     */
    function publicTransfer(
        address _from,
        address _to,
        uint256 _tokenId
    ) external nonReentrant {
        if (!_isApprovedOrOwner(msg.sender, _tokenId)) revert NotOwnerOrApproved();
        if (tokenData[_tokenId].isPrivate) revert TokenIsPrivate();

        publicOwners[_tokenId] = _to;
        delete tokenApprovals[_tokenId];

        _transfer(_from, _to, _tokenId);
    }

    /**
     * @dev Private transfer with stealth address
     */
    function privateTransfer(
        uint256 _tokenId,
        bytes32 _newStealthOwner,
        bytes32 _nullifier,
        bytes calldata _zkProof
    ) external nonReentrant {
        if (!_exists(_tokenId)) revert TokenNotFound();
        if (tokenData[_tokenId].isPrivate == false) revert TokenIsPublic();
        if (nullifierSpent[_nullifier]) revert NullifierAlreadySpent();

        // Verify ownership proof
        bytes32 currentStealth = stealthOwners[_tokenId];
        if (!verifyOwnershipProof(_tokenId, currentStealth, _nullifier, _zkProof)) {
            revert InvalidZKProof();
        }

        // Mark nullifier spent
        nullifierSpent[_nullifier] = true;

        // Update stealth owner
        stealthOwners[_tokenId] = _newStealthOwner;

        emit PrivateTransfer(_tokenId, _nullifier, _newStealthOwner);
    }

    /**
     * @dev Approve operator
     */
    function approve(address _to, uint256 _tokenId) external {
        address owner = _ownerOf(_tokenId);
        if (owner != msg.sender && !operatorApprovals[owner][msg.sender]) {
            revert NotOwnerOrApproved();
        }

        tokenApprovals[_tokenId] = _to;
        emit Approval(owner, _to, _tokenId);
    }

    /**
     * @dev Set approval for all
     */
    function setApprovalForAll(address _operator, bool _approved) external {
        operatorApprovals[msg.sender][_operator] = _approved;
        emit ApprovalForAll(msg.sender, _operator, _approved);
    }

    /**
     * @dev Get owner (public tokens only)
     */
    function ownerOf(uint256 _tokenId) public view override returns (address) {
        if (tokenData[_tokenId].isPrivate) {
            return address(0); // Private tokens have no public owner
        }
        return publicOwners[_tokenId];
    }

    /**
     * @dev Get encrypted metadata
     */
    function getEncryptedMetadata(uint256 _tokenId) 
        external 
        view 
        returns (bytes memory) 
    {
        return encryptedMetadata[_tokenId];
    }

    /**
     * @dev Check if token is private
     */
    function isPrivate(uint256 _tokenId) external view returns (bool) {
        return tokenData[_tokenId].isPrivate;
    }

    /**
     * @dev Get token metadata (public)
     */
    function getMetadata(uint256 _tokenId) external view returns (string memory) {
        return tokenData[_tokenId].publicMetadata;
    }

    /**
     * @dev Get total supply
     */
    function totalSupply() public view override(ERC721Enumerable) returns (uint256) {
        return totalMinted;
    }

    /**
     * @dev Get all tokens owned by address (public only)
     */
    function tokensOfOwner(address _owner) external view returns (uint256[] memory) {
        uint256 tokenCount = balanceOf(_owner);
        uint256[] memory tokenIds = new uint256[](tokenCount);

        for (uint i = 0; i < tokenCount; i++) {
            tokenIds[i] = tokenOfOwnerByIndex(_owner, i);
        }

        return tokenIds;
    }

    /**
     * @dev Reveal metadata (requires view key - off-chain verification)
     */
    function revealMetadata(
        uint256 _tokenId,
        bytes32 _viewKey,
        bytes calldata _zkProof
    ) external {
        if (!verifyViewKey(_tokenId, _viewKey, _zkProof)) {
            revert InvalidZKProof();
        }

        emit ViewKeyRevealed(_tokenId, msg.sender);
    }

    // Internal functions
    function _isApprovedOrOwner(address _spender, uint256 _tokenId) 
        internal 
        view 
        returns (bool) 
    {
        address owner = _ownerOf(_tokenId);
        return (
            _spender == owner ||
            tokenApprovals[_tokenId] == _spender ||
            operatorApprovals[owner][_spender]
        );
    }

    function _ownerOf(uint256 _tokenId) internal view returns (address) {
        return publicOwners[_tokenId];
    }

    function verifyMetadataProof(
        bytes32 _metadataHash,
        bytes calldata _encryptedMetadata,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyMetadata.selector,
                _metadataHash,
                _encryptedMetadata,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    function verifyOwnershipProof(
        uint256 _tokenId,
        bytes32 _currentStealth,
        bytes32 _nullifier,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyNFTOwnership.selector,
                _tokenId,
                _currentStealth,
                _nullifier,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    function verifyViewKey(
        uint256 _tokenId,
        bytes32 _viewKey,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifyViewKey.selector,
                _tokenId,
                _viewKey,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }

    // Required overrides
    function _beforeTokenTransfer(
        address from,
        address to,
        uint256 tokenId,
        uint256 batchSize
    ) internal override(ERC721, ERC721Enumerable) {
        super._beforeTokenTransfer(from, to, tokenId, batchSize);
    }

    function supportsInterface(bytes4 interfaceId) 
        public 
        view 
        override(ERC721, ERC721Enumerable) 
        returns (bool) 
    {
        return super.supportsInterface(interfaceId);
    }

    function setZKVerifier(address _newVerifier) external onlyOwner {
        zkVerifier = _newVerifier;
    }
}

interface IZKVerifier {
    function verifyMetadata(
        bytes32 metadataHash,
        bytes calldata encryptedMetadata,
        bytes calldata proof
    ) external view returns (bool);

    function verifyNFTOwnership(
        uint256 tokenId,
        bytes32 currentStealth,
        bytes32 nullifier,
        bytes calldata proof
    ) external view returns (bool);

    function verifyViewKey(
        uint256 tokenId,
        bytes32 viewKey,
        bytes calldata proof
    ) external view returns (bool);
}
