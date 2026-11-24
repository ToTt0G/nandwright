// When to offer the setup: once on first install, once for someone updating
// to a version that has it, and after "Not Now" once more after the next
// update. No VS Code here, so it can be unit tested.

/** What was answered, kept in the global state. */
export interface OfferState {
    answer?: 'setUp' | 'notNow' | 'never';
    /** How many times "Not Now" was chosen. */
    notNow?: number;
    /** The extension's version when the offer was last answered. */
    version?: string;
}

export const OFFER_KEY = 'nandwright.setupOffer';

export function shouldOffer(state: OfferState | undefined, version: string): boolean {
    if (!state?.answer) {
        return true;
    }
    if (state.answer !== 'notNow') {
        return false;
    }
    return (state.notNow ?? 1) < 2 && state.version !== version;
}

export function afterAnswer(state: OfferState | undefined, answer: OfferState['answer'], version: string): OfferState {
    return {
        answer,
        notNow: (state?.notNow ?? 0) + (answer === 'notNow' ? 1 : 0),
        version,
    };
}
