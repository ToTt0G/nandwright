// The circuit diagram panel (runs inside a VS Code webview).
//
// It draws one level of a chip as a live schematic: the layout comes from
// circuitLayout.ts, the values from the extension, which owns the
// simulation. This file draws, animates and reports what the student does.

import type { Bits, Diagram, DiagramCommand, FromCircuit, Settings, ToCircuit, Unbound, Values, ValuesCause, BitRange } from './circuitTypes';
import { gatePaths, leadEnd, LEAD } from './circuitSymbols';
import { isPngBase64, pngSize } from './circuitExport';
import { Layout, layoutDiagram, maxOf, minOf, MAX_DRAWN_PARTS, valuesFit, PartGeom, PinGeom, Point, PortGeom, range, WireGeom } from './circuitLayout';
import { Radix, formatValue, toBigInt, typedValue } from '../values';

declare function acquireVsCodeApi(): {
    postMessage(message: unknown): void;
    getState(): any;
    setState(state: any): void;
};

const vscode = acquireVsCodeApi();
const send = (message: FromCircuit) => vscode.postMessage(message);
/** The token of the diagram shown: commands carry it, so a late one is not applied to another diagram. */
let shownToken = 0;
/** The file-target token of the chip shown: clock/reset carry it across rebuilds, dropped on chip switch. */
let shownTarget = 0;
const command = (message: Unbound<DiagramCommand>) => send({ ...message, diagram: shownToken } as DiagramCommand);

const SVG = 'http://www.w3.org/2000/svg';
/** Milliseconds per half-layer of the propagation animation (about 90 ms a layer). */
const HALF_LAYER_MS = 45;

// ---- state ------------------------------------------------------------------

let diagram: Diagram | null = null;
let values: Values | null = null;
let layout: Layout | null = null;
let settings: Settings = { animate: true, radix: 'dec' };
let problem: string | null = null;
let highlight: { part?: number; net?: number } = {};
let view = { k: 1, x: 0, y: 0 };
/** True while the view is as "Fit" left it, so a resize fits again. */
let fitted = true;
let shownPath = '';

// Elements of the drawing, by what they show.
interface Drawn {
    svg: SVGSVGElement;
    viewport: SVGGElement;
    pulses: SVGGElement;
    wires: { geom: WireGeom; el: SVGPathElement; glow: SVGPathElement; hit: SVGPathElement; state: string }[];
    byNet: Map<number, Element[]>;
    junctions: { net: number; bits: BitRange | null; width: number; el: SVGCircleElement; t: number }[];
    labels: { net: number; bits: BitRange | null; width: number; prefix: string; el: SVGTextElement; kind: string }[];
    ports: { geom: PortGeom; el: SVGGElement; value: SVGTextElement | null; bit: SVGCircleElement | null; bitText: SVGTextElement | null }[];
    parts: Map<number, SVGGElement>;
    leads: { part: number; side: 'in' | 'out'; pin: number; bits: BitRange | null; width: number; el: SVGElement }[];
}
let drawn: Drawn | null = null;

// ---- small DOM helpers ------------------------------------------------------

function s<K extends keyof SVGElementTagNameMap>(tag: K, attributes: Record<string, string | number> = {}, ...children: (Node | string)[]): SVGElementTagNameMap[K] {
    const node = document.createElementNS(SVG, tag);
    for (const [name, value] of Object.entries(attributes)) {
        node.setAttribute(name, String(value));
    }
    node.append(...children);
    return node;
}

function h<K extends keyof HTMLElementTagNameMap>(tag: K, attributes: Record<string, string> = {}, ...children: (Node | string)[]): HTMLElementTagNameMap[K] {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes)) {
        if (name === 'class') {
            node.className = value;
        } else {
            node.setAttribute(name, value);
        }
    }
    node.append(...children);
    return node;
}

function icon(paths: string): SVGSVGElement {
    const svg = s('svg', { viewBox: '0 0 16 16', 'aria-hidden': 'true' });
    for (const d of paths.split('|')) {
        svg.append(s('path', { d }));
    }
    return svg;
}

const ICONS = {
    fit: 'M2 6V2h4|M10 2h4v4|M14 10v4h-4|M6 14H2v-4|M5.5 5.5h5v5h-5z',
    plus: 'M8 3.5v9|M3.5 8h9',
    minus: 'M3.5 8h9',
    export: 'M8 2v8|M5 7l3 3 3-3|M3 11v2.5h10V11',
    pulse: 'M1.5 8h3l1.5-4 3 8 1.5-4h4',
    tick: 'M3 11V5h4v6h4V5h2',
    step: 'M2 11V5h3v6h3V5h3v6h3',
    reset: 'M3.5 8a4.5 4.5 0 1 0 1.4-3.3|M3.5 2.5v3h3',
};

const round = (n: number) => Math.round(n * 10) / 10;
const pathData = (points: Point[]) => points.map((p, i) => `${i ? 'L' : 'M'}${round(p.x)},${round(p.y)}`).join(' ');

function sliceValue(bits: Bits, slice: BitRange | null): bigint {
    const value = toBigInt(bits);
    if (!slice) {
        return value;
    }
    return (value >> BigInt(slice[0])) & ((1n << BigInt(slice[1])) - 1n);
}

