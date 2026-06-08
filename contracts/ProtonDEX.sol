// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

/**
 * @title ProtonDEX
 * @dev Decentralized exchange with privacy features
 * Supports encrypted order books and private swaps
 */
contract ProtonDEX is ReentrancyGuard, Ownable {

    // Fee structure
    uint256 public constant FEE_DENOMINATOR = 10000;
    uint256 public tradingFee = 30; // 0.3%
    address public feeRecipient;

    // ZK Verifier
    address public zkVerifier;

    // Trading pairs
    struct TradingPair {
        address tokenA;
        address tokenB;
        uint256 reserveA;
        uint256 reserveB;
        uint256 totalLiquidity;
        uint256 encryptedVolume;
        bool active;
    }

    mapping(bytes32 => TradingPair) public pairs;
    mapping(bytes32 => uint256) public pairLiquidity;

    // Encrypted order book
    struct EncryptedOrder {
        address owner;
        bytes32 encryptedAmount;
        bytes32 encryptedPrice;
        bool isBuy;
        bytes32 zkProof;
        uint256 timestamp;
        bool active;
    }

    mapping(bytes32 => EncryptedOrder) public encryptedOrders;
    mapping(bytes32 => bytes32[]) public pairOrderHashes;

    // LP tokens
    mapping(bytes32 => mapping(address => uint256)) public lpBalances;

    // Events
    event PairCreated(address indexed tokenA, address indexed tokenB, bytes32 pairHash);
    event LiquidityAdded(
        address indexed provider,
        bytes32 indexed pairHash,
        uint256 amountA,
        uint256 amountB,
        uint256 liquidity
    );
    event LiquidityRemoved(
        address indexed provider,
        bytes32 indexed pairHash,
        uint256 amountA,
        uint256 amountB,
        uint256 liquidity
    );
    event PrivateSwap(
        bytes32 indexed pairHash,
        bytes32 indexed nullifier,
        bytes32 encryptedAmountIn,
        bytes32 encryptedAmountOut
    );
    event EncryptedOrderPlaced(
        bytes32 indexed orderHash,
        bytes32 indexed pairHash,
        bool isBuy
    );
    event OrderCancelled(bytes32 indexed orderHash);

    // Errors
    error PairAlreadyExists();
    error PairNotFound();
    error InvalidAmount();
    error InvalidRatio();
    error InsufficientLiquidity();
    error SlippageExceeded();
    error InvalidZKProof();
    error OrderNotFound();
    error Unauthorized();

    constructor(address _feeRecipient, address _zkVerifier) Ownable(msg.sender) {
        feeRecipient = _feeRecipient;
        zkVerifier = _zkVerifier;
    }

    /**
     * @dev Create trading pair
     */
    function createPair(address _tokenA, address _tokenB) external onlyOwner {
        if (_tokenA == _tokenB) revert InvalidAmount();

        (address token0, address token1) = _sortTokens(_tokenA, _tokenB);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        if (pairs[pairHash].tokenA != address(0)) revert PairAlreadyExists();

        pairs[pairHash] = TradingPair({
            tokenA: token0,
            tokenB: token1,
            reserveA: 0,
            reserveB: 0,
            totalLiquidity: 0,
            encryptedVolume: 0,
            active: true
        });

        emit PairCreated(token0, token1, pairHash);
    }

    /**
     * @dev Add liquidity
     */
    function addLiquidity(
        address _tokenA,
        address _tokenB,
        uint256 _amountA,
        uint256 _amountB,
        uint256 _minLiquidity
    ) external nonReentrant returns (uint256 liquidity) {
        (address token0, address token1) = _sortTokens(_tokenA, _tokenB);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        TradingPair storage pair = pairs[pairHash];
        if (pair.tokenA == address(0)) revert PairNotFound();

        // Transfer tokens
        _safeTransferFrom(token0, msg.sender, address(this), _amountA);
        _safeTransferFrom(token1, msg.sender, address(this), _amountB);

        // Calculate liquidity
        if (pair.totalLiquidity == 0) {
            liquidity = _sqrt(_amountA * _amountB);
        } else {
            uint256 liquidityA = (_amountA * pair.totalLiquidity) / pair.reserveA;
            uint256 liquidityB = (_amountB * pair.totalLiquidity) / pair.reserveB;
            liquidity = liquidityA < liquidityB ? liquidityA : liquidityB;
        }

        if (liquidity < _minLiquidity) revert SlippageExceeded();

        // Update reserves
        pair.reserveA += _amountA;
        pair.reserveB += _amountB;
        pair.totalLiquidity += liquidity;

        // Mint LP tokens
        lpBalances[pairHash][msg.sender] += liquidity;

        emit LiquidityAdded(msg.sender, pairHash, _amountA, _amountB, liquidity);
    }

    /**
     * @dev Remove liquidity
     */
    function removeLiquidity(
        address _tokenA,
        address _tokenB,
        uint256 _liquidity,
        uint256 _minAmountA,
        uint256 _minAmountB
    ) external nonReentrant returns (uint256 amountA, uint256 amountB) {
        (address token0, address token1) = _sortTokens(_tokenA, _tokenB);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        TradingPair storage pair = pairs[pairHash];
        if (pair.tokenA == address(0)) revert PairNotFound();

        if (lpBalances[pairHash][msg.sender] < _liquidity) revert InsufficientLiquidity();

        // Calculate amounts
        amountA = (_liquidity * pair.reserveA) / pair.totalLiquidity;
        amountB = (_liquidity * pair.reserveB) / pair.totalLiquidity;

        if (amountA < _minAmountA || amountB < _minAmountB) revert SlippageExceeded();

        // Update
        lpBalances[pairHash][msg.sender] -= _liquidity;
        pair.reserveA -= amountA;
        pair.reserveB -= amountB;
        pair.totalLiquidity -= _liquidity;

        // Transfer back
        _safeTransfer(token0, msg.sender, amountA);
        _safeTransfer(token1, msg.sender, amountB);

        emit LiquidityRemoved(msg.sender, pairHash, amountA, amountB, _liquidity);
    }

    /**
     * @dev Private swap using ZK proof
     */
    function privateSwap(
        address _tokenIn,
        address _tokenOut,
        bytes32 _nullifier,
        bytes32 _encryptedAmountIn,
        bytes32 _encryptedAmountOut,
        bytes32 _minAmountOutCommitment,
        bytes calldata _zkProof
    ) external nonReentrant {
        (address token0, address token1) = _sortTokens(_tokenIn, _tokenOut);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        TradingPair storage pair = pairs[pairHash];
        if (pair.tokenA == address(0)) revert PairNotFound();

        // Verify ZK proof
        if (!verifySwapProof(
            pairHash,
            _nullifier,
            _encryptedAmountIn,
            _encryptedAmountOut,
            _zkProof
        )) revert InvalidZKProof();

        // Execute swap (amounts are encrypted, verified off-chain)
        // In production, this would update encrypted reserves

        emit PrivateSwap(pairHash, _nullifier, _encryptedAmountIn, _encryptedAmountOut);
    }

    /**
     * @dev Public swap (non-private fallback)
     */
    function swap(
        address _tokenIn,
        address _tokenOut,
        uint256 _amountIn,
        uint256 _minAmountOut,
        address _to
    ) external nonReentrant returns (uint256 amountOut) {
        (address token0, address token1) = _sortTokens(_tokenIn, _tokenOut);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        TradingPair storage pair = pairs[pairHash];
        if (pair.tokenA == address(0)) revert PairNotFound();

        // Calculate output with fee
        uint256 amountInWithFee = _amountIn * (FEE_DENOMINATOR - tradingFee) / FEE_DENOMINATOR;

        if (_tokenIn == token0) {
            amountOut = (amountInWithFee * pair.reserveB) / (pair.reserveA + amountInWithFee);
            pair.reserveA += _amountIn;
            pair.reserveB -= amountOut;
        } else {
            amountOut = (amountInWithFee * pair.reserveA) / (pair.reserveB + amountInWithFee);
            pair.reserveB += _amountIn;
            pair.reserveA -= amountOut;
        }

        if (amountOut < _minAmountOut) revert SlippageExceeded();

        // Transfer
        _safeTransferFrom(_tokenIn, msg.sender, address(this), _amountIn);
        _safeTransfer(_tokenOut, _to, amountOut);

        // Transfer fee
        uint256 fee = _amountIn - amountInWithFee;
        _safeTransfer(_tokenIn, feeRecipient, fee);
    }

    /**
     * @dev Place encrypted limit order
     */
    function placeEncryptedOrder(
        address _tokenA,
        address _tokenB,
        bytes32 _encryptedAmount,
        bytes32 _encryptedPrice,
        bool _isBuy,
        bytes32 _zkProof
    ) external returns (bytes32 orderHash) {
        (address token0, address token1) = _sortTokens(_tokenA, _tokenB);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        if (pairs[pairHash].tokenA == address(0)) revert PairNotFound();

        orderHash = keccak256(abi.encodePacked(
            msg.sender,
            pairHash,
            _encryptedAmount,
            _encryptedPrice,
            _isBuy,
            block.timestamp
        ));

        encryptedOrders[orderHash] = EncryptedOrder({
            owner: msg.sender,
            encryptedAmount: _encryptedAmount,
            encryptedPrice: _encryptedPrice,
            isBuy: _isBuy,
            zkProof: _zkProof,
            timestamp: block.timestamp,
            active: true
        });

        pairOrderHashes[pairHash].push(orderHash);

        emit EncryptedOrderPlaced(orderHash, pairHash, _isBuy);
    }

    /**
     * @dev Cancel order
     */
    function cancelOrder(bytes32 _orderHash) external {
        EncryptedOrder storage order = encryptedOrders[_orderHash];
        if (order.owner == address(0)) revert OrderNotFound();
        if (order.owner != msg.sender) revert Unauthorized();

        order.active = false;
        emit OrderCancelled(_orderHash);
    }

    /**
     * @dev Get reserves
     */
    function getReserves(address _tokenA, address _tokenB) 
        external 
        view 
        returns (uint256 reserveA, uint256 reserveB) 
    {
        (address token0, address token1) = _sortTokens(_tokenA, _tokenB);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        TradingPair storage pair = pairs[pairHash];
        return (pair.reserveA, pair.reserveB);
    }

    /**
     * @dev Calculate swap amount
     */
    function calculateSwapAmount(
        address _tokenIn,
        address _tokenOut,
        uint256 _amountIn
    ) external view returns (uint256 amountOut) {
        (address token0, address token1) = _sortTokens(_tokenIn, _tokenOut);
        bytes32 pairHash = keccak256(abi.encodePacked(token0, token1));

        TradingPair storage pair = pairs[pairHash];
        if (pair.tokenA == address(0)) return 0;

        uint256 amountInWithFee = _amountIn * (FEE_DENOMINATOR - tradingFee) / FEE_DENOMINATOR;

        if (_tokenIn == token0) {
            amountOut = (amountInWithFee * pair.reserveB) / (pair.reserveA + amountInWithFee);
        } else {
            amountOut = (amountInWithFee * pair.reserveA) / (pair.reserveB + amountInWithFee);
        }
    }

    /**
     * @dev Update trading fee
     */
    function setTradingFee(uint256 _newFee) external onlyOwner {
        require(_newFee <= 1000, "Fee too high"); // Max 10%
        tradingFee = _newFee;
    }

    /**
     * @dev Update fee recipient
     */
    function setFeeRecipient(address _newRecipient) external onlyOwner {
        feeRecipient = _newRecipient;
    }

    // Internal functions
    function _sortTokens(address _tokenA, address _tokenB) 
        internal 
        pure 
        returns (address token0, address token1) 
    {
        require(_tokenA != _tokenB, "Same token");
        (token0, token1) = _tokenA < _tokenB ? (_tokenA, _tokenB) : (_tokenB, _tokenA);
    }

    function _sqrt(uint256 _x) internal pure returns (uint256 y) {
        uint256 z = (_x + 1) / 2;
        y = _x;
        while (z < y) {
            y = z;
            z = (_x / z + z) / 2;
        }
    }

    function _safeTransfer(address _token, address _to, uint256 _amount) internal {
        (bool success, bytes memory data) = _token.call(
            abi.encodeWithSelector(IERC20.transfer.selector, _to, _amount)
        );
        require(success && (data.length == 0 || abi.decode(data, (bool))), "Transfer failed");
    }

    function _safeTransferFrom(
        address _token,
        address _from,
        address _to,
        uint256 _amount
    ) internal {
        (bool success, bytes memory data) = _token.call(
            abi.encodeWithSelector(IERC20.transferFrom.selector, _from, _to, _amount)
        );
        require(success && (data.length == 0 || abi.decode(data, (bool))), "TransferFrom failed");
    }

    function verifySwapProof(
        bytes32 _pairHash,
        bytes32 _nullifier,
        bytes32 _encryptedAmountIn,
        bytes32 _encryptedAmountOut,
        bytes calldata _proof
    ) internal view returns (bool) {
        (bool success, bytes memory result) = zkVerifier.staticcall(
            abi.encodeWithSelector(
                IZKVerifier.verifySwap.selector,
                _pairHash,
                _nullifier,
                _encryptedAmountIn,
                _encryptedAmountOut,
                _proof
            )
        );
        if (!success) return false;
        return abi.decode(result, (bool));
    }
}

interface IZKVerifier {
    function verifySwap(
        bytes32 pairHash,
        bytes32 nullifier,
        bytes32 encryptedAmountIn,
        bytes32 encryptedAmountOut,
        bytes calldata proof
    ) external view returns (bool);
}
