// Entry point of the integration tests: runs inside VS Code's extension host.

import Mocha from 'mocha';

export function run(): Promise<void> {
    const mocha = new Mocha({ ui: 'bdd', color: true, timeout: 60_000, slow: 5_000 });
    // Bundled into this file; registering through `suite.emit` gives the test
    // file mocha's globals (describe, it) before it is evaluated.
    mocha.suite.emit('pre-require', globalThis, 'extension.test', mocha);
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    require('./extension.test');
    return new Promise((resolve, reject) => {
        mocha.run((failures) => (failures > 0 ? reject(new Error(`${failures} test(s) failed`)) : resolve()));
    });
}
