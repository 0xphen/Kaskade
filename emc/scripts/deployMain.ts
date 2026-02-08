import { toNano } from '@ton/core';
import { Main } from '../build/Main/Main_Main';
import { NetworkProvider } from '@ton/blueprint';

export async function run(provider: NetworkProvider) {
    const main = provider.open(await Main.fromInit());

    await main.send(
        provider.sender(),
        {
            value: toNano('0.05'),
        },
        null,
    );

    await provider.waitForDeploy(main.address);

    // run methods on `main`
}
