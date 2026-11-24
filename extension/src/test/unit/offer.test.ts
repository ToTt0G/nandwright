// When the setup is offered.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { afterAnswer, shouldOffer } from '../../offer';

test('offered once to someone who never answered, on install or update', () => {
    assert.equal(shouldOffer(undefined, '0.4.0'), true);
    assert.equal(shouldOffer({}, '0.4.0'), true);
});

test('never again after Set Up or Don\'t Ask Again', () => {
    assert.equal(shouldOffer(afterAnswer(undefined, 'setUp', '0.4.0'), '0.5.0'), false);
    assert.equal(shouldOffer(afterAnswer(undefined, 'never', '0.4.0'), '0.5.0'), false);
});

test('Not Now: once more after the next update, then never', () => {
    const first = afterAnswer(undefined, 'notNow', '0.4.0');
    assert.equal(shouldOffer(first, '0.4.0'), false);
    assert.equal(shouldOffer(first, '0.4.1'), true);
    const second = afterAnswer(first, 'notNow', '0.4.1');
    assert.equal(shouldOffer(second, '0.4.1'), false);
    assert.equal(shouldOffer(second, '0.5.0'), false);
});
