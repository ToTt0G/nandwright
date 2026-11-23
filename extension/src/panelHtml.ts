// The HTML shell and the shared look of the extension's panels.

import * as vscode from 'vscode';

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

// Everything is drawn with VS Code's own theme variables, so the panels are
// right in light, dark and high-contrast themes without knowing which is on.
const BASE_STYLE = `
:root { --gap: 12px; --line: var(--vscode-widget-border, var(--vscode-contrastBorder, var(--vscode-panel-border, transparent))); }
* { box-sizing: border-box; }
body { margin: 0; padding: 0 20px 28px; color: var(--vscode-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); line-height: 1.45; }
main { max-width: 640px; }
header { display: flex; align-items: baseline; flex-wrap: wrap; gap: 10px; padding: 18px 0 10px; }
h1 { margin: 0; font-size: 1.5em; font-weight: 600; font-family: var(--vscode-editor-font-family); letter-spacing: -0.01em; }
h2 { margin: 22px 0 6px; font-size: 0.8em; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--vscode-descriptionForeground); }
.badge { padding: 1px 8px; border-radius: 10px; font-size: 0.85em; color: var(--vscode-badge-foreground); background: var(--vscode-badge-background); border: 1px solid var(--vscode-contrastBorder, transparent); }
.time { margin-left: auto; font-family: var(--vscode-editor-font-family); color: var(--vscode-descriptionForeground); font-variant-numeric: tabular-nums; }
.hint { margin: 2px 0 8px; color: var(--vscode-descriptionForeground); }
.toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding-bottom: 4px; }
.spacer { flex: 1 1 auto; }
button { font: inherit; color: inherit; cursor: pointer; }
button:focus-visible, input:focus-visible, tr:focus-visible { outline: 1px solid var(--vscode-focusBorder); outline-offset: 1px; }
.primary, .secondary { padding: 4px 12px; border-radius: 2px; border: 1px solid var(--vscode-button-border, var(--vscode-contrastBorder, transparent)); }
.primary { color: var(--vscode-button-foreground); background: var(--vscode-button-background); }
.primary:hover { background: var(--vscode-button-hoverBackground); }
.secondary { color: var(--vscode-button-secondaryForeground); background: var(--vscode-button-secondaryBackground); }
.secondary:hover { background: var(--vscode-button-secondaryHoverBackground); }
.segmented { display: inline-flex; border: 1px solid var(--line); border-radius: 3px; overflow: hidden; }
.segmented button { padding: 3px 10px; border: 0; background: transparent; color: var(--vscode-descriptionForeground); }
.segmented button + button { border-left: 1px solid var(--line); }
.segmented button:hover { color: var(--vscode-foreground); background: var(--vscode-toolbar-hoverBackground); }
.segmented .selected { color: var(--vscode-list-activeSelectionForeground, var(--vscode-foreground)); background: var(--vscode-list-activeSelectionBackground); }
.value, .bus, .bit, .truth { font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; }
.value { white-space: pre-wrap; overflow-wrap: anywhere; }
.rejected { color: var(--vscode-errorForeground); font-size: 0.9em; }
.rejected:empty { display: none; }
.problem { margin-top: 16px; padding: 10px 12px; border: 1px solid var(--vscode-inputValidation-errorBorder, var(--vscode-errorForeground)); background: var(--vscode-inputValidation-errorBackground, transparent); border-radius: 3px; }
.problem pre { margin: 6px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); }
select { font: inherit; padding: 3px 6px; color: var(--vscode-dropdown-foreground); background: var(--vscode-dropdown-background); border: 1px solid var(--vscode-dropdown-border, var(--vscode-contrastBorder, transparent)); border-radius: 2px; }
button:disabled, input:disabled { opacity: 0.5; cursor: default; }
.muted { color: var(--vscode-descriptionForeground); }
`;