function reducedMotion(): boolean {
    return !!settings.reducedMotion || window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

// ---- the page ------------------------------------------------------------------

const root = document.getElementById('root')!;
root.className = 'circuit';
root.textContent = '';

const crumbs = h('nav', { class: 'c-crumbs', 'aria-label': 'Chip level' });
const clockGroup = h('div', { class: 'c-group c-clock', role: 'group', 'aria-label': 'Clock' });
const timeLabel = h('span', { class: 'c-time', title: 'Clock time', 'aria-live': 'polite' }, '0');
const radixGroup = h('div', { class: 'segmented', role: 'group', 'aria-label': 'Number format' });
const animateButton = h('button', { class: 'c-icon', type: 'button', title: 'Animate changes as they spread through the circuit', 'aria-pressed': 'true' });
const zoomLabel = h('span', { class: 'c-zoom', 'aria-live': 'polite' }, '100%');
const exportMenu = h('div', { class: 'c-menu', role: 'menu', hidden: '' });
const stage = h('div', { class: 'c-stage', tabindex: '0', role: 'application', 'aria-label': 'Circuit diagram. Arrows pan, plus and minus zoom, 0 fits, Tab moves between parts, Enter opens a part.' });
const tip = h('div', { class: 'c-tip', hidden: '', role: 'tooltip' });
const problemBox = h('div', { class: 'c-problem', hidden: '', role: 'alert' });
const editor = h('input', { class: 'c-edit', hidden: '', type: 'text', spellcheck: 'false', autocomplete: 'off' });
const editError = h('div', { class: 'c-edit-error', 'aria-live': 'polite' });
const noteBox = h('div', { class: 'c-note', hidden: '', role: 'status', 'aria-live': 'polite' });
const hint = h('div', { class: 'c-hint' });
const tooLarge = h('div', { class: 'c-too-large', hidden: '', role: 'status' });

function button(label: string, title: string, paths: string | null, onClick: () => void, text = true): HTMLButtonElement {
    const b = h('button', { class: 'c-icon', type: 'button', title, 'aria-label': title });
    if (paths) {
        b.append(icon(paths));
    }
    if (text && label) {
        b.append(label);
    }
    b.addEventListener('click', onClick);
    return b;
}

{
    clockGroup.append(
        button('Tick', 'Tick: the clock goes up (time 0 → 0+)', ICONS.tick, () => send({ type: 'clock', action: 'tick', target: shownTarget })),
        button('Tock', 'Tock: the clock goes down (time 0+ → 1)', null, () => send({ type: 'clock', action: 'tock', target: shownTarget })),
        button('Step', 'Step: a tick and a tock', ICONS.step, () => send({ type: 'clock', action: 'step', target: shownTarget })),
        timeLabel,
        button('', 'Reset the clock and the chip', ICONS.reset, () => send({ type: 'clock', action: 'reset', target: shownTarget }), false),
    );
    clockGroup.hidden = true;
    for (const radix of ['dec', 'bin', 'hex'] as Radix[]) {
        const b = h('button', { type: 'button', 'data-radix': radix, title: { dec: 'Decimal', bin: 'Binary', hex: 'Hexadecimal' }[radix] }, radix);
        b.addEventListener('click', () => {
            if (settings.radix !== radix) {
                settings = { ...settings, radix };
                send({ type: 'settings', radix });
                updateToolbar();
                redraw(false);
            }
        });
        radixGroup.append(b);
    }
    animateButton.append(icon(ICONS.pulse), 'Animate');
    animateButton.addEventListener('click', () => {
        settings = { ...settings, animate: !settings.animate };
        send({ type: 'settings', animate: settings.animate });
        updateToolbar();
    });
    const exportButton = button('Export…', 'Export this level as SVG or PNG', ICONS.export, () => {
        exportMenu.hidden = !exportMenu.hidden;
        if (!exportMenu.hidden) {
            (exportMenu.firstElementChild as HTMLElement | null)?.focus();
        }
    });
    exportButton.setAttribute('aria-haspopup', 'menu');
    const svgItem = h('button', { type: 'button', role: 'menuitem' }, 'SVG (vector)…');
    svgItem.addEventListener('click', () => {
        exportMenu.hidden = true;
        exportAs('svg');
    });
    const pngItem = h('button', { type: 'button', role: 'menuitem' }, 'PNG (2× image)…');
    pngItem.addEventListener('click', () => {
        exportMenu.hidden = true;
        exportAs('png');
    });
    exportMenu.append(svgItem, pngItem);
    exportMenu.addEventListener('keydown', (event) => {
        if (event.key === 'Escape') {
            exportMenu.hidden = true;
            exportButton.focus();
        }
    });
    const bar = h(
        'header',
        { class: 'c-bar' },
        crumbs,
        h('span', { class: 'spacer' }),
        clockGroup,
        radixGroup,
        animateButton,
        h(
            'div',
            { class: 'c-group', role: 'group', 'aria-label': 'Zoom' },
            button('', 'Zoom out (-)', ICONS.minus, () => zoomBy(1 / 1.25), false),
            zoomLabel,
            button('', 'Zoom in (+)', ICONS.plus, () => zoomBy(1.25), false),
            button('Fit', 'Fit the diagram to the panel (0)', ICONS.fit, () => fit()),
        ),
        exportButton,
        exportMenu,
    );
    stage.append(tip, problemBox, tooLarge, editor, editError, noteBox, hint);
    root.append(bar, stage);
    document.addEventListener('pointerdown', (event) => {
        if (!exportMenu.hidden && !exportMenu.contains(event.target as Node) && !exportButton.contains(event.target as Node)) {
            exportMenu.hidden = true;
        }
    });
}

function updateToolbar(): void {
    for (const b of radixGroup.querySelectorAll('button')) {
        const on = b.getAttribute('data-radix') === settings.radix;
        b.classList.toggle('selected', on);
        b.setAttribute('aria-pressed', String(on));
    }
    animateButton.setAttribute('aria-pressed', String(settings.animate));
    clockGroup.hidden = !values?.clocked;
    timeLabel.textContent = values?.time || '0';
    zoomLabel.textContent = `${Math.round(view.k * 100)}%`;
}

function updateCrumbs(): void {
    crumbs.textContent = '';
    if (!diagram) {
        return;
    }
    const list = diagram.breadcrumb.length ? diagram.breadcrumb : [{ chip: diagram.chip, path: diagram.path }];
    list.forEach((crumb, i) => {
        if (i > 0) {
            crumbs.append(h('span', { class: 'sep', 'aria-hidden': 'true' }, '›'));
        }
        const label: (Node | string)[] = [crumb.chip];
        if (crumb.path.length) {
            label.push(h('span', { class: 'part-no' }, `part ${crumb.path[crumb.path.length - 1] + 1}`));
        }
        if (i === list.length - 1) {
            crumbs.append(h('span', { class: 'here', 'aria-current': 'page' }, ...label));
        } else {
            const b = h('button', { type: 'button', title: `Back to ${crumb.chip}` }, ...label);
            b.addEventListener('click', () => command({ type: 'drill', path: crumb.path }));
            crumbs.append(b);
        }
    });
}

// ---- drawing ----------------------------------------------------------------

function redraw(refit: boolean): void {
    finishAnimation();
    hideTip();
    closeEditor(false);
    if (!diagram) {
        return;
    }
    if (diagram.parts.length > MAX_DRAWN_PARTS) {
        // Laying out and drawing this many parts would freeze the panel (and
        // used to overflow the stack): say so instead.
        layout = null;
        drawn?.svg.remove();
        drawn = null;
        hint.textContent = '';
        tooLarge.hidden = false;
        tooLarge.textContent = `${diagram.chip} has ${diagram.parts.length.toLocaleString('en')} parts at this level: too large to draw (the limit is ${MAX_DRAWN_PARTS.toLocaleString('en')}). Open the chips it is made of separately.`;
        return;
    }
    tooLarge.hidden = true;
    const started = performance.now();
    layout = layoutDiagram(diagram, { radix: settings.radix });
    const old = drawn?.svg;
    drawn = render(diagram, layout);
    if (old) {
        old.replaceWith(drawn.svg);
    } else {
        stage.prepend(drawn.svg);
    }
    drawn.svg.classList.toggle('stale', problem !== null);
    applyValues(null);
    applyHighlight();
    if (refit) {
        fit();
    } else {
        applyView();
    }
    const top = diagram.path.length === 0;
    hint.textContent = diagram.parts.length === 0 ? 'This chip has no parts.' : top ? 'Click an input to change it · double-click a part to open it' : 'Inputs here are driven by the chip above';
    void started;
}

function render(d: Diagram, l: Layout): Drawn {
    const svg = s('svg', { class: 'c-svg', role: 'img', 'aria-label': `Circuit of ${d.chip}` });
    const defs = s('defs');
    const pattern = s('pattern', { id: 'c-dots', width: 18, height: 18, patternUnits: 'userSpaceOnUse' }, s('circle', { class: 'grid-dot', cx: 9, cy: 9, r: 0.9 }));
    defs.append(pattern);
    svg.append(defs);
    const viewport = s('g', { class: 'viewport' });
    svg.append(viewport);
    viewport.append(s('rect', { class: 'grid', x: -4000, y: -4000, width: l.width + 8000, height: l.height + 8000, fill: 'url(#c-dots)' }));
    const gGlow = s('g', { class: 'g-glow' });
    const gWires = s('g', { class: 'g-wires' });
    const gHits = s('g', { class: 'g-hits' });
    const gJunctions = s('g', { class: 'g-junctions' });
    const gTies = s('g', { class: 'g-ties' });
    const gParts = s('g', { class: 'g-parts' });
    const gPorts = s('g', { class: 'g-ports' });
    const gLabels = s('g', { class: 'g-labels' });
    const pulses = s('g', { class: 'g-pulses' });
    viewport.append(gGlow, gWires, gJunctions, gTies, gParts, gPorts, gLabels, pulses, gHits);

    const result: Drawn = { svg, viewport, pulses, wires: [], byNet: new Map(), junctions: [], labels: [], ports: [], parts: new Map(), leads: [] };
    const addNet = (net: number, el: Element) => {
        let list = result.byNet.get(net);
        if (!list) {
            result.byNet.set(net, (list = []));
        }
        list.push(el);
    };

    for (const geom of l.wires) {
        const dPath = pathData(geom.points);
        const bus = geom.width > 1 ? ' bus' : '';
        const glow = s('path', { class: `glow${bus}`, d: dPath });
        const el = s('path', { class: `wire${bus}`, d: dPath });
        const hit = s('path', { class: 'hit', d: dPath, 'data-net': geom.net });
        if (geom.bits) {
            hit.setAttribute('data-bits', `${geom.bits[0]},${geom.bits[1]}`);
        }
        gGlow.append(glow);
        gWires.append(el);
        gHits.append(hit);
        addNet(geom.net, el);
        addNet(geom.net, glow);
        result.wires.push({ geom, el, glow, hit, state: '' });
    }
    for (const j of l.junctions) {
        const el = s('circle', { class: `junction${j.width > 1 ? ' bus' : ''}`, cx: round(j.x), cy: round(j.y), r: j.width > 1 ? 3.6 : 2.9 });
        gJunctions.append(el);
        addNet(j.net, el);
        result.junctions.push({ net: j.net, bits: j.bits, width: j.width, el, t: j.t });
    }
    for (const mark of l.marks) {
        const g = s('g', { class: 'mark-g' });
        const line = s('line', { class: 'mark', x1: mark.x - 3.5, y1: mark.y + 5, x2: mark.x + 3.5, y2: mark.y - 5 });
        g.append(line, s('text', { class: 'mark-text', x: mark.x + 4, y: mark.y - 4.5 }, String(mark.width)));
        gLabels.append(g);
        addNet(mark.net, line);
    }
    for (const label of l.labels) {
        const el = s('text', { class: `label ${label.kind}`, x: round(label.x), y: round(label.y), 'text-anchor': label.anchor }, label.text);
        gLabels.append(el);
        addNet(label.net, el);
        result.labels.push({ net: label.net, bits: label.bits, width: label.width, prefix: label.text, el, kind: label.kind });
    }
    for (const tie of l.ties) {
        const g = s('g', { class: 'tie-g' });
        const text = tie.width > 1 ? (tie.value ? 'true' : 'false') : tie.value ? '1' : '0';
        const boxW = text.length * 5.6 + 7;
        const x1 = tie.x - 7;
        const x0 = x1 - boxW;
        const cls = tie.value ? ' hi' : '';
        g.append(
            s('line', { class: `tie${cls}`, x1, y1: tie.y, x2: tie.x, y2: tie.y }),
            s('rect', { class: `tie-box${cls}`, x: round(x0), y: tie.y - 6.5, width: round(boxW), height: 13, rx: 3 }),
            s('text', { class: `tie-text${cls}`, x: round(x0 + boxW / 2), y: tie.y + 3.2, 'text-anchor': 'middle' }, text),
        );
        g.append(s('title', {}, `${d.parts[tie.part]?.inputs[tie.pin]?.name ?? 'pin'} = ${tie.value ? 'true' : 'false'}`));
        gTies.append(g);
    }
    const editable = d.path.length === 0;
    const partById = new Map(l.parts.map((p) => [p.index, p]));
    for (const index of l.tabOrder) {
        const geom = partById.get(index)!;
        const g = drawPart(d, geom, result);
        gParts.append(g);
        result.parts.set(index, g);
    }
    for (const port of [...l.inputs, ...l.outputs]) {
        const drawnPort = drawPort(port, editable && port.side === 'in');
        gPorts.append(drawnPort.el);
        result.ports.push(drawnPort);
        if (port.net >= 0) {
            addNet(port.net, drawnPort.el);
        }
    }
    return result;
}

function drawPart(d: Diagram, geom: PartGeom, out: Drawn): SVGGElement {
    const part = d.parts[geom.index];
    const gate = geom.shape !== 'box';
    const g = s('g', {
        class: `part ${gate ? 'gate' : 'boxed'}${geom.drill ? ' composite' : ''}`,
        transform: `translate(${round(geom.x)},${round(geom.y)})`,
        tabindex: 0,
        role: 'button',
        'data-part': geom.index,
        'aria-label': `${geom.chip}, part ${geom.index + 1}${geom.drill ? ', press Enter to open' : ''}`,
    });
    // A row on a pin carries only its own bits (`a[0..7]=lo`): its lead shows those.
    const lead = (side: 'in' | 'out', pin: PinGeom, row: number, x1: number, x2: number, y: number) => {
        const bits = pin.slices[row] ?? null;
        const width = bits ? bits[1] : pin.width;
        const el = s('line', { class: `lead${width > 1 ? ' bus' : ''}`, x1, y1: y, x2, y2: y });
        if (bits) {
            el.setAttribute('data-slice', range(bits));
        }
        out.leads.push({ part: geom.index, side, pin: pin.index, bits, width, el });
        return el;
    };
    if (gate) {
        const shapes = gatePaths(geom.shape, geom.w, geom.bodyH);
        const end = leadEnd(geom.shape);
        g.append(s('path', { class: 'halo', d: shapes.body }));
        for (const pin of geom.inputs) {
            pin.anchors.forEach((a, r) => g.append(lead('in', pin, r, 0, end + 2, a.y - geom.y)));
        }
        for (const pin of geom.outputs) {
            pin.anchors.forEach((a, r) => g.append(lead('out', pin, r, geom.w - LEAD - 2, geom.w, a.y - geom.y)));
        }
        g.append(s('path', { class: 'body', d: shapes.body }));
        if (shapes.extra) {
            g.append(s('path', { class: 'extra', d: shapes.extra }));
        }
        if (shapes.bubble) {
            g.append(s('circle', { class: 'bubble', cx: shapes.bubble.cx, cy: shapes.bubble.cy, r: shapes.bubble.r }));
        }
        if (geom.shape === 'mux' || geom.shape === 'dmux') {
            for (const pin of geom.inputs) {
                g.append(s('text', { class: 'pin-small', x: LEAD + 3, y: pin.anchors[0].y - geom.y + 3 }, pin.name));
            }
            if (geom.shape === 'dmux') {
                for (const pin of geom.outputs) {
                    g.append(s('text', { class: 'pin-small', x: geom.w - LEAD - 3, y: pin.anchors[0].y - geom.y + 3, 'text-anchor': 'end' }, pin.name));
                }
            }
        }
        if (geom.variant) {
            g.append(s('text', { class: 'variant', x: geom.w / 2, y: geom.bodyH + 9, 'text-anchor': 'middle' }, geom.variant));
        }
    } else {
        g.append(s('rect', { class: 'halo', x: 0, y: 0, width: geom.w, height: geom.h, rx: 5 }));
        g.append(s('rect', { class: 'body', x: 0, y: 0, width: geom.w, height: geom.h, rx: 5 }));
        g.append(s('text', { class: 'chip-name', x: 9, y: 18 }, geom.chip));
        g.append(s('line', { class: 'header-rule', x1: 0, y1: 26.5, x2: geom.w, y2: 26.5 }));
        const pinText = (pin: { name: string; width: number }, x: number, y: number, anchor: string) => {
            const t = s('text', { class: 'pin', x, y: y + 3.6, 'text-anchor': anchor }, pin.name);
            if (pin.width > 1) {
                t.append(s('tspan', { class: 'pin-width' }, `[${pin.width}]`));
            }
            return t;
        };
        for (const pin of geom.inputs) {
            g.append(pinText(pin, 7, pin.anchors[0].y - geom.y, 'start'));
            if (pin.anchors.length > 1) {
                const y0 = pin.anchors[0].y - geom.y;
                const y1 = pin.anchors[pin.anchors.length - 1].y - geom.y;
                g.append(s('path', { class: 'extra', d: `M3,${y0 - 4} V${y1 + 4}` }));
            }
            pin.anchors.forEach((a, r) => g.append(lead('in', pin, r, 0, 3, a.y - geom.y)));
        }
        for (const pin of geom.outputs) {
            g.append(pinText(pin, geom.w - 7, pin.anchors[0].y - geom.y, 'end'));
            pin.anchors.forEach((a, r) => g.append(lead('out', pin, r, geom.w - 3, geom.w, a.y - geom.y)));
        }
        if (geom.sequential) {
            const cy = geom.h - 11;
            g.append(s('path', { class: 'clock', d: `M0,${cy - 5} L7,${cy} L0,${cy + 5}` }));
        }
    }
    if (geom.drill) {
        const bx = geom.w - 17;
        const by = gate ? -6 : 6;
        const drill = s('g', { class: 'drill-btn', transform: `translate(${bx},${by})`, 'data-drill': geom.index });
        drill.append(
            s('rect', { x: 0, y: 0, width: 13, height: 13, rx: 3 }),
            s('path', { d: 'M4.5,3.5 H9.5 V8.5 M9.5,3.5 L3.5,9.5' }),
            s('title', {}, `Open ${geom.chip}`),
        );
        g.append(drill);
    }
    g.append(s('title', {}, `${part.chip}${part.drill ? ' — double-click to open' : part.builtin ? ' (built-in)' : ''}`));
    return g;
}

function drawPort(port: PortGeom, editable: boolean): Drawn['ports'][number] {
    const tip = 8;
    const w = port.w;
    const hgt = port.h;
    const g = s('g', {
        class: `port ${port.side}${port.width > 1 ? ' bus' : ''}${editable ? ' editable' : port.side === 'in' ? ' driven' : ''}`,
        transform: `translate(${round(port.x)},${round(port.y)})`,
        'data-port': `${port.side}:${port.index}`,
    });
    if (editable) {
        g.setAttribute('tabindex', '0');
        g.setAttribute('role', port.width === 1 ? 'switch' : 'button');
    }
    const y0 = 0;
    const y1 = hgt;
    const shape = port.side === 'in' ? `M3,${y0} H${w - tip} L${w},${hgt / 2} L${w - tip},${y1} H3 Q0,${y1} 0,${y1 - 3} V${y0 + 3} Q0,${y0} 3,${y0} Z` : `M0,${y0} H${w - tip} L${w},${hgt / 2} L${w - tip},${y1} H0 Z`;
    g.append(s('path', { class: 'shape', d: shape }));
    const mid = 9;
    g.append(s('text', { class: 'port-name', x: port.side === 'in' ? 8 : 9, y: mid + 4 }, port.name));
    let value: SVGTextElement | null = null;
    let bit: SVGCircleElement | null = null;
    let bitText: SVGTextElement | null = null;
    if (port.width === 1) {
        bit = s('circle', { class: 'bit', cx: w - tip - 9, cy: mid, r: 6 });
        bitText = s('text', { class: 'bit-text', x: w - tip - 9, y: mid + 3.3, 'text-anchor': 'middle' }, '0');
        g.append(bit, bitText);
    } else {
        value = s('text', { class: 'port-value', x: w - tip - 4, y: mid + 3.8, 'text-anchor': 'end' }, '0');
        g.append(value);
    }
    return { geom: port, el: g, value, bit, bitText };
}

// ---- values -------------------------------------------------------------------

function netBits(net: number, source: Values | null = values): Bits {
    return source?.nets[net] ?? 0;
}

function wireState(geom: WireGeom, source: Values | null): string {
    const v = sliceValue(netBits(geom.net, source), geom.bits);
    return geom.width > 1 ? (v !== 0n ? 'nz' : 'z') : v !== 0n ? 'hi' : 'lo';
}

function setWire(w: Drawn['wires'][number], state: string): void {
    if (w.state === state) {
        return;
    }
    w.state = state;
    for (const el of [w.el, w.glow]) {
        el.classList.toggle('hi', state === 'hi');
        el.classList.toggle('nz', state === 'nz');
    }
}

function setNetDecor(net: number, source: Values): void {
    if (!drawn) {
        return;
    }
    for (const j of drawn.junctions) {
        if (j.net === net) {
            const v = sliceValue(netBits(net, source), j.bits) !== 0n;
            j.el.classList.toggle(j.width > 1 ? 'nz' : 'hi', v);
        }
    }
    for (const label of drawn.labels) {
        if (label.net === net) {
            updateLabel(label, source);
        }
    }
}

function updateLabel(label: Drawn['labels'][number], source: Values): void {
    if (label.kind !== 'value') {
        return;
    }
    const v = sliceValue(netBits(label.net, source), label.bits);
    const text = formatValue(v.toString(), label.width, settings.radix);
    label.el.textContent = label.prefix ? `${label.prefix} ${text}` : text;
    label.el.classList.toggle('zero', v === 0n);
}

function updatePort(port: Drawn['ports'][number], source: Values): void {
    const bits = port.geom.net >= 0 ? netBits(port.geom.net, source) : 0;
    const v = toBigInt(bits);
    if (port.bit && port.bitText) {
        port.bit.classList.toggle('hi', v !== 0n);
        port.bitText.classList.toggle('hi', v !== 0n);
        port.bitText.textContent = v !== 0n ? '1' : '0';
        if (port.el.getAttribute('role') === 'switch') {
            port.el.setAttribute('aria-checked', String(v !== 0n));
        }
    }
    if (port.value) {
        port.value.textContent = formatValue(bits, port.geom.width, settings.radix);
        port.el.classList.toggle('zero', v === 0n);
    }
    port.el.setAttribute('aria-label', `${port.geom.side === 'in' ? 'Input' : 'Output'} ${port.geom.name} = ${formatValue(bits, port.geom.width, 'dec')}`);
}

function updateLeads(source: Values): void {
    if (!drawn) {
        return;
    }
    for (const lead of drawn.leads) {
        const pv = source.parts[lead.part];
        const bits = pv ? (lead.side === 'in' ? pv.inputs[lead.pin] : pv.outputs[lead.pin]) : 0;
        const v = sliceValue(bits ?? 0, lead.bits) !== 0n;
        lead.el.classList.toggle(lead.width > 1 ? 'nz' : 'hi', v);
    }
}

/** Shows `values` at once (no animation); `null` re-applies the current ones. */
function applyValues(source: Values | null): void {
    const v = source ?? values;
    if (!drawn || !v) {
        return;
    }
    for (const w of drawn.wires) {
        setWire(w, wireState(w.geom, v));
    }
    for (const j of drawn.junctions) {
        j.el.classList.toggle(j.width > 1 ? 'nz' : 'hi', sliceValue(netBits(j.net, v), j.bits) !== 0n);
    }
    for (const label of drawn.labels) {
        updateLabel(label, v);
    }
    for (const port of drawn.ports) {
        updatePort(port, v);
    }
    updateLeads(v);
}

// ---- propagation animation --------------------------------------------------------

let animation: { frame: number; finish: () => void } | null = null;

function finishAnimation(): void {
    if (animation) {
        const a = animation;
        animation = null;
        cancelAnimationFrame(a.frame);
        a.finish();
    }
    drawn?.pulses.replaceChildren();
}

function animateTo(next: Values, previous: Values | null, cause: ValuesCause): void {
    finishAnimation();
    values = next;
    updateToolbar();
    if (!drawn || !previous || !settings.animate || reducedMotion() || previous.nets.length !== next.nets.length) {
        applyValues(next);
        return;
    }
    const changed = drawn.wires.filter((w) => wireState(w.geom, previous) !== wireState(w.geom, next) || sliceValue(netBits(w.geom.net, previous), w.geom.bits) !== sliceValue(netBits(w.geom.net, next), w.geom.bits));
    if (changed.length === 0) {
        applyValues(next);
        return;
    }
    // Ports, junctions and labels follow their net; inputs at once.
    const netStart = new Map<number, number>();
    const netEnd = new Map<number, number>();
    for (const w of changed) {
        netStart.set(w.geom.net, Math.min(netStart.get(w.geom.net) ?? Infinity, w.geom.t0));
        netEnd.set(w.geom.net, Math.max(netEnd.get(w.geom.net) ?? -Infinity, w.geom.t1));
    }
    const t0 = minOf(changed, (w) => w.geom.t0, Infinity);
    const t1 = maxOf(changed, (w) => w.geom.t1, -Infinity);
    const pending = new Set(changed);
    const pendingNets = new Set(netStart.keys());
    const pendingOut = new Set(drawn.ports.filter((p) => p.geom.side === 'out' && netEnd.has(p.geom.net)));
    for (const port of drawn.ports) {
        if (!pendingOut.has(port)) {
            updatePort(port, next);
        }
    }
    updateLeads(next);
    void cause;
    const started = performance.now();
    const step = () => {
        if (!drawn) {
            return;
        }
        const t = t0 + (performance.now() - started) / HALF_LAYER_MS;
        for (const w of pending) {
            if (w.geom.t0 <= t) {
                pending.delete(w);
                setWire(w, wireState(w.geom, next));
                pulse(w, next);
            }
        }
        for (const net of pendingNets) {
            if (netStart.get(net)! <= t) {
                pendingNets.delete(net);
                setNetDecor(net, next);
            }
        }
        for (const port of pendingOut) {
            if (netEnd.get(port.geom.net)! <= t) {
                pendingOut.delete(port);
                updatePort(port, next);
            }
        }
        if (t >= t1 && pending.size === 0) {
            animation = null;
            return;
        }
        animation!.frame = requestAnimationFrame(step);
    };
    animation = {
        frame: requestAnimationFrame(step),
        finish: () => {
            applyValues(next);
        },
    };
}

function pulse(w: Drawn['wires'][number], next: Values): void {
    if (!drawn) {
        return;
    }
    const points = w.geom.points;
    let length = 0;
    for (let i = 1; i < points.length; i++) {
        length += Math.abs(points[i].x - points[i - 1].x) + Math.abs(points[i].y - points[i - 1].y);
    }
    if (length < 1) {
        return;
    }
    const state = wireState(w.geom, next);
    const cls = w.geom.width > 1 ? 'bus' : state === 'hi' ? 'hi' : 'lo';
    const d = w.el.getAttribute('d')!;
    const flash = s('path', { class: `flash ${cls}`, d });
    const dot = s('path', { class: `pulse ${cls}`, d });
    const dash = Math.min(16, Math.max(6, length * 0.5));
    dot.setAttribute('stroke-dasharray', `${dash} ${length + dash}`);
    drawn.pulses.append(flash, dot);
    const duration = Math.max(110, (w.geom.t1 - w.geom.t0) * HALF_LAYER_MS * 2.2);
    const travel = dot.animate([{ strokeDashoffset: dash }, { strokeDashoffset: -length }], { duration, easing: 'cubic-bezier(.3,.1,.4,1)' });
    const glow = flash.animate([{ opacity: cls === 'lo' ? 0.25 : 0.45 }, { opacity: 0 }], { duration: duration + 260, easing: 'ease-out' });
    travel.onfinish = () => dot.remove();
    glow.onfinish = () => flash.remove();
}

// ---- view: pan and zoom ------------------------------------------------------------

function applyView(): void {
    drawn?.viewport.setAttribute('transform', `translate(${round(view.x)},${round(view.y)}) scale(${view.k.toFixed(4)})`);
    zoomLabel.textContent = `${Math.round(view.k * 100)}%`;
}

function fit(): void {
    if (!layout) {
        return;
    }
    const w = stage.clientWidth || 800;
    const hgt = stage.clientHeight || 600;
    const k = Math.max(0.1, Math.min(1.75, (w - 24) / layout.width, (hgt - 24) / layout.height));
    view = { k, x: (w - layout.width * k) / 2, y: (hgt - layout.height * k) / 2 };
    fitted = true;
    applyView();
}

function zoomBy(factor: number, cx = stage.clientWidth / 2, cy = stage.clientHeight / 2): void {
    const k = Math.max(0.1, Math.min(5, view.k * factor));
    const f = k / view.k;
    view = { k, x: cx - (cx - view.x) * f, y: cy - (cy - view.y) * f };
    fitted = false;
    applyView();
    hideTip();
}

function panBy(dx: number, dy: number): void {
    view = { ...view, x: view.x + dx, y: view.y + dy };
    fitted = false;
    applyView();
}

stage.addEventListener(
    'wheel',
    (event) => {
        event.preventDefault();
        const rect = stage.getBoundingClientRect();
        const scale = event.ctrlKey ? 0.01 : event.deltaMode === 1 ? 0.05 : 0.0018;
        zoomBy(Math.exp(-event.deltaY * scale), event.clientX - rect.left, event.clientY - rect.top);
    },
    { passive: false },
);

let drag: { x: number; y: number; vx: number; vy: number; moved: boolean; id: number } | null = null;
let suppressClick = false;
stage.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || (event.target as Element).closest('.c-edit, .c-tip, .c-problem')) {
        return;
    }
    drag = { x: event.clientX, y: event.clientY, vx: view.x, vy: view.y, moved: false, id: event.pointerId };
});
stage.addEventListener('pointermove', (event) => {
    if (drag && drag.id === event.pointerId) {
        const dx = event.clientX - drag.x;
        const dy = event.clientY - drag.y;
        if (!drag.moved && Math.hypot(dx, dy) > 4) {
            drag.moved = true;
            stage.setPointerCapture(event.pointerId);
            stage.classList.add('panning');
            hideTip();
        }
        if (drag.moved) {
            view = { ...view, x: drag.vx + dx, y: drag.vy + dy };
            fitted = false;
            applyView();
            return;
        }
    }
    hover(event);
});
const endDrag = (event: PointerEvent) => {
    if (drag && drag.id === event.pointerId) {
        suppressClick = drag.moved;
        drag = null;
        stage.classList.remove('panning');
    }
};
stage.addEventListener('pointerup', endDrag);
stage.addEventListener('pointercancel', endDrag);
stage.addEventListener('pointerleave', () => {
    if (!drag) {
        hideTip();
        setHoverNet(null);
    }
});

