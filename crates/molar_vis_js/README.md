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

## Live scene listings

Use `await vis.mols()` and `await mol.reps()` to list the current scene, including
objects added in the viewer UI. These methods replace the old synchronous `mols`
and `reps` properties. Handles use stable identities, so sidebar reordering does
not change which object a handle controls. A deleted object's handle becomes
stale instead of referring to another object.

```js
for (const molecule of await vis.mols()) {
  for (const representation of await molecule.reps()) {
    representation.color = "element";
  }
}
```

## Selection, contacts, and camera

| Call | Effect |
|---|---|
| `vis.focus(mol, sel)` | frame a selection from `sys.select(...)` |
| `vis.rotation_center(x, y, z)` | set the orbit pivot in nm without moving the view |
| `vis.unobstructed_view(rep, zoom_out, resolution)` | search for a view that exposes the most target atoms |
| `vis.unobstructed_view_multi(reps, zoom_out, resolution)` | use a nonempty array of representations as one target |
| `mol.add_interactions(partner_rep)` | add a contact representation against another representation |
| `rep.width = pixels` | set Lines or Interactions width (at least 1 pixel) |

For example, `vis.unobstructed_view(rep, 1.0, 256)` searches 256 coarse directions
and frames the target. A zoom-out factor of 2.0 doubles its framed extent. Other
visible representations act as occluders. The multi-target form can combine
representations from different molecules. Supply both numeric arguments explicitly
in JavaScript; Python supplies defaults. Contact partners in JavaScript must be
`RepHandle` values (Python also accepts a molecule and uses its first representation).

## Custom meshes and colors

Attach a PLY mesh to a molecule with `mol.add_mesh_ply(name, Uint8Array)`.
Coordinates are in nm. `await mol.meshes()` returns the current mesh handles. Their `visible`, `material`,
`style` (`"points"`, `"wireframe"`, `"surface"`), `point_size`, `line_width`, and
`smoothing` properties control the same state as the GUI. Sizes are 1–10 pixels;
smoothing is 0–5 and applies to Surface. `await mesh.ply_bytes()` returns the original
geometry, normals, and vertex colors as binary PLY; `mesh.delete()` removes it.
`JSON.parse(await mesh.details_json())` returns the current name, visibility,
material, style, sizes, smoothing, vertex count, and triangle count. Mesh setters
queue changes; these readbacks include edits made through the GUI.

```js
const ply = new Uint8Array(await (await fetch("surface.ply")).arrayBuffer());
const mesh = mol.add_mesh_ply("surface.ply", ply);
mesh.style = "wireframe";
mesh.line_width = 2;
vis.set_element_color(6, new Uint8Array([90, 90, 90, 255]));
vis.set_continuous_gradient(new Uint8Array([
    40, 80, 230, 255,       // low
    255, 255, 255, 255,     // middle
    230, 40, 40, 255,       // high
]), 0.4);
```

Use `vis.set_color_preset(name)` to apply **Default**, **VMD**, **PyMOL**,
**Discovery Studio**, **MOE** or **Mol\*** element, secondary-structure and continuous
colors together. It retains the discrete palette. `await vis.color_preset()` reads
the active preset name, or `Custom` after editing its colors. Preset colors are saved
with the normal color preferences.

Color methods use RGBA8 (`Uint8Array(4)`); the gradient takes three concatenated
colors (`Uint8Array(12)`) and a midpoint from 0.01 to 0.99.
`vis.reset_element_color(number)` removes an override.
`vis.set_secondary_color(index, rgba)` edits one of nine classes, ordered α helix,
3₁₀ helix, π helix, β sheet, β bridge, turn, bend, polyproline, coil.
`vis.set_palette_color(index, rgba)` edits a discrete palette entry.
`vis.set_palette(colors)` replaces the palette with a nonempty flat RGBA8 array;
`vis.add_palette_color(rgba)` and `vis.remove_palette_color(index)` change its length
while retaining at least one entry. `vis.reset_colors()` restores factory colors.
`await vis.colors_json()` returns a JSON string with the current preferences.
Changes apply live. `await vis.save_settings()` explicitly saves startup defaults to
browser localStorage. Readback methods are asynchronous because queued edits run on
the viewer's update loop.

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
style/color/material setters, custom meshes, shared color preferences, and view controls. Coordinates are **static after
load** (no live JS-driven coordinate edits yet). One viewer per page. molar's
analysis/coordinate API is not exposed. Browser mesh I/O uses byte arrays; native
Python additionally offers path-based mesh loading/saving, file rendering, and session
saving. Browser rendering and downloads remain available through the GUI.

## Architecture

Like `molar_vis_py`, the handles push [`AppJob`](../molar_vis_core/src/app.rs) closures
onto a channel drained at the top of `App::ui`; the viewer renders the JS-owned
`System` by reference through a `WebSystemSource` (a `SharedSource` over an `Rc<System>`
— safe borrows, no raw pointers, since the browser owns its own data and runs
single-threaded).
