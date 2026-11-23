// The language-server process.
//
// VS Code forks this file as a Node process and talks LSP to it over IPC.
// Every message goes to the engine, which answers with zero or more messages.
//
// The engine must never take the editor's language features down with it. If
// it traps, this process makes a fresh engine, replays the handshake and the
// open documents, and carries on. The document being handled when it trapped
// is left out of the replay until the student edits it again, so one bad file
// cannot cause a crash loop. The edit opens it in the engine anew, with the
// text as it then is.

import * as path from 'node:path';
import { IPCMessageReader, IPCMessageWriter, Message } from 'vscode-jsonrpc/node';
import { EngineCrash, WasmEngine } from '../engine/wasm';

const wasmPath = path.join(__dirname, 'engine.wasm');
const reader = new IPCMessageReader(process);
const writer = new IPCMessageWriter(process);

interface OpenDocument {
    languageId: string;
    version: number;
    text: string;
}

let engine = newEngine();
let initialize: any | undefined;
const documents = new Map<string, OpenDocument>();
const quarantined = new Set<string>();

function newEngine(): WasmEngine {
    return new WasmEngine(wasmPath, (line) => console.error(line));
}

function send(messages: string[]): void {
    for (const message of messages) {
        void writer.write(JSON.parse(message) as Message);
    }
}

function feed(message: unknown): string[] {
    return engine.call<{ messages: string[] }>({ op: 'lsp', message: JSON.stringify(message) }).messages ?? [];
}

function track(message: any): void {
    const params = message.params;
    switch (message.method) {
        case 'initialize':
            initialize = message;
            break;
        case 'textDocument/didOpen':
            documents.set(params.textDocument.uri, {
                languageId: params.textDocument.languageId,
                version: params.textDocument.version,
                text: params.textDocument.text,
            });
            quarantined.delete(params.textDocument.uri);
            break;
        case 'textDocument/didChange': {
            const document = documents.get(params.textDocument.uri);
            const last = params.contentChanges?.[params.contentChanges.length - 1];
            if (document && last && typeof last.text === 'string') {
                document.version = params.textDocument.version;
                document.text = last.text;
            }
            quarantined.delete(params.textDocument.uri);
            break;
        }
        case 'textDocument/didClose':
            documents.delete(params.textDocument.uri);
            quarantined.delete(params.textDocument.uri);
            break;
    }
}

function documentOf(message: any): string | undefined {
    return message?.params?.textDocument?.uri;
}

/** A document the engine no longer knows keeps no problems on show. */
function clearDiagnostics(uri: string): void {
    void writer.write({
        jsonrpc: '2.0',
        method: 'textDocument/publishDiagnostics',
        params: { uri, diagnostics: [] },
    } as unknown as Message);
}

function didOpen(uri: string, document: OpenDocument): unknown {
    return { jsonrpc: '2.0', method: 'textDocument/didOpen', params: { textDocument: { uri, ...document } } };
}

/** Builds a fresh engine and brings it to the state the client expects. */
function recover(culprit: string | undefined): void {
    if (culprit) {
        quarantined.add(culprit);
    }
    for (let attempt = 0; attempt < 2; attempt++) {
        engine = newEngine();
        try {
            if (initialize) {
                feed(initialize);
                feed({ jsonrpc: '2.0', method: 'initialized', params: {} });
            }
            for (const [uri, document] of documents) {
                if (!quarantined.has(uri)) {
                    send(feed(didOpen(uri, document)));
                }
            }
            break;
        } catch (error) {
            if (!(error instanceof EngineCrash)) {
                throw error;
            }
            // Replaying crashed too: start again with no documents at all.
            for (const uri of documents.keys()) {
                quarantined.add(uri);
            }
        }
    }
    for (const uri of quarantined) {
        clearDiagnostics(uri);
    }
}

reader.listen((message: any) => {
    const uri = documentOf(message);
    // Asked before `track`, which lifts the quarantine on an edit.
    const unknownToEngine = uri !== undefined && quarantined.has(uri);
    track(message);
    if (message.method === 'exit') {
        process.exit(0);
    }
    if (unknownToEngine && message.method === 'textDocument/didChange') {
        // The student has edited the file the engine stopped on. The engine
        // has never been told about it since, so a change means nothing to
        // it: open the document there, with the text as it is now.
        const document = documents.get(uri);
        if (!document) {
            return;
        }
        message = didOpen(uri, document);
    } else if (unknownToEngine && message.method !== 'textDocument/didOpen') {
        if (message.id !== undefined && message.method) {
            void writer.write({ jsonrpc: '2.0', id: message.id, result: null } as Message);
        }
        return;
    }
    try {
        send(feed(message));
    } catch (error) {
        if (!(error instanceof EngineCrash)) {
            throw error;
        }
        console.error(`Nandwright: the language engine stopped (${error.message}) and was restarted.`);
        recover(uri);
        if (message.id !== undefined && message.method) {
            void writer.write({
                jsonrpc: '2.0',
                id: message.id,
                error: { code: -32603, message: 'The language engine was restarted. Try again.' },
            } as unknown as Message);
        }
    }
});

process.on('uncaughtException', (error) => {
    console.error(`Nandwright language server: ${error.stack ?? error}`);
});
process.on('unhandledRejection', (error) => {
    console.error(`Nandwright language server: ${error}`);
});