new ResizeObserver(() => {
    if (fitted) {
        fit();
    }
}).observe(stage);

// ---- hover: net highlight and tooltip -------------------------------------------------

let hoverNet: number | null = null;

function setHoverNet(net: number | null): void {
    if (!drawn || hoverNet === net) {
        return;
    }
    if (hoverNet !== null) {
        for (const el of drawn.byNet.get(hoverNet) ?? []) {
            el.classList.remove('on');
        }
    }
    hoverNet = net;
    drawn.svg.classList.toggle('focus-net', net !== null);
    if (net !== null) {
        for (const el of drawn.byNet.get(net) ?? []) {
            el.classList.add('on');
        }
    }
}

function valueRows(bits: Bits | bigint, width: number): HTMLTableElement {
    const text = typeof bits === 'bigint' ? bits.toString() : bits;
    const table = h('table');
    for (const radix of ['dec', 'bin', 'hex'] as Radix[]) {
        table.append(h('tr', {}, h('td', {}, radix), h('td', {}, formatValue(text, width, radix))));
    }
    return table;
}

function showTip(event: { clientX: number; clientY: number }, ...content: Node[]): void {
    tip.replaceChildren(...content);
    tip.hidden = false;
    const rect = stage.getBoundingClientRect();
    let x = event.clientX - rect.left + 14;
    let y = event.clientY - rect.top + 16;
    const tw = tip.offsetWidth;
    const th = tip.offsetHeight;
    if (x + tw > rect.width - 6) {
        x = Math.max(6, event.clientX - rect.left - tw - 12);
    }
    if (y + th > rect.height - 6) {
        y = Math.max(6, event.clientY - rect.top - th - 12);
    }
    tip.style.left = `${x}px`;
    tip.style.top = `${y}px`;
}

