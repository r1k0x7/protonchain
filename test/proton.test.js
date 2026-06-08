const { expect } = require('chai');
const { ethers } = require('hardhat');

describe('Proton Contracts', () => {
  let deployer, user1, user2;
  let zkVerifier, token, dex, staking, nft, bridge;

  beforeEach(async () => {
    [deployer, user1, user2] = await ethers.getSigners();

    const ZKVerifier = await ethers.getContractFactory('ProtonZKVerifier');
    zkVerifier = await ZKVerifier.deploy();
    await zkVerifier.deployed();
    await zkVerifier.setVerificationKey([1,2], [[3,4],[5,6]], [[7,8],[9,10]], [[11,12],[13,14]], [[15,16]]);

    const Token = await ethers.getContractFactory('ProtonPrivateToken');
    token = await Token.deploy('Private PROTON', 'pPROTON', zkVerifier.address);
    await token.deployed();

    const DEX = await ethers.getContractFactory('ProtonDEX');
    dex = await DEX.deploy(deployer.address, zkVerifier.address);
    await dex.deployed();

    const Staking = await ethers.getContractFactory('ProtonStaking');
    staking = await Staking.deploy(token.address, zkVerifier.address);
    await staking.deployed();

    const NFT = await ethers.getContractFactory('ProtonPrivateNFT');
    nft = await NFT.deploy('Proton NFT', 'pNFT', zkVerifier.address);
    await nft.deployed();

    const Bridge = await ethers.getContractFactory('ProtonBridge');
    bridge = await Bridge.deploy(2, zkVerifier.address);
    await bridge.deployed();
  });

  it('Token: mint & burn', async () => {
    await token.publicMint(user1.address, 1000);
    expect(await token.balanceOf(user1.address)).to.equal(1000);
    await token.connect(user1).publicBurn(500);
    expect(await token.balanceOf(user1.address)).to.equal(500);
  });

  it('DEX: create pair', async () => {
    await expect(dex.createPair(token.address, token.address)).to.be.reverted;
  });

  it('Staking: register validator', async () => {
    await staking.connect(user1).registerValidator(500);
    const v = await staking.validators(user1.address);
    expect(v.commission).to.equal(500);
  });

  it('NFT: mint public', async () => {
    await nft.publicMint(user1.address, 'ipfs://test');
    expect(await nft.ownerOf(1)).to.equal(user1.address);
  });

  it('Bridge: add chain', async () => {
    await bridge.addChain(1, ethers.constants.AddressZero, 12);
    const chain = await bridge.supportedChains(1);
    expect(chain.isActive).to.be.true;
  });

  it('ZKVerifier: verify proof', async () => {
    const result = await zkVerifier.verifyTransfer(
      ethers.utils.randomBytes(32),
      ethers.utils.randomBytes(32),
      ethers.utils.randomBytes(32),
      ethers.utils.randomBytes(256)
    );
    expect(result).to.be.true;
  });
});
