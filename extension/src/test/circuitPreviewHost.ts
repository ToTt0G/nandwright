// The extension's side of the circuit panel, faked in a plain browser page for
// scripts/circuit-preview.mjs: a fixture chip simulated by circuitFixtures.ts
// answers the webview's messages as circuitPanel.ts would.

import { FixtureSession, wideChip } from './circuitFixtures';
import { BASE_STYLE } from '../panelStyle';
import { CIRCUIT_STYLE } from '../webview/circuitStyle';
import type { FromCircuit, Settings, ToCircuit, ValuesCause } from '../webview/circuitTypes';

const params = new URLSearchParams(location.search);
const wide = /^Wide(\d+)$/.exec(params.get('chip') ?? '');
const chip = wide ? wideChip(Number(wide[1])) : params.get('chip') ?? 'VoteCheck';
const session = new FixtureSession(chip);
for (const [pin, value] of new URLSearchParams(params.get('set') ?? '').entries()) {
    session.set(pin, value);
}
for (let i = 0; i < Number(params.get('steps') ?? 0); i++) {
    session.step('step');
}
let path: number[] = (params.get('path') ?? '').split('.').filter(Boolean).map(Number);
let settings: Settings = { animate: params.get('animate') !== '0', radix: (params.get('radix') as Settings['radix']) ?? 'dec' };

const style = document.createElement('style');
style.textContent = BASE_STYLE + CIRCUIT_STYLE;
document.head.append(style);

const log: FromCircuit[] = [];
let token = 0;
const post = (message: ToCircuit) => window.postMessage(message, '*');
const values = (cause: ValuesCause) => post({ type: 'values', values: session.values(path), cause });

function handle(message: FromCircuit): void {
    log.push(message);
    switch (message.type) {
        case 'ready':
            post({ type: 'diagram', diagram: session.diagram(path), settings, token: ++token, target: 1 });
            break;
        case 'set':
            session.set(message.pin, message.value);
            values('input');
            break;
        case 'clock':
            session.step(message.action);
            values(message.action === 'reset' ? 'reset' : 'clock');
            break;
        case 'drill':
            path = message.path;
            post({ type: 'diagram', diagram: session.diagram(path), settings, token: ++token, target: 1 });
            break;
        case 'settings':
            settings = { ...settings, ...(message.animate !== undefined ? { animate: message.animate } : {}), ...(message.radix ? { radix: message.radix } : {}) };
            break;
        case 'error':
            console.error(message.message);
            break;
    }
}

(window as any).acquireVsCodeApi = () => ({
    postMessage: (message: FromCircuit) => setTimeout(() => handle(message)),
    getState: () => undefined,
    setState: () => undefined,
});
(window as any).preview = { post, session, log, values, chip };
