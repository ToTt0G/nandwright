// The HTML shell and the shared look of the extension's panels.

import * as vscode from 'vscode';
import { BASE_STYLE } from './panelStyle';

/**
 * A panel page: no remote content, scripts and styles only with this page's
 * nonce, and the panel's script from `dist/webview`.
 */
export function panelHtml(
    webview: vscode.Webview,
    extensionUri: vscode.Uri,
    script: string,
    title: string,
    style: string,
): string {
    const source = webview.asWebviewUri(vscode.Uri.joinPath(extensionUri, 'dist', 'webview', script));
    const nonce = Array.from({ length: 32 }, () => Math.floor(Math.random() * 36).toString(36)).join('');
    const csp = ["default-src 'none'", `style-src 'nonce-${nonce}'`, `script-src 'nonce-${nonce}'`, 'img-src data:'].join('; ');
    const escaped = title.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
    return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${escaped}</title>
<style nonce="${nonce}">${BASE_STYLE}${style}</style>
</head>
<body>
<main id="root"><p class="hint">Loading…</p></main>
<script nonce="${nonce}" src="${source}"></script>
</body>
</html>`;
}
