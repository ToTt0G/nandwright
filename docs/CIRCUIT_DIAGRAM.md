# Circuit diagram: design and contracts

The circuit diagram is a separate webview panel that draws an HDL chip as a live
schematic: the chip's parts as gate symbols or boxes, wires coloured by their current
value, with drill-down into composite parts. This file is the contract between the
three pieces built for it. Change it only together with every side.

```
crates/simulator   per-instance probes into the flattened circuit
crates/wasm        chip.diagram / chip.values ops on the existing chip session
extension/src/circuitPanel.ts        panel, command, editor links, export
extension/src/chipSessions.ts        one hub per chip file: the session both panels share, its queue and events
extension/src/webview/circuit.ts     webview entry: toolbar, rendering, interaction
extension/src/webview/circuitLayout.ts  pure layout (no DOM), unit-tested
extension/src/webview/circuitTypes.ts   the types below, shared by both sides
```

## Behaviour (what a student sees)

- **Command** `nandwright.showCircuit`, "Show Circuit Diagram" (category Nand2Tetris), on
  `.hdl` files: editor title (next to Open in simulator), editor context menu, Explorer
  context menu, Command Palette. Opens beside the editor. Follows the active `.hdl` editor
  under the same rule as the chip simulator panel.
- **Layout**: inputs on the left edge, outputs on the right edge, parts left to right in
  signal-flow order, orthogonal wires. Deterministic: the same chip always looks the same.
- **Symbols**: distinctive gate shapes, chosen by chip name (also for 16-bit and multi-way
  variants, which get a small width/arity label): `Not` triangle with bubble, `And`,
  `Nand`, `Or`, `Nor`, `Xor` the usual shapes (`Nand`/`Nor` with bubble), `Mux*` and
  `DMux*` trapezoids. Everything else is a box with the chip name and pin names on its
  sides. Sequential parts (`DFF`, `Bit`, `Register`, `RAM*`, `PC`, …: `sequential: true`)
  get a clock triangle on the box edge. Composite parts (`drill: true`) get a small
  "open" affordance.
- **Values**: a 1-bit wire is dim when 0 and bright when 1. A bus is drawn thicker with a
  `/16` width mark and its value as a small label in the panel's radix (dec/bin/hex,
  toolbar toggle). Hovering a wire highlights the whole net and shows a tooltip: name,
  width, value in all three radixes. Constants (`true`, `false`) are drawn as tie-off
  symbols at the part pin.
- **Inputs**: at the top level, clicking a 1-bit input pin toggles it; clicking a bus
  input opens an inline field that takes decimal, binary (`0b…`, `%B…`) or hex (`0x…`,
  `%X…`). At deeper levels inputs are driven and not editable. Clocked chips show Tick,
  Tock, Step (tick+tock), the clock time and Reset in the toolbar.
- **Drill-down**: double-click a composite part (or press Enter on it) to show its
  insides with live values from the same simulation. A breadcrumb (`Mux4Way16 › Mux16
  (part 2) › Mux (part 7)`) goes back up. Built-in parts cannot be opened.
- **HDL link**: clicking a part reveals and selects its part statement in the `.hdl`
  file that defines the chip being shown (opening that file if it is another chip).
  Clicking a wire highlights every part statement that uses the net (editor decoration)
  and selects the first. Moving the editor cursor into a part statement highlights that
  part in the diagram; on an identifier that names a net of the shown chip, it highlights
  that net.
- **Propagation animation**: after an input change or a clock step, the wires whose
  value changed light up in signal-flow order: layer by layer of the layout, about
  90 ms per layer, a short pulse travelling along the wire, then the new colour stays.
  A new change while animating finishes the old one at once. Toolbar toggle; setting
  `nandwright.circuit.animate` (default true). Off when the OS asks for reduced motion.
- **Export**: toolbar "Export…": SVG or PNG (2× scale), save dialog, of the current
  level with theme colours baked in (the SVG must not depend on CSS variables).
- **Pan and zoom**: drag to pan, wheel/pinch to zoom, "Fit" button; fit on first draw
  and on drill. Keyboard: arrows pan, `+`/`-` zoom, `0` fit, Tab moves between parts.
- **Errors**: if the chip does not elaborate, show the engine's message and keep the
  last good diagram greyed underneath. Reload on edit (debounced like the simulator
  panel), keeping inputs and, if it still exists, the drill path.
- **Theme**: VS Code theme variables only; light, dark and high-contrast must all read
  well. No external resources; strict CSP like the other panels.
