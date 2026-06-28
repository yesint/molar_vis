# molar_vis_js

The **wasm-bindgen JavaScript API** for the molar_vis viewer — the web half of the
dual-host scripting plan. It mirrors the native Python module (`molar_vis_py`) almost
line-for-line, so the same script reads nearly identically in Python and JavaScript:

```js
import init, { start, System } from "./pkg/molar_vis.js";
await init();
const vis = start("molar_vis_canvas");
const sys = System.from_bytes("p.pdb", new Uint8Array(await (await fetch("p.pdb")).arrayBuffer()));
const mol = vis.add_mol(sys);
const rep = mol.add_rep(sys.select("protein"), "cartoon", "ss");
rep.style = "lines";                 // setters apply live
vis.rotate(30, 15);
vis.projection("perspective");
vis.background_gradient([0.02, 0.02, 0.06], [0.10, 0.12, 0.20]);
```

## Build

The crate is a `cdylib` whose content is gated to `wasm32`, so a plain native
`cargo build` of the workspace compiles it to an empty library. Build the importable ES
module with [wasm-pack](https://rustwasm.github.io/wasm-pack/):

```sh
wasm-pack build crates/molar_vis_js --target web --release --out-dir web/pkg
```

That emits `web/pkg/molar_vis.js` + `molar_vis_bg.wasm` + `molar_vis.d.ts`. A host page
imports them as an ES module (`--target web` needs `await init()` before any call and
must be served over HTTP, not `file://`).

## Demo

`web/index.html` is both the GitHub Pages demo and the canonical usage example: it drives
the viewer entirely through this public API (fetch `2lao.pdb` → `System.from_bytes` →
`add_mol` → `add_rep`). To run it locally:

```sh
wasm-pack build crates/molar_vis_js --target web --out-dir web/pkg
cp tests/2lao.pdb crates/molar_vis_js/web/2lao.pdb     # the demo fetches ./2lao.pdb
python -m http.server -d crates/molar_vis_js/web 8000  # ES modules + wasm need HTTP
# open http://localhost:8000/ ; the browser console logs mols/reps counts
```

## Scope (v1)

Viewer control only — load-from-bytes, selection, `add_mol`/`add_rep`, the
style/color/material setters, and the view controls. Coordinates are **static after
load** (no live JS-driven coordinate edits yet). One viewer per page. molar's
analysis/coordinate API is not exposed.

## Architecture

Like `molar_vis_py`, the handles push [`AppJob`](../molar_vis_core/src/app.rs) closures
onto a channel drained at the top of `App::ui`; the viewer renders the JS-owned
`System` by reference through a `WebSystemSource` (a `SharedSource` over an `Rc<System>`
— safe borrows, no raw pointers, since the browser owns its own data and runs
single-threaded).
