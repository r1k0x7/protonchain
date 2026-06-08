const { ethers } = require('hardhat');

async function main() {
  const [deployer] = await ethers.getSigners();
  console.log('Deploying with:', deployer.address);

  // Deploy ZK Verifier
  const ZKVerifier = await ethers.getContractFactory('ProtonZKVerifier');
  const zkVerifier = await ZKVerifier.deploy();
  await zkVerifier.deployed();
  console.log('ZKVerifier:', zkVerifier.address);

  // Set dummy VK
  await zkVerifier.setVerificationKey([1,2], [[3,4],[5,6]], [[7,8],[9,10]], [[11,12],[13,14]], [[15,16]]);

  // Deploy contracts
  const contracts = {};

  const PrivateToken = await ethers.getContractFactory('ProtonPrivateToken');
  contracts.token = await PrivateToken.deploy('Private PROTON', 'pPROTON', zkVerifier.address);
  await contracts.token.deployed();

  const DEX = await ethers.getContractFactory('ProtonDEX');
  contracts.dex = await DEX.deploy(deployer.address, zkVerifier.address);
  await contracts.dex.deployed();

  const Staking = await ethers.getContractFactory('ProtonStaking');
  contracts.staking = await Staking.deploy(contracts.token.address, zkVerifier.address);
  await contracts.staking.deployed();

  const NFT = await ethers.getContractFactory('ProtonPrivateNFT');
  contracts.nft = await NFT.deploy('Proton NFT', 'pNFT', zkVerifier.address);
  await contracts.nft.deployed();

  const Bridge = await ethers.getContractFactory('ProtonBridge');
  contracts.bridge = await Bridge.deploy(2, zkVerifier.address);
  await contracts.bridge.deployed();

  console.log('\nDeployment complete!');
  for (const [name, contract] of Object.entries(contracts)) {
    console.log(`${name}: ${contract.address}`);
  }
}

main().catch(console.error);
