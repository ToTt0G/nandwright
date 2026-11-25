import * as vscode from 'vscode';

export function followsEditor(): boolean {
    return vscode.workspace.getConfiguration('nandwright.simulator').get<boolean>('followEditor', true);
}