function hideTip(): void {
    tip.hidden = true;
}

function hover(event: PointerEvent): void {
    if (!diagram || !values) {
        return;
    }
    const target = (event.target as Element).closest('[data-net], [data-part], [data-port]');
    if (!target) {
        hideTip();
        setHoverNet(null);
        return;
    }
    if (target.hasAttribute('data-net')) {
        const net = Number(target.getAttribute('data-net'));
        const decl = diagram.nets[net];
        if (!decl) {
            return;
        }
        setHoverNet(net);
        const bitsAttr = target.getAttribute('data-bits');
        const head = h('div', { class: 't-head' }, decl.name, h('span', { class: 'muted' }, `  ${decl.width} bit${decl.width > 1 ? 's' : ''} · ${decl.kind}`));
        const nodes: Node[] = [head, valueRows(netBits(net), decl.width)];
        if (bitsAttr) {
            const [start, width] = bitsAttr.split(',').map(Number) as BitRange;
            const v = sliceValue(netBits(net), [start, width]);
            nodes.push(h('div', { class: 't-foot' }, `this wire: ${decl.name}${range([start, width])} = ${formatValue(v.toString(), width, settings.radix)}`));
        }
        nodes.push(h('div', { class: 't-foot' }, 'Click to find it in the HDL'));
        showTip(event, ...nodes);
        return;
    }
    if (target.hasAttribute('data-port')) {
        const [side, i] = target.getAttribute('data-port')!.split(':');
        const pin = side === 'in' ? diagram.inputs[Number(i)] : diagram.outputs[Number(i)];
        const port = layout?.[side === 'in' ? 'inputs' : 'outputs'][Number(i)];
        if (!pin || !port) {
            return;
        }
        setHoverNet(port.net >= 0 ? port.net : null);
        const editable = side === 'in' && diagram.path.length === 0;
        showTip(
            event,
            h('div', { class: 't-head' }, pin.name, h('span', { class: 'muted' }, `  ${side === 'in' ? 'input' : 'output'} · ${pin.width} bit${pin.width > 1 ? 's' : ''}`)),
            valueRows(port.net >= 0 ? netBits(port.net) : 0, pin.width),
            ...(editable ? [h('div', { class: 't-foot' }, pin.width === 1 ? 'Click to flip' : 'Click to type a value')] : []),
        );
        return;
    }
    setHoverNet(null);
    const index = Number(target.getAttribute('data-part'));
    const part = diagram.parts[index];
    const pv = values.parts[index];
    if (!part) {
        return;
    }
    const table = h('table');
    const row = (name: string, width: number, bits: Bits | undefined, dir: string) =>
        table.append(h('tr', {}, h('td', {}, dir), h('td', {}, name), h('td', {}, formatValue(bits ?? 0, width, settings.radix))));
    part.inputs.forEach((pin, i) => row(pin.name, pin.width, pv?.inputs[i], 'in'));
    part.outputs.forEach((pin, i) => row(pin.name, pin.width, pv?.outputs[i], 'out'));
    const kind = part.drill ? 'double-click to open' : part.builtin ? 'built-in' : '';
    showTip(
        event,
        h('div', { class: 't-head' }, part.chip, h('span', { class: 'muted' }, `  part ${index + 1}${part.sequential ? ' · clocked' : ''}`)),
        table,
        h('div', { class: 't-foot' }, ['Click to show it in the HDL', kind].filter(Boolean).join(' · ')),
    );
}

