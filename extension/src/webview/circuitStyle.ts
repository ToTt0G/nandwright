// The look of the circuit diagram panel: passed to `panelHtml` by the panel,
// and used by the preview harness. Theme variables only, so light, dark and
// high-contrast themes all work; the export bakes the computed colours in.

export const CIRCUIT_STYLE = `
html, body { height: 100%; }
body { padding: 0; overflow: hidden; background: var(--vscode-editor-background); }
main.circuit { max-width: none; height: 100vh; display: flex; flex-direction: column; }
.circuit {
  --c-bg: var(--vscode-editor-background);
  --c-fg: var(--vscode-editor-foreground, var(--vscode-foreground));
  --c-muted: var(--vscode-descriptionForeground);
  --c-hi: var(--vscode-charts-green, #3fb950);
  --c-lo: color-mix(in srgb, var(--c-fg) 30%, var(--c-bg));
  --c-bus: var(--vscode-charts-blue, #3794ff);
  --c-bus-zero: color-mix(in srgb, var(--c-bus) 42%, var(--c-bg));
  --c-body: color-mix(in srgb, var(--c-fg) 4%, var(--c-bg));
  --c-body-gate: color-mix(in srgb, var(--c-fg) 7%, var(--c-bg));
  --c-stroke: color-mix(in srgb, var(--c-fg) 62%, var(--c-bg));
  --c-grid: color-mix(in srgb, var(--c-fg) 13%, var(--c-bg));
  --c-accent: var(--vscode-focusBorder, #0078d4);
  --c-hl: var(--vscode-charts-yellow, #e2c541);
  --c-port: color-mix(in srgb, var(--c-fg) 9%, var(--c-bg));
  --c-error: var(--vscode-errorForeground, #f14c4c);
}
body.vscode-light .circuit { --c-hi: var(--vscode-charts-green, #388a34); --c-lo: color-mix(in srgb, var(--c-fg) 26%, var(--c-bg)); --c-grid: color-mix(in srgb, var(--c-fg) 16%, var(--c-bg)); }
body.vscode-high-contrast .circuit, body.vscode-high-contrast-light .circuit {
  --c-lo: color-mix(in srgb, var(--c-fg) 55%, var(--c-bg));
  --c-bus-zero: color-mix(in srgb, var(--c-bus) 70%, var(--c-bg));
  --c-body: var(--c-bg); --c-body-gate: var(--c-bg);
  --c-stroke: var(--vscode-contrastBorder, var(--c-fg));
  --c-grid: transparent;
}

/* Toolbar */
.c-bar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; padding: 8px 12px; border-bottom: 1px solid var(--line); background: var(--vscode-editor-background); position: relative; z-index: 2; }
.c-crumbs { display: flex; align-items: center; flex-wrap: wrap; gap: 2px; min-width: 0; font-family: var(--vscode-editor-font-family); font-size: 1.05em; }
.c-crumbs button { padding: 2px 6px; border: 0; border-radius: 3px; background: none; color: var(--vscode-textLink-foreground); }
.c-crumbs button:hover { background: var(--vscode-toolbar-hoverBackground); color: var(--vscode-textLink-activeForeground); }
.c-crumbs .here { padding: 2px 6px; font-weight: 600; }
.c-crumbs .sep { color: var(--c-muted); opacity: .7; }
.c-crumbs .part-no { color: var(--c-muted); font-weight: 400; font-family: var(--vscode-font-family); font-size: .85em; margin-left: 4px; }
.c-group { display: inline-flex; align-items: center; gap: 4px; }
.c-icon { display: inline-flex; align-items: center; justify-content: center; gap: 5px; height: 26px; min-width: 26px; padding: 0 7px; border-radius: 4px; border: 1px solid transparent; background: transparent; color: var(--vscode-foreground); }
.c-icon:hover { background: var(--vscode-toolbar-hoverBackground); }
.c-icon[aria-pressed="true"] { color: var(--vscode-inputOption-activeForeground, var(--vscode-foreground)); background: var(--vscode-inputOption-activeBackground, var(--vscode-toolbar-activeBackground)); border-color: var(--vscode-inputOption-activeBorder, transparent); }
.c-icon svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round; }
.c-zoom { min-width: 3.6em; text-align: center; color: var(--c-muted); font-variant-numeric: tabular-nums; font-size: .9em; }
.c-time { min-width: 3.5em; padding: 2px 8px; border-radius: 10px; text-align: center; font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; color: var(--vscode-badge-foreground); background: var(--vscode-badge-background); }
.c-clock { padding-right: 10px; margin-right: 2px; border-right: 1px solid var(--line); }
.c-clock[hidden] { display: none; }
.c-menu { position: absolute; right: 12px; top: calc(100% - 4px); display: flex; flex-direction: column; min-width: 150px; padding: 4px; border-radius: 5px; background: var(--vscode-menu-background, var(--vscode-editorWidget-background)); color: var(--vscode-menu-foreground, var(--vscode-foreground)); border: 1px solid var(--vscode-menu-border, var(--vscode-widget-border, var(--line))); box-shadow: 0 4px 16px var(--vscode-widget-shadow, rgba(0,0,0,.3)); }
.c-menu[hidden] { display: none; }
.c-menu button { text-align: left; padding: 5px 10px; border: 0; border-radius: 3px; background: none; }
.c-menu button:hover, .c-menu button:focus-visible { color: var(--vscode-menu-selectionForeground, var(--vscode-foreground)); background: var(--vscode-menu-selectionBackground, var(--vscode-list-activeSelectionBackground)); }

/* Stage */
.c-stage { position: relative; flex: 1 1 auto; min-height: 0; overflow: hidden; outline: none; cursor: grab; touch-action: none; user-select: none; -webkit-user-select: none; }
.c-stage.panning { cursor: grabbing; }
.c-stage:focus-visible { box-shadow: inset 0 0 0 1px var(--c-accent); }
.c-stage > svg { display: block; width: 100%; height: 100%; }
.c-stage svg.stale { filter: grayscale(1); opacity: .35; transition: opacity .2s; }
.c-hint { position: absolute; left: 12px; bottom: 8px; color: var(--c-muted); font-size: .85em; pointer-events: none; opacity: .8; }
.c-note { position: absolute; left: 16px; right: 16px; bottom: 32px; margin-inline: auto; width: fit-content; max-width: 560px; padding: 6px 12px; border-radius: 4px; border: 1px solid var(--vscode-inputValidation-infoBorder, var(--vscode-widget-border, var(--line))); background: var(--vscode-inputValidation-infoBackground, var(--vscode-editorWidget-background)); color: var(--vscode-foreground); box-shadow: 0 2px 10px var(--vscode-widget-shadow, rgba(0,0,0,.25)); font-size: .92em; pointer-events: none; z-index: 4; }
.c-note[hidden] { display: none; }
.c-problem { position: absolute; left: 16px; right: 16px; top: 16px; margin-inline: auto; width: fit-content; max-width: 680px; padding: 10px 14px; border-radius: 4px; border: 1px solid var(--vscode-inputValidation-errorBorder, var(--c-error)); background: var(--vscode-inputValidation-errorBackground, var(--vscode-editorWidget-background)); box-shadow: 0 4px 18px var(--vscode-widget-shadow, rgba(0,0,0,.3)); z-index: 3; }
.c-problem[hidden] { display: none; }
.c-too-large { position: absolute; left: 16px; right: 16px; top: 40%; transform: translateY(-50%); margin-inline: auto; width: fit-content; max-width: 560px; padding: 12px 16px; border-radius: 4px; border: 1px solid var(--vscode-widget-border, var(--vscode-contrastBorder, var(--c-line, transparent))); background: var(--vscode-editorWidget-background); text-align: center; }
.c-too-large[hidden] { display: none; }
.c-problem strong { display: block; margin-bottom: 4px; }
.c-problem .c-retry { margin-top: 8px; }
.c-problem pre { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); }
.c-tip { position: absolute; z-index: 4; pointer-events: none; padding: 6px 9px; border-radius: 4px; min-width: 120px; max-width: 320px; color: var(--vscode-editorHoverWidget-foreground, var(--vscode-foreground)); background: var(--vscode-editorHoverWidget-background, var(--vscode-editorWidget-background)); border: 1px solid var(--vscode-editorHoverWidget-border, var(--line)); box-shadow: 0 2px 10px var(--vscode-widget-shadow, rgba(0,0,0,.25)); font-size: .92em; }
.c-tip[hidden] { display: none; }
.c-tip .t-head { font-weight: 600; margin-bottom: 3px; font-family: var(--vscode-editor-font-family); }
.c-tip .t-head .muted { font-weight: 400; font-family: var(--vscode-font-family); }
.c-tip table { border-collapse: collapse; font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; }
.c-tip td { padding: 0 10px 0 0; white-space: nowrap; }
.c-tip td:first-child { color: var(--c-muted); }
.c-tip .t-foot { margin-top: 4px; color: var(--c-muted); font-size: .9em; }
.c-edit { position: absolute; z-index: 5; padding: 1px 5px; font-family: var(--vscode-editor-font-family); font-size: 12px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--c-accent); border-radius: 3px; outline: none; }
.c-edit[hidden] { display: none; }
.c-edit-error { position: absolute; z-index: 5; color: var(--c-error); font-size: .85em; background: var(--vscode-editor-background); padding: 1px 4px; border-radius: 2px; }
.c-edit-error:empty { display: none; }

/* Diagram */
.c-svg text { font-family: var(--vscode-editor-font-family, monospace); fill: var(--c-fg); }
.grid-dot { fill: var(--c-grid); }
.wire, .lead { fill: none; stroke: var(--c-lo); stroke-width: 1.75; stroke-linecap: round; stroke-linejoin: round; transition: opacity .15s; }
.wire.hi, .lead.hi { stroke: var(--c-hi); }
.wire.bus, .lead.bus { stroke: var(--c-bus-zero); stroke-width: 3.25; }
.wire.bus.nz, .lead.bus.nz { stroke: var(--c-bus); }
.glow { fill: none; stroke: transparent; stroke-width: 6; stroke-linecap: round; stroke-linejoin: round; opacity: .16; pointer-events: none; }
.glow.hi { stroke: var(--c-hi); }
.glow.bus.nz { stroke: var(--c-bus); stroke-width: 8; opacity: .12; }
.hit { fill: none; stroke: transparent; stroke-width: 10; pointer-events: stroke; cursor: pointer; }
.junction { fill: var(--c-lo); stroke: var(--c-bg); stroke-width: 1.25; }
.junction.hi { fill: var(--c-hi); }
.junction.bus { fill: var(--c-bus-zero); }
.junction.bus.nz { fill: var(--c-bus); }
.mark { stroke: var(--c-muted); stroke-width: 1.25; stroke-linecap: round; }
.c-svg text.mark-text { font-size: 8.5px; fill: var(--c-muted); }
.c-svg text.label { font-size: 10px; paint-order: stroke; stroke: var(--c-bg); stroke-width: 3.5px; stroke-linejoin: round; }
.c-svg text.label.value { fill: var(--c-bus); font-weight: 600; }
.c-svg text.label.value.zero { fill: color-mix(in srgb, var(--c-bus) 65%, var(--c-fg)); font-weight: 400; }
.c-svg text.label.slice { fill: var(--c-muted); }
.body { fill: var(--c-body); stroke: var(--c-stroke); stroke-width: 1.4; stroke-linejoin: round; }
.gate .body, .gate .bubble { fill: var(--c-body-gate); }
.bubble { stroke: var(--c-stroke); stroke-width: 1.4; }
.extra { fill: none; stroke: var(--c-stroke); stroke-width: 1.4; stroke-linecap: round; }
.header-rule { stroke: var(--c-stroke); stroke-opacity: .35; stroke-width: 1; }
.c-svg text.chip-name { font-family: var(--vscode-font-family, sans-serif); font-size: 12px; font-weight: 600; }
.c-svg text.variant { font-size: 9.5px; fill: var(--c-muted); }
.c-svg text.pin { font-size: 10.5px; fill: color-mix(in srgb, var(--c-fg) 82%, var(--c-bg)); }
.c-svg text.pin-small { font-size: 8.5px; fill: var(--c-muted); }
.c-svg tspan.pin-width { fill: var(--c-muted); font-size: 8.5px; }
.clock { fill: none; stroke: var(--c-stroke); stroke-width: 1.3; stroke-linejoin: round; }
.drill-btn rect { fill: var(--c-body); stroke: var(--c-stroke); stroke-width: 1; }
.drill-btn path { fill: none; stroke: var(--c-fg); stroke-width: 1.2; stroke-linecap: round; stroke-linejoin: round; }
.drill-btn { cursor: zoom-in; }
.drill-btn:hover rect { stroke: var(--c-accent); fill: color-mix(in srgb, var(--c-accent) 18%, var(--c-bg)); }
.part { cursor: pointer; outline: none; }
.part.composite { cursor: pointer; }
.part:hover .body, .part:focus-visible .body { stroke: var(--c-accent); }
.part:focus-visible .body { stroke-width: 2.2; }
.part.hl .body { stroke: var(--c-hl); stroke-width: 2.4; }
.part.hl .halo { stroke: var(--c-hl); }
.halo { fill: none; stroke: transparent; stroke-width: 8; stroke-opacity: .25; pointer-events: none; }
.tie { stroke: var(--c-lo); stroke-width: 1.5; fill: none; stroke-linecap: round; }
.tie.hi { stroke: var(--c-hi); }
.tie-box { fill: var(--c-bg); stroke: var(--c-lo); stroke-width: 1.2; }
.tie-box.hi { stroke: var(--c-hi); fill: color-mix(in srgb, var(--c-hi) 14%, var(--c-bg)); }
.c-svg text.tie-text { font-size: 9px; font-weight: 600; fill: var(--c-muted); }
.c-svg text.tie-text.hi { fill: var(--c-hi); }
.port .shape { fill: var(--c-port); stroke: var(--c-stroke); stroke-width: 1.3; stroke-linejoin: round; }
.port.driven .shape { stroke-dasharray: 3 2; }
.c-svg .port text.port-name { font-size: 11px; font-weight: 600; }
.c-svg .port text.port-value { font-size: 10.5px; fill: var(--c-muted); font-variant-numeric: tabular-nums; }
.c-svg .port.bus text.port-value { fill: var(--c-bus); }
.c-svg .port.bus.zero text.port-value { fill: var(--c-muted); }
.port .bit { fill: var(--c-bg); stroke: var(--c-lo); stroke-width: 1.3; }
.port .bit.hi { fill: var(--c-hi); stroke: var(--c-hi); }
.c-svg .port text.bit-text { font-size: 9.5px; font-weight: 700; fill: var(--c-muted); }
.c-svg .port text.bit-text.hi { fill: var(--c-bg); }
.port.editable { cursor: pointer; outline: none; }
.port.editable:hover .shape, .port.editable:focus-visible .shape { stroke: var(--c-accent); stroke-width: 2; }
.port.hl .shape { stroke: var(--c-hl); stroke-width: 2.2; }
.pulse { fill: none; stroke-linecap: round; pointer-events: none; stroke-width: 4; stroke: var(--c-fg); opacity: .95; }
.pulse.hi { stroke: color-mix(in srgb, var(--c-hi) 55%, #fff); }
.pulse.bus { stroke: color-mix(in srgb, var(--c-bus) 55%, #fff); stroke-width: 6; }
body.vscode-light .pulse.hi { stroke: var(--c-hi); }
body.vscode-light .pulse.bus { stroke: var(--c-bus); }
.flash { fill: none; stroke-linecap: round; pointer-events: none; stroke-width: 9; opacity: 0; }
.flash.hi { stroke: var(--c-hi); }
.flash.lo { stroke: var(--c-muted); stroke-width: 6; }
.flash.bus { stroke: var(--c-bus); stroke-width: 11; }
/* Hovering a net: it stands out, the rest steps back. */
.c-svg.focus-net .wire:not(.on), .c-svg.focus-net .junction:not(.on), .c-svg.focus-net .label:not(.on), .c-svg.focus-net .mark:not(.on) { opacity: .28; }
.wire.on { stroke-width: 2.6; }
.wire.bus.on { stroke-width: 4.2; }
.glow.on { stroke: var(--c-accent); opacity: .3; stroke-width: 9; }
.wire.hl { stroke: var(--c-hl); }
.junction.hl { fill: var(--c-hl); }
.glow.hl { stroke: var(--c-hl); opacity: .35; }
`;
