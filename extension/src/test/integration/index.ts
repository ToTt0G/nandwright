// Entry point of the integration tests: runs inside VS Code's extension host.

import * as fs from 'node:fs';
import Mocha from 'mocha';

export function run(): Promise<void> {
    const mocha = new Mocha({ ui: 'bdd', color: true, timeout: 60_000, slow: 5_000 });
    // Bundled into this file; registering through `suite.emit` gives the test
    // file mocha's globals (describe, it) before it is evaluated.
    mocha.suite.emit('pre-require', globalThis, 'extension.test', mocha);
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    require('./extension.test');
    return new Promise((resolve, reject) => {
        // The extension host's console does not reach the terminal on every
        // platform, so the results also go to a file the runner prints.
        const lines: string[] = [];
        const runner = mocha.run((failures) => {
            lines.push(`${runner.stats?.passes ?? 0} passing, ${failures} failing`);
            const report = process.env.NANDWRIGHT_TEST_REPORT;
            if (report) {
                fs.writeFileSync(report, `${lines.join('\n')}\n`);
            }
            if (failures > 0) {
                reject(new Error(`${failures} test(s) failed`));
            } else {
                resolve();
            }
        });
        runner.on('pass', (test) => lines.push(`  ok    ${test.fullTitle()}`));
        runner.on('fail', (test, error) => lines.push(`  FAIL  ${test.fullTitle()}\n${String(error?.stack ?? error)}`));
    });
}