// ---- clicks and keys -------------------------------------------------------------------

stage.addEventListener('click', (event) => {
    if (suppressClick) {
        suppressClick = false;
        return;
    }
    if (!diagram) {
        return;
    }
    const target = event.target as Element;
    const drillEl = target.closest('[data-drill]');
    if (drillEl) {
        drillInto(Number(drillEl.getAttribute('data-drill')));
        return;
    }
    const port = target.closest('[data-port]');
    if (port) {
        hideTip();
        clickPort(port as SVGGElement);
        return;
    }
    const part = target.closest('[data-part]');
    if (part) {
        const index = Number(part.getAttribute('data-part'));
        highlight = { part: index };
        applyHighlight();
        command({ type: 'reveal', part: index });
        return;
    }
    const wire = target.closest('[data-net]');
    if (wire) {
        const net = Number(wire.getAttribute('data-net'));
        highlight = { net };
        applyHighlight();
        command({ type: 'reveal', net });
        return;
    }
    if (highlight.part !== undefined || highlight.net !== undefined) {
        highlight = {};
        applyHighlight();
    }
});

stage.addEventListener('dblclick', (event) => {
    const part = (event.target as Element).closest('[data-part]');
    if (part) {
        drillInto(Number(part.getAttribute('data-part')));
    }
});