- **Size**: chips up to ~60 visible parts (a student's CPU or ALU) must lay out in well
  under 100 ms and stay legible when zoomed. A level with more than 2,000 parts
  (`MAX_DRAWN_PARTS`) is not drawn: the panel says it is too large to draw. Layout and
  drawing use loops, never `Math.max(...list)`, which overflows the stack on long lists.

## Engine ops (runtime chip session)

Both ops work on the chip session opened by `chip.open`. `path` is the list of part
indices from the root chip down to the instance shown (`[]` = the root). A path that
does not name a composite instance is an error (`{"error": …}`, as other ops).

Values are encoded as in `chip.state` pins today: a JSON number when it fits in 53 bits,
else a decimal string. (Hack widths are ≤ 16, but do not assume it.)

### `{"op":"chip.diagram","path":[…]}` →

```jsonc
{
  "revision": 1,                       // starts at 1; +1 on each successful chip.reset
  "path": [2, 7],
  "chip": "Mux",
  "file": "/abs/folder/Mux.hdl",      // file defining this chip; null if built-in
  "sourceHash": "f3235dc7995af525",   // 64-bit FNV-1a hex of `file` as built; null when `file` is null
  "breadcrumb": [                      // root first; one entry per path element + root
    { "chip": "Mux4Way16", "path": [] },
    { "chip": "Mux16", "path": [2] },
    { "chip": "Mux", "path": [2, 7] }
  ],
  "inputs":  [ { "name": "a", "width": 1 }, … ],
  "outputs": [ { "name": "out", "width": 1 } ],
  "nets": [                            // composite net order: inputs, outputs, internal
    { "name": "a", "width": 1, "kind": "input" },
    { "name": "out", "width": 1, "kind": "output" },
    { "name": "notSel", "width": 1, "kind": "internal" }
  ],
  "parts": [
    {
      "chip": "Not",
      "drill": false,                  // composite: can be opened
      "builtin": true,
      "sequential": false,
      "span": { "start": 120, "end": 145, "line": 9, "endLine": 9 }, // in `file`, 0-based lines
      "inputs":  [ { "name": "in", "width": 1 } ],
      "outputs": [ { "name": "out", "width": 1 } ],
      "deps": [[0]],                   // per output: input indices it depends on combinationally
      "conns": [
        { "dir": "in",  "pin": 0, "pinBits": [0, 1], "net": 2, "netBits": [0, 1] },
        { "dir": "in",  "pin": 1, "pinBits": [0, 16], "constant": false },
        { "dir": "out", "pin": 0, "pinBits": [0, 1], "net": 5, "netBits": [0, 1] }
      ]
    }
  ],
  "values": { … as chip.values … }
}
```

`pinBits`/`netBits` are `[start, width]`, least-significant bit first. A part may have
several connections on one pin (`a[0..7]=x, a[8..15]=y`) and an output pin may feed
several nets; unconnected input bits are 0.

### `{"op":"chip.values","path":[…]}` →

```jsonc
{
  "revision": 1,
  "path": [2, 7],
  "time": "3+",          // as chip.state; "" when not clocked
  "clocked": true,
  "nets":  [ 0, 1, 65535, … ],                   // one per net, same order as diagram.nets
  "parts": [ { "inputs": [ … ], "outputs": [ … ] }, … ]  // each part's own pin values
}
```

Part pin values come from the part instance itself, so they are right for constant-fed
inputs and unconnected outputs too.

Every `chip.state` reply (including `chip.open`, `chip.set`, `chip.tick`, `chip.reset`, …),
`chip.diagram` and `chip.values` includes a `revision` number that starts at 1 and
increments on each successful `chip.reset` reload (unchanged if a reload fails).
`chip.values` also echoes `path`, so a panel can detect stale values against its current
diagram and reload instead of applying mismatched net indices.

Every existing chip op (`chip.set`, `chip.tick`, …) keeps its reply shape (with `revision`
added to `chip.state`). The panel calls `chip.values` after each change.

**Source identity (`sourceHash`).** `chip.diagram` returns `"sourceHash"`, a 16-character
lowercase zero-padded hex string (no `0x` prefix) of the 64-bit FNV-1a hash of the exact
source text of `file` captured during elaboration/reset (never re-read at diagram time;
`null` when `file` is `null`). Algorithm over the exact UTF-8 bytes of the text (no
line-ending normalisation): start with `h = 0xcbf29ce484222325`, and for each byte `b`
compute `h = ((h ^ b) * 0x00000100000001b3) mod 2^64`. Test vector:
`"CHIP X {}\n"` → `"f3235dc7995af525"`.

**Revision guard (`expectRevision`).** Every chip-session op (`chip.set`, `chip.eval`,
`chip.tick`, `chip.tock`, `chip.step`, `chip.reset`, `chip.state`, `chip.diagram`,
`chip.values`, `chip.table`, `key`) accepts an optional `"expectRevision": <number>`. When
present (non-null) and different from the session's current `revision`, the op leaves
session state untouched and returns a `stale-revision` error. Omitted or `null` keeps
unguarded behaviour.

**Typed errors.** Chip-session error replies (and ended-session errors) keep
`{"error": {"message": "…"}}` and add `"code"` and, whenever a chip session exists, the
session's current `"revision"`:

```jsonc
{ "error": { "message": "…", "code": "stale-revision", "revision": 2 } }
```

Error codes:

- `stale-revision` — `expectRevision` did not match the session's current `revision`.
- `invalid-path` — `chip.diagram` / `chip.values` `path` is malformed or does not name a
  composite instance of the current build.
- `build-failed` — `chip.open` or `chip.reset` could not elaborate the chip (no `revision`
  on `chip.open`; on `chip.reset` the previous circuit and `revision` are kept and returned).
- `session-ended` — unknown or closed `session` id (no `revision`).
- `invalid-input` — bad pin name or value for `chip.set`.
- `other` — any other chip-session error (e.g. `chip.table` on a clocked chip, out-of-order
  `chip.tick`/`chip.tock`, bad `key` code, malformed `expectRevision`, unknown op).

Probes cost memory (one `u32` per flattened bit), so they are built only for
interactive chip sessions, never for test runs.

## Panel ⇄ webview messages

Extension → webview:

```ts
{ type: 'diagram', diagram: Diagram, settings: Settings, token: number, target: number }   // new chip, path or reload: full redraw
{ type: 'values', values: Values, cause: 'input' | 'clock' | 'reset' | 'external' }
{ type: 'highlight', part?: number, net?: number }           // from editor cursor; none = clear
{ type: 'error', message: string, retry?: boolean }          // keep last diagram, greyed; retry: show Retry
{ type: 'note', message: string }                            // short non-modal note that goes away (rebuilt, busy)
{ type: 'settings', settings: Settings }
interface Settings { animate: boolean; radix: 'dec' | 'bin' | 'hex'; reducedMotion?: boolean }
```

Webview → extension:

```ts
{ type: 'ready' }
{ type: 'drawn', diagram: number }                           // the diagram with this token is drawn
{ type: 'set', pin: string, value: string | number, diagram: number }   // top-level input
{ type: 'clock', action: 'tick' | 'tock' | 'step' | 'reset', target: number }  // on the chip file across rebuilds
{ type: 'retry' }                                            // Retry after the simulator stopped for good
{ type: 'drill', path: number[], diagram: number }           // into a part or up via breadcrumb
{ type: 'reveal', part?: number, net?: number, diagram: number }        // click → source
{ type: 'export', format: 'svg', data: string }             // SVG text
{ type: 'export', format: 'png', data: string }             // base64, no data: prefix
{ type: 'settings', animate?: boolean, radix?: 'dec' | 'bin' | 'hex' }
{ type: 'error', message: string, notify?: boolean }        // webview-side failure, for the log
                                                             // (notify: also shown, e.g. a failed export)
```

**Commands are bound to a diagram, and clock/reset to a file target.** Each `diagram`
message carries a new `token` (counting diagrams shown) and a `target` token (counting
chip files the panel has targeted); `set`, `drill` and `reveal` carry back `token` and
are dropped if the diagram was replaced since (another chip, a reload, a drill). `clock`
messages (including Reset) carry `target`: they survive rebuilds of the same chip (so
Reset followed quickly by Step runs in click order) but are dropped after the panel
navigated to another file. The webview answers each `diagram` with `drawn`, which tests
wait for.

**Values are checked against the diagram.** A `chip.values` reply is used only if its
`revision` and `path` are the diagram's (and it has a value per net and part). If its
revision is newer, the chip was built again (a reload shared with the simulator panel):
the panel fetches the new diagram instead. The webview applies the same check and drops
values that do not fit the diagram it shows.

## Session sharing

If the chip simulator panel and the circuit panel show the same `.hdl` file, they use
one runtime chip session: flipping an input or clocking in either one updates both.

### The hub

`chipSessions.ts` keeps one **hub** per chip file (`ChipSessions` is the registry,
`ChipHub` the hub). The file does not import `vscode`; `src/test/unit/chipSessions.test.ts`
drives it with a fake simulator (`src/test/fakeRuntime.ts`). A panel joins the hub of its
file (`sessions.join(file, panel)`, synchronous), the last one to leave closes the session.
Switching chips is leaving one hub and joining another, at once.

The hub owns the session id, the current `revision`, the last **build error** and one FIFO
queue. **Every** runtime call for the session goes through that queue, from either panel,
one at a time: `state`, `table`, `set`, `setAll`, `eval`, `tick`, `tock`, `step`, `reload`
(after an edit), `reset` (the student's Reset), `diagram`, `values`
(`hub.request(panel, request)`). No panel talks to the runtime itself.

After each op the hub publishes **one event** to every member panel:

```ts
{ seq, key, origin, request, op, path?,            // seq: 1, 2, 3, … per hub, the order ops ran in
  outcome: { ok: true, value } | { ok: false, error: { message, code, revision? } },
  session, revision, buildError, halted }          // the hub's state after the op
```

`op` is the request's op, or the hub's own: `open` (the chip opened or reopened),
`lost` (the session is gone) and `halted` (recovery gave up). Panels update only from
these events, synchronously, in `seq` order. So:

- **Order is the engine's.** The simulator records its clock history from each clock
  op's own event (whoever clicked), so two quick steps in either panel are recorded as
  `1, 2`, never `2, 1`, however late another reply comes.
- **What a panel wants next is queued behind what caused it.** After a change event the
  circuit panel asks for `values` (one request at a time; a change while it waits only
  updates the cause); after a build event, for the `diagram` of the level it wants; the
  simulator asks for the truth table when the build changes. Each panel uses only the
  reply to its own latest request.
- **Build errors live in the hub.** A failed `reload`/`reset`/`open` sets `buildError`
  (the old circuit and revision run on); only a build that succeeds clears it. A clock op
  or an input never does, so both panels keep showing it.
- **Edits are built once.** `reload` does nothing if the edit generation (counted in
  `extension.ts` from edits and saves of `.hdl` files, read before the text is sent) has
  already been built; `reset` always builds. An edit made while the chip opens is built at
  once. A chip that did not build is not opened again until an edit or Reset.

### Guards

- **Inputs are bound to the rendered build.** Every `set` and `setAll` requires an
  `expect: {session, revision}` captured from the view the student actually saw (sent
  back by the simulator webview, or mapped from the circuit diagram's `token`); the hub
  checks it and forwards `expectRevision` to the engine (the session must match too, since
  a reopened session restarts revisions from 1). A click made on build N that arrives
  after a rebuild to N+1 fails with `stale-revision` and sets nothing; the circuit panel
  shows a non-modal "The chip was rebuilt; click again." note (preserved across redraws)
  and refreshes the diagram, and the simulator shows per-pin rejection feedback for both
  1-bit and bus pins (or a non-modal note for a stale truth-table row), preserved across
  redraws until the next edit of that pin or a short timeout.
- **Explicit drills and breadcrumbs are bound to the rendered build.** Explicit drill and
  breadcrumb clicks send `diagram` with `expect: {session, revision}` → `expectRevision`
  at the queue head, without mutating `path` prematurely. If a rebuild raced ahead while
  the replacement diagram was still pending, the stale drill fails with `stale-revision`,
  never opens a different part at the same index, refreshes the current level, and shows
  "The chip was rebuilt; click again." Automatic refreshes remain unguarded and keep
  walking up on `invalid-path`.
- **Clock ops and Reset carry a file-target token.** They are not bound to a build or a
  diagram, so Reset followed quickly by Step runs in click order across rebuilds of the
  same chip, but both panels reject clock/reset messages (and inputs) from an older file
  target after Follow Editor switches the panel to another chip.
- **Diagram tokens** still bind `set`, `drill` and `reveal` to the diagram they were
  clicked on (they name pins, parts and nets of it); the panel drops them if it has drawn
  another diagram since. The webview still answers each diagram with `drawn`.
- **Queue coalescing and backlog bound.** Before enqueueing, the hub coalesces superseded
  pending reads (`state`, `table`, `values`, and unguarded `diagram`) per `origin` +
  `path` so only the latest pending read runs, and coalesces duplicate pending `reload`s
  into a single queued rebuild. Pending user actions (`set`, `setAll`, `eval`, `tick`,
  `tock`, `step`, `reset`, and explicit drills) are bounded (`MAX_QUEUED_ACTIONS = 16`):
  accepted actions keep strict FIFO order, and excess actions fail immediately with `busy`
  and surface as a short non-modal note in the asking panel.
- **Values must fit** the diagram: same session, `revision` and `path`, and a value per net
  and part (`valuesFit`, also checked in the webview). Values of another build bring the
  new diagram instead.
- **Drill paths survive transport failures.** If the session goes away (worker stopped,
  `session-ended`, engine replaced) while an in-flight drill or refresh runs, the panel
  keeps its path and asks again once the hub has reopened the chip. Only `invalid-path`
  from the current build shortens it on automatic refreshes: one level up and ask again,
  down to the deepest level that still exists (or the root). Values that come back
  `invalid-path` do the same, so the panel is never left greyed.
- **Source offsets need the same text.** `chip.diagram` reports the `sourceHash` of the
  text it was built from. Reveal captures its diagram and spans before awaiting editor
  calls, rechecks the diagram `token` (and disposal) after both `openTextDocument` and
  `showTextDocument`, and compares the document's FNV-1a hash against the *captured*
  diagram's `sourceHash` (`sourceHash.ts`, tested against the vector above), never a newer
  diagram's; otherwise it falls back to the span's lines.

### Recovery

The runtime client fires `onDidStop` when the worker stops, when it replaces a trapped
engine (a `crashed: true` reply; fired before that reply is delivered), **and** when a
single `call`/`tryCall` exceeds its deadline (`CALL_TIMEOUT_MS = 10_000`; streaming
budgeted ops `runTest` and `runMachine` are not subject to the single-call deadline). On
timeout, the client invalidates and terminates the hung worker, fires `onDidStop`, and
settles all pending calls on that worker with a transport error. All three go through the
same path: `ChipSessions` invalidates every hub (event `lost`), and each hub whose chip
was open reopens it (event `open`, restoring the last known non-zero top-level inputs via
a guarded `setAll` on the new build, and bringing both panels' state and diagram back with
the drill path kept). An op whose reply says the session ended without a stop drops the
session too; the next op reopens it.

Automatic recovery is **bounded**: at most 3 automatic restarts within 60 s per runtime
(`MAX_RESTARTS`, `RESTART_WINDOW_MS`). A fourth stop in the window halts every hub (event
`halted`): ops fail at once with `halted`, without touching the runtime, and both panels
show the error with a **Retry** button — including alongside a retained `buildError` so a
failed build followed by a runtime halt still offers Retry. Recovery and Retry are
distinct queue jobs: a queued automatic recovery rechecks the halt at execution time so it
never exceeds the restart budget, and Retry lifts the halt only when its job reaches the
queue head (earlier queued ops still fail as `halted`). A later edit or Reset also tries
once; if the worker stops again within the window, the hubs halt again. A worker that
cannot start (or hangs) is started four times, then waits. Disposing `ChipSessions` tears
down every hub through the same close path as the last member leaving, draining queued
work and closing late opens.

### What the earlier fixes guaranteed, and where it is now

- *An event names its session*: subsumed. Events come from the hub of one file to its
  members only; a panel that switched chips has left that hub.
- *A clock step carries its state*: kept, as the event's outcome, now in `seq` order.
- *A Reset carries its outcome*: subsumed by the hub's `buildError`.
- *Edits during an open are not lost*: kept (generation read with the text sent).
  Document versions are no longer tracked: `sourceHash` replaces them.
- *Recovery checks who is still there*: kept. Ops of a hub that everyone left resolve with
  nothing; a session opened for nobody is closed. A failed overlay sync is not taken for a
  build error.
- *Session numbers are never reused*: kept (`firstSession` in `runtime/protocol.ts`).
- Diagram tokens, `drawn`, values-fit checks and version-checked reveal: kept, the reveal
  check now by `sourceHash`.
- The crash hook for tests is `testHooks.crashWorker()` on the extension's exports, present
  only when `context.extensionMode` is `ExtensionMode.Test`; `Runtime` has no such method.

## Panel decisions (added with the panel)

- **Following the editor** uses the simulator's setting, `nandwright.simulator.followEditor`.
  Switching to the file of a chip that is drilled into does not retarget the panel.
- **Settings**: the radix chosen in the toolbar is kept in workspace state; the Animate
  toggle writes the user setting `nandwright.circuit.animate`. `reducedMotion` is sent when
  `workbench.reduceMotion` is `on`; otherwise the webview's media query decides.
- **Drill path on reload**: if the path no longer names a composite part (`invalid-path`),
  the panel goes up one level at a time until it does. A session that went away keeps it.
- **A rejected input** (a value that does not fit) shows as a warning notification; the
  diagram is not greyed.
- **Reveal** opens the file in the editor group that is not the panel's, and does not take
  focus from the panel. Spans are byte offsets; they are converted for the editor, and if
  the text is not the text built (its hash is not `sourceHash`), the span's lines are
  selected instead.
- **Cursor highlight** sends the part whose statement holds the cursor and, if the word
  under it names a net of the shown level (not a pin name before `=`), that net too.

## Webview decisions (added with the webview)

Choices the contract left open, as `extension/src/webview/circuit*.ts` makes them:

- **Files.** `circuitTypes.ts` (the types above), `circuitLayout.ts` (pure layout),
  `circuitSymbols.ts` (gate shapes, pure), `circuitStyle.ts` (`CIRCUIT_STYLE`, to pass to
  `panelHtml` as the panel style), `circuit.ts` (entry, bundled to `dist/webview/circuit.js`).
  The panels' shared base style moved from `panelHtml.ts` to `panelStyle.ts` (no `vscode`
  import) so the preview harness can use it; `panelHtml` is unchanged in behaviour.
- **Breadcrumb numbers** are 1-based, as a student counts part statements: path `[2]` shows
  as `Mux16 part 3`. Tooltips and aria labels count the same way.
- **Layout depends on the radix**: input/output ports and bus value labels are sized for the
  widest value in the chosen radix, so switching dec/bin/hex re-lays out (keeping pan and
  zoom). For a given chip and radix the drawing is deterministic, and values never move it.
- **Cycle cutting** uses `deps`: an edge inside a cycle into an input that no output of that
  part depends on (`deps`) is cut first; if a part sends no `deps`, `sequential` decides.
  Then DFS back edges. Cut edges are drawn as wires that run back left between the layers.
- **Several connections on one pin** (`a[0..7]=x, a[8..15]=y`), different slices of one
  output pin (`out=x, out[0..3]=y`) and an output port fed by several slices each get their
  own row on the pin, so wires never share an end point. An output pin feeding several nets
  with the same bits (`out=a, out=b`) is one row: the wires leave it together and part at a
  junction dot. Slice labels: `[0..7]` is the net's bits, `→[8..15]` the pin's.
- **Partly driven nets** (`out=x[3]`): the wires of that driver carry and show only its bits.
- **Animation**: changed wires light in layout order at 45 ms per half-layer (column, then
  channel: about 90 ms a layer); wires that run back are timed by their distance from the
  driver. Output ports update when their wire arrives; input ports at once. The `cause` of
  a `values` message is not used yet.
- **Keys** beyond the spec: Enter/Space on a focused input port flips or edits it, Enter on
  a built-in part reveals it, Backspace goes up one level, Escape clears the highlight.
- **Export** sends the whole current level (not the visible viewport) with the editor
  background colour, no hover or highlight; colours are resolved to `#rrggbb`/`rgba()`.
  PNG is 2× unless that would pass the canvas limits: it is scaled down so that no side is
  over 16,384 px and the area is at most 2^28 px (`circuitExport.ts`), with a note saying
  so. The size is taken from the layout before the image decodes, and the result must be
  PNG data; otherwise the export fails with a message shown to the student.
- **Leads of a pin with several connections** (`a[0..7]=lo, a[8..15]=hi`) each show only
  their own bits (`PinGeom.slices`), not the whole pin.
- **Webview `error` messages** are also sent for uncaught script errors (`window.onerror`).

### Preview harness

`npm run preview:circuit` (or `node scripts/circuit-preview.mjs --out <dir>` after a build)
writes static pages that run `dist/webview/circuit.js` against a fake extension host
(`src/test/circuitPreviewHost.ts`, simulating made-up chips from
`src/test/circuitFixtures.ts`) in dark, light and high-contrast colours, and takes
Playwright screenshots, including hover, animation, inline edit, error, drill and export.
Open `<dir>/index.html` to click around. `--no-shots` skips the screenshots.
