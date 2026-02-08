import { Blockchain, SandboxContract, TreasuryContract } from '@ton/sandbox';
import { toNano } from '@ton/core';
import { Main } from '../build/Main/Main_Main';
import '@ton/test-utils';

describe('Main', () => {
    let blockchain: Blockchain;
    let deployer: SandboxContract<TreasuryContract>;
    let main: SandboxContract<Main>;

    beforeEach(async () => {
        blockchain = await Blockchain.create();

        main = blockchain.openContract(await Main.fromInit());

        deployer = await blockchain.treasury('deployer');

        const deployResult = await main.send(
            deployer.getSender(),
            {
                value: toNano('0.05'),
            },
            null,
        );

        expect(deployResult.transactions).toHaveTransaction({
            from: deployer.address,
            to: main.address,
            deploy: true,
            success: true,
        });
    });

    it('should deploy', async () => {
        // the check is done inside beforeEach
        // blockchain and main are ready to use
    });
});