function drillInto(index: number): void {
    if (!diagram) {
        return;
    }
    const part = diagram.parts[index];
    if (part?.drill) {
        command({ type: 'drill', path: [...diagram.path, index] });
    } else if (part) {
        flashNote(`${part.chip} is built in: it has no parts to show.`);
    }
}

let noteTimer: ReturnType<typeof setTimeout> | null = null;

function hideNote(): void {
    if (noteTimer !== null) {
        clearTimeout(noteTimer);
        noteTimer = null;
    }
    noteBox.hidden = true;
    noteBox.textContent = '';
}

function flashNote(text: string): void {
    if (noteTimer !== null) {
        clearTimeout(noteTimer);
    }
    noteBox.textContent = text;
    noteBox.hidden = false;
    noteTimer = setTimeout(() => {
        noteTimer = null;
        noteBox.hidden = true;
        noteBox.textContent = '';
    }, 3500);
}

function clickPort(el: SVGGElement): void {
    if (!diagram || diagram.path.length !== 0) {
        return;
    }
    const [side, i] = el.getAttribute('data-port')!.split(':');
    if (side !== 'in') {
        return;
    }
    const pin = diagram.inputs[Number(i)];
    const port = layout?.inputs[Number(i)];
    if (!pin || !port) {
        return;
    }
    if (pin.width === 1) {
        const on = port.net >= 0 && toBigInt(netBits(port.net)) !== 0n;
        command({ type: 'set', pin: pin.name, value: on ? 0 : 1 });
    } else {
        openEditor(el, pin.name, pin.width, port.net);
    }
}

let editing: { pin: string; shown: string } | null = null;

function openEditor(el: SVGGElement, pin: string, width: number, net: number): void {
    hideTip();
    const rect = el.getBoundingClientRect();
    const stageRect = stage.getBoundingClientRect();
    const shown = formatValue(net >= 0 ? netBits(net) : 0, width, settings.radix);
    editing = { pin, shown };
    editor.value = shown;
    editor.hidden = false;
    editor.setAttribute('aria-label', `${pin}, ${width} bits: decimal, 0b… binary or 0x… hex`);
    editor.style.left = `${Math.max(4, rect.left - stageRect.left)}px`;
    editor.style.top = `${rect.top - stageRect.top + rect.height + 4}px`;
    editor.style.width = `${Math.max(rect.width, 150)}px`;
    editError.textContent = '';
    editError.style.left = editor.style.left;
    editError.style.top = `${rect.top - stageRect.top + rect.height + 30}px`;
    editor.focus();
    editor.select();
}

function closeEditor(commit: boolean): void {
    if (!editing) {
        return;
    }
    const { pin, shown } = editing;
    editing = null;
    const text = editor.value.trim();
    editor.hidden = true;
    editError.textContent = '';
    if (commit && text !== shown && text !== '') {
        command({ type: 'set', pin, value: typedValue(text, settings.radix) });
    }
    stage.focus();
}

editor.addEventListener('keydown', (event) => {
    event.stopPropagation();
    if (event.key === 'Enter') {
        closeEditor(true);
    } else if (event.key === 'Escape') {
        closeEditor(false);
    }
});
editor.addEventListener('blur', () => closeEditor(true));
editor.addEventListener('pointerdown', (event) => event.stopPropagation());

stage.addEventListener('keydown', (event) => {
    if (event.target === editor) {
        return;
    }
    const target = event.target as Element;
    const step = 48;
    switch (event.key) {
        case 'ArrowLeft':
            panBy(step, 0);
            break;
        case 'ArrowRight':
            panBy(-step, 0);
            break;
        case 'ArrowUp':
            panBy(0, step);
            break;
        case 'ArrowDown':
            panBy(0, -step);
            break;
        case '+':
        case '=':
            zoomBy(1.25);
            break;
        case '-':
        case '_':
            zoomBy(1 / 1.25);
            break;
        case '0':
            fit();
            break;
        case 'Enter':
        case ' ': {
            const part = target.closest?.('[data-part]');
            const port = target.closest?.('[data-port]');
            if (port) {
                clickPort(port as SVGGElement);
            } else if (part) {
                const index = Number(part.getAttribute('data-part'));
                if (event.key === 'Enter' && diagram?.parts[index]?.drill) {
                    drillInto(index);
                } else {
                    highlight = { part: index };
                    applyHighlight();
                    command({ type: 'reveal', part: index });
                }
            } else {
                return;
            }
            break;
        }
        case 'Backspace':
            if (diagram && diagram.path.length) {
                command({ type: 'drill', path: diagram.path.slice(0, -1) });
                break;
            }
            return;
        case 'Escape':
            highlight = {};
            applyHighlight();
            hideTip();
            break;
        default:
            return;
    }
    event.preventDefault();
});

stage.addEventListener('focusin', (event) => {
    const part = (event.target as Element).closest?.('[data-part], [data-port]');
    if (part && layout) {
        ensureVisible(part as SVGGElement);
    }
});

function ensureVisible(el: SVGGElement): void {
    const r = el.getBoundingClientRect();
    const s = stage.getBoundingClientRect();
    let dx = 0;
    let dy = 0;
    if (r.left < s.left + 12) {
        dx = s.left + 12 - r.left;
    } else if (r.right > s.right - 12) {
        dx = s.right - 12 - r.right;
    }
    if (r.top < s.top + 12) {
        dy = s.top + 12 - r.top;
    } else if (r.bottom > s.bottom - 12) {
        dy = s.bottom - 12 - r.bottom;
    }
    if (dx || dy) {
        panBy(dx, dy);
    }
}

// ---- highlight from the editor -----------------------------------------------------------

function applyHighlight(): void {
    if (!drawn) {
        return;
    }
    for (const el of drawn.svg.querySelectorAll('.hl')) {
        el.classList.remove('hl');
    }
    if (highlight.part !== undefined) {
        drawn.parts.get(highlight.part)?.classList.add('hl');
    }
    if (highlight.net !== undefined) {
        for (const el of drawn.byNet.get(highlight.net) ?? []) {
            el.classList.add('hl');
        }
    }
}

// ---- export --------------------------------------------------------------------------------

const INLINE = ['fill', 'fill-opacity', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray', 'stroke-linecap', 'stroke-linejoin', 'opacity', 'paint-order'];
const INLINE_TEXT = ['font-family', 'font-size', 'font-weight', 'font-style', 'letter-spacing'];

/** The current level as a standalone SVG: computed theme colours, no classes, no CSS variables. */
function exportSvg(): string {
    if (!drawn || !layout) {
        throw new Error('Nothing to export yet.');
    }
    finishAnimation();
    setHoverNet(null);
    const out = document.createElementNS(SVG, 'svg');
    out.setAttribute('xmlns', SVG);
    out.setAttribute('width', String(layout.width));
    out.setAttribute('height', String(layout.height));
    out.setAttribute('viewBox', `0 0 ${layout.width} ${layout.height}`);
    const bg = getComputedStyle(drawn.svg.closest('.circuit') ?? document.body).getPropertyValue('--c-bg').trim() || getComputedStyle(document.body).backgroundColor;
    const background = document.createElementNS(SVG, 'rect');
    background.setAttribute('width', '100%');
    background.setAttribute('height', '100%');
    background.setAttribute('fill', resolveColor(bg));
    out.append(background);
    const copy = (from: Element, into: Element) => {
        for (const child of Array.from(from.children)) {
            const cls = child.getAttribute('class') ?? '';
            if (/\b(hit|grid|g-hits|g-pulses|halo)\b/.test(cls) || child.tagName === 'title' || child.tagName === 'defs') {
                continue;
            }
            const style = getComputedStyle(child);
            if (style.display === 'none' || style.visibility === 'hidden') {
                continue;
            }
            const clone = child.cloneNode(false) as Element;
            for (const name of ['class', 'tabindex', 'role', 'aria-label', 'aria-checked', 'data-part', 'data-port', 'data-net', 'data-bits', 'data-drill']) {
                clone.removeAttribute(name);
            }
            const props = child.tagName === 'text' || child.tagName === 'tspan' ? [...INLINE, ...INLINE_TEXT] : INLINE;
            if (child.tagName !== 'g') {
                for (const prop of props) {
                    const value = style.getPropertyValue(prop);
                    if (value && value !== 'normal' && !(prop === 'stroke-dasharray' && value === 'none')) {
                        clone.setAttribute(prop, prop === 'fill' || prop === 'stroke' ? resolveColor(value) : value);
                    }
                }
            } else if (style.opacity !== '1') {
                clone.setAttribute('opacity', style.opacity);
            }
            if (child.tagName === 'text' || child.tagName === 'tspan') {
                for (const node of Array.from(child.childNodes)) {
                    if (node.nodeType === Node.TEXT_NODE) {
                        clone.append(node.cloneNode());
                    }
                }
            }
            into.append(clone);
            copy(child, clone);
        }
    };
    const content = document.createElementNS(SVG, 'g');
    out.append(content);
    // Copy the drawing, without the view transform.
    // Without the editor's highlight or keyboard focus: a picture of the circuit.
    const saved = highlight;
    highlight = {};
    applyHighlight();
    (document.activeElement as HTMLElement | null)?.blur?.();
    try {
        copy(drawn.viewport, content);
    } finally {
        highlight = saved;
        applyHighlight();
    }
    return `<?xml version="1.0" encoding="UTF-8"?>\n${new XMLSerializer().serializeToString(out)}`;
}

const colorCache = new Map<string, string>();
let colorCanvas: CanvasRenderingContext2D | null = null;
/**
 * A computed colour as `#rrggbb` or `rgba()`, so that no `color-mix()`,
 * `color(srgb …)` or variable is left in the export: painted on a pixel and
 * read back.
 */
function resolveColor(value: string): string {
    if (!value || value === 'none' || value === 'transparent' || /^(#|rgb\()/.test(value)) {
        return value;
    }
    const cached = colorCache.get(value);
    if (cached) {
        return cached;
    }
    colorCanvas ??= Object.assign(document.createElement('canvas'), { width: 1, height: 1 }).getContext('2d', { willReadFrequently: true });
    if (!colorCanvas) {
        return value;
    }
    colorCanvas.clearRect(0, 0, 1, 1);
    colorCanvas.fillStyle = '#000';
    colorCanvas.fillStyle = value;
    colorCanvas.fillRect(0, 0, 1, 1);
    const [r, g, b, a] = colorCanvas.getImageData(0, 0, 1, 1).data;
    const hex = (n: number) => n.toString(16).padStart(2, '0');
    const resolved = a === 255 ? `#${hex(r)}${hex(g)}${hex(b)}` : a === 0 ? 'none' : `rgba(${r},${g},${b},${(a / 255).toFixed(3)})`;
    colorCache.set(value, resolved);
    return resolved;
}

async function exportPng(svg: string): Promise<string> {
    if (!layout) {
        throw new Error('Nothing to export yet.');
    }
    // The size of the drawing exported, not of whatever is shown by the time the image has loaded.
    const width = layout.width;
    const height = layout.height;
    const size = pngSize(width, height, 2);
    if (!size) {
        throw new Error('The diagram has no size.');
    }
    const image = new Image();
    image.decoding = 'async';
    const loaded = new Promise<void>((resolve, reject) => {
        image.onload = () => resolve();
        image.onerror = () => reject(new Error('The diagram could not be drawn as an image.'));
    });
    image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
    await loaded;
    const canvas = document.createElement('canvas');
    canvas.width = size.width;
    canvas.height = size.height;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
        throw new Error(`The diagram is too large for a PNG (${size.width} × ${size.height} pixels). Export it as SVG instead.`);
    }
    ctx.scale(size.scale, size.scale);
    ctx.drawImage(image, 0, 0, width, height);
    const data = canvas.toDataURL('image/png').replace(/^data:image\/png;base64,/, '');
    if (!isPngBase64(data)) {
        throw new Error(`The diagram is too large for a PNG (${size.width} × ${size.height} pixels). Export it as SVG instead.`);
    }
    if (size.scale < 2) {
        flashNote(`The diagram is large: the PNG is ${size.width} × ${size.height} pixels (${Math.round(size.scale * 100)}%).`);
    }
    return data;
}

async function exportAs(format: 'svg' | 'png'): Promise<void> {
    try {
        const svg = exportSvg();
        send({ type: 'export', format, data: format === 'svg' ? svg : await exportPng(svg) });
    } catch (error) {
        send({ type: 'error', message: `Export failed: ${error instanceof Error ? error.message : String(error)}`, notify: true });
    }
}

// ---- messages ------------------------------------------------------------------------------

function showProblem(message: string | null, retry = false): void {
    problem = message;
    problemBox.hidden = message === null;
    problemBox.replaceChildren();
    if (message !== null) {
        const heading = retry ? 'The simulator stopped.' : diagram ? 'The chip has a problem; showing the last version that worked.' : 'The chip has a problem.';
        problemBox.append(h('strong', {}, heading), h('pre', {}, message));
        if (retry) {
            const again = h('button', { type: 'button', class: 'primary c-retry' }, 'Retry');
            again.addEventListener('click', () => send({ type: 'retry' }));
            problemBox.append(again);
        }
    }
    drawn?.svg.classList.toggle('stale', message !== null);
}

window.addEventListener('message', (event: MessageEvent<ToCircuit>) => {
    const message = event.data;
    try {
        switch (message.type) {
            case 'diagram': {
                const path = message.diagram.path.join('/');
                const refit = !diagram || path !== shownPath || diagram.chip !== message.diagram.chip;
                const sameLevel = diagram && !refit;
                const previous = values;
                if (shownTarget !== 0 && shownTarget !== message.target) {
                    hideNote();
                }
                diagram = message.diagram;
                shownToken = message.token;
                shownTarget = message.target;
                shownPath = path;
                const radixChanged = settings.radix !== message.settings.radix;
                settings = { ...message.settings };
                values = diagram.values;
                if (!sameLevel) {
                    highlight = {};
                }
                showProblem(null);
                updateCrumbs();
                updateToolbar();
                redraw(refit);
                void previous;
                void radixChanged;
                send({ type: 'drawn', diagram: shownToken });
                break;
            }
            case 'values': {
                // Values of another build of the chip, or of another level,
                // would light the wrong wires: the right diagram follows.
                if (!diagram || !valuesFit(diagram, message.values)) {
                    break;
                }
                const previous = values;
                animateTo(message.values, previous, message.cause);
                break;
            }
            case 'highlight':
                highlight = { part: message.part, net: message.net };
                applyHighlight();
                if (message.part !== undefined && drawn) {
                    const el = drawn.parts.get(message.part);
                    if (el) {
                        ensureVisible(el);
                    }
                }
                break;
            case 'error':
                showProblem(message.message, message.retry === true);
                break;
            case 'note':
                flashNote(message.message);
                break;
            case 'settings': {
                const radixChanged = settings.radix !== message.settings.radix;
                settings = { ...message.settings };
                updateToolbar();
                if (radixChanged) {
                    redraw(false);
                }
                break;
            }
        }
    } catch (error) {
        send({ type: 'error', message: `Circuit view: ${error instanceof Error ? error.stack ?? error.message : String(error)}` });
    }
});

window.addEventListener('error', (event) => {
    send({ type: 'error', message: `Circuit view: ${event.message}` });
});

updateToolbar();
send({ type: 'ready' });
