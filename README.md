# molar_vis — a modern molecular viewer in pure Rust

[![Rust](https://img.shields.io/badge/rust-1.83+-blue.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Artistic--2.0-blue.svg)](#license)
[![Built on molar](https://img.shields.io/badge/built%20on-molar-orange.svg)](https://github.com/yesint/molar)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20WASM-green.svg)](#building)

A modern, **legacy-free** molecular visualizer modeled after [VMD](https://www.ks.uiuc.edu/Research/vmd/),
written in **pure Rust**. The molecule is drawn with hand-written **GPU ray-cast
impostors** (WGSL) and real-time cartoon ribbons — no OpenGL fixed pipeline, no X11,
no C/C++/Tcl. It runs natively on Linux, Windows, macOS and compiles to WebAssembly
([try it now online](https://yesint.github.io/molar_vis/)).

The binary is called `molar_vis`. It is built on [molar](https://github.com/yesint/molar)
— a Rust molecular-analysis library by the same author — for file I/O, atom selections,
topology and secondary-structure assignment, and renders on
[`eframe`/`egui`](https://github.com/emilk/egui) + [`wgpu`](https://github.com/gfx-rs/wgpu).

Beyond the basics it leans into the **niceties that make a viewer pleasant to live in**:
screen-space **ambient occlusion** and real-time **cast shadows**, VMD-style **depth cueing**,
**order-independent transparency**, **gradient backgrounds**, a material picker with **live
previews**, periodic-image replication with dashed wrap-around bonds, and a **hover lens** that
quietly reveals the atoms tucked under a cartoon ribbon or a molecular surface. A settings dialog
remembers how you like things between runs.

![molar_vis](docs/screenshot.png)

## Why another molecular viewer?

VMD is the gold standard for molecular visualization, but it carries three decades
of accumulated legacy: a C/C++ core glued together with Tcl, an OpenGL fixed-function
rendering path, and a build that fights modern toolchains and platforms. `molar_vis`
is a clean-room reimagining of the *good parts* of VMD on a modern foundation:

- **Pure Rust, end to end** — memory-safe, no segfaults, no manual memory management,
  one `cargo build`.
- **A modern GPU pipeline** — atoms and bonds are ray-cast as analytic impostors in
  fragment shaders that write true depth, so they occlude perfectly and stay crisp at
  any zoom; cartoon ribbons are a real triangle mesh that interleaves with them. No
  fixed-function OpenGL, no immediate mode.
- **Portable by construction** — `wgpu` targets Vulkan, Metal, DX12 and WebGPU, so the
  same code runs on the desktop and in a browser.
- **VMD muscle memory preserved** — the same selection language, the same mouse
  navigation, the same representations and coloring schemes.

It is deliberately small and focused: a fast, beautiful, hackable viewer, not a
reimplementation of every VMD feature.

## Features

**Representations** — six styles, all GPU-accelerated:

| Style | Rendering | Notes |
|---|---|---|
| **Lines** | 1‑px GL lines | the lightweight default, VMD-authentic |
| **Licorice** | cylinder + sphere impostors | uniform bond radius |
| **Ball and Stick** | sphere + cylinder impostors | scaled VDW balls + thin sticks |
| **VDW** | sphere impostors | space-filling, true van der Waals radii |
| **Cartoon** | indexed triangle mesh | secondary-structure ribbons (see below) |
| **Surface** | grid + Surface Nets mesh | solvent-excluded (rolling-probe) surface (see below) |

**Cartoon ribbons** — a faithful port of VMD's *NewCartoon*: a per-chain
modified Catmull–Rom spline through the Cα trace, a carbonyl-derived ribbon frame with
running-average orientation, elliptical cross-sections that morph by secondary-structure
class, and β-strand arrowheads. Secondary structure is assigned by molar's built-in
**DSSP** (Kabsch–Sander) or **`dss`** (a clean-room port of PyMOL's algorithm),
selectable per representation.

**Molecular surface** — the **solvent-excluded (rolling-probe) surface**, computed the
robust way modern tools (PyMOL/Chimera/EDTSurf) use: a distance field on a grid (the SAS
solid, then a Felzenszwalb–Huttenlocher distance transform carving the probe) is contoured
with **Surface Nets** for a watertight, smooth mesh — no fragile analytic patch stitching.
Per-rep **probe radius**, **grid resolution** and **smoothing** controls; scales to 100k+
atoms. (molar's PowerSASA backend also exposes exact SASA areas + analytic SAS/SES meshes.)

**Materials** — eleven VMD-style presets (Opaque, Transparent, Glass, Translucent, Ghost,
Glossy, Diffuse, Metal, and the ambient-occlusion trio AOChalky / AOShiny / AOEdgy): per-rep
ambient / diffuse / specular / shininess (Blinn-Phong) plus **opacity** and a silhouette
**outline** (AOEdgy), with **order-independent transparency** (weighted-blended OIT) so
overlapping translucent surfaces blend correctly without sorting. The material picker isn't a
plain list — it's a **grid of live previews**, each a little shaded molecule rendered with that
material, so Glossy, Metal, Glass and the matte AO presets read at a glance.

**Trajectories** (native) — load multi-frame trajectories (xtc/trr/dcd/gro/multi-MODEL pdb)
into a molecule with a VMD-style playback bar (first / step / play-pause / step / last,
editable frame field, loop, fps) and a frame slider; sync or background-async loading.
Frame changes are zero-copy (rendered by reference) with incremental GPU updates. Per-representation
**trajectory smoothing** (Savitzky–Golay) damps thermal jitter on the fly, and loaded frames can be
trimmed or decimated.

**Coloring schemes** — Element (CPK), Chain, ResID, ResName, Index, B-factor,
Secondary structure, and **Solid** (a custom color you pick), each with a drawn descriptive
icon in the picker.

**Selections** — molar's VMD/Pteros-style selection language: `protein`, `backbone`,
`water`, `name CA`, `resid 1:50`, `chain A`, `within 5.0 of ...`, and much more.
Selections are compiled once and re-evaluated only when needed. The selection field helps you as
you type: it **paints the erroring span of an invalid selection red, in place**, and shows a hint
of the **available chains, residue names and id/index ranges** for the keyword you're on.

**Picking, lasso & the hover lens** — flip the top-bar **Sel. mode** to explore atoms with the mouse:

- **Hover** — point at an atom for its identity and *real* coordinates with a glowing outline ring
  (or a whole-residue highlight in *Residues* scope). Native picking uses a GPU id-buffer, so it
  stays instant on huge systems.
- **Lasso** — draw a freehand loop to grab atoms (Shift adds, Ctrl subtracts). The catch is staged
  as a *pending* selection that **glows in each representation's own style** — lasso a helix shown as
  cartoon and the **ribbon itself** lights up, not dots over it — with a one-click accept/discard. A
  **scope** dropdown grows every pick to whole **Residues** or heavy-atoms-plus-their-**H**.

…and the star of the show, the **hover detail lens**: hover over a cartoon ribbon or a molecular
surface and the atoms hiding *underneath* — the chemistry those styles abstract away — fade into view
as a small CPK ball-and-stick "lens" that tracks your cursor down the line of sight. It reveals the
camera-facing residues right where you're looking (not just the single nearest atom, so it works in
ribbon gaps and surface dimples too) and dissolves softly back in as you move on — so you can read the
underlying structure without ever switching representation.

**Rendering options** — collected in the **view-settings menu** (the hamburger on the right of
the top bar, with **Camera / Lighting / Scene** tabs):
- **Projection** — perspective or orthographic (orthographic is the default).
- **Depth cueing** (fog) — fades distant geometry toward the background, with three VMD-style
  falloff curves: **Linear**, **Exp**, **Exp²** (plus *None*), and Strength / Start controls.
- **Ambient occlusion** (SSAO) — a screen-space pass that darkens creases and contact points.
- **Cast shadows** — real-time directional shadows from a key light (shadow mapping), applied
  deferred so they cost one extra geometry pass.
- **Background** — a flat color or a vertical **gradient** (top / bottom color pickers).
- **Orientation axes** gizmo (VMD-style), in any viewport corner.
- **Anti-aliasing** — supersampling (configurable 1–4×, default 2×; smooths the ray-cast impostor
  silhouettes that MSAA can't touch), with idle frames costing **zero GPU**.

**Camera & display**
- Quaternion arcball camera with VMD mouse mapping (rotate / roll / pan / dolly / zoom-to-cursor).
- Zoom-to-selection and zoom-to-molecule; per-molecule periodic-box wireframe.
- **Periodic images** — replicate any representation across ±a/±b/±c lattice cells (drawn *by
  reference*, no data duplicated), with the box wireframe. Enable **periodic bond search** (a
  setting; off by default since it's slower on large structures) and covalent bonds that wrap across
  a box face are found and drawn as **dashed minimum-image half-bonds**, with cartoon ribbons
  splitting cleanly at the boundary rather than streaking across the cell.

**Scene**
- Multiple molecules, each with multiple representations.
- Per-representation selection, style, color and material, with a tabbed settings panel
  (**Style** / **Traj** / **Periodic**) for per-rep tunables.
- Drag-to-reorder representations; duplicate; show/hide.
- Full **undo/redo** with named history (Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y).

**Settings, sessions & files**
- **Program settings** — a cogwheel in the toolbar opens a tabbed window
  (*Appearance / Rendering / View / Representations / Behavior*) for everything that used to be baked
  in at launch: UI theme + font scale + accent color, anti-aliasing and shadow-map quality, the
  default projection / background / lighting / representation / coloring for *new* scenes, mouse
  sensitivity, trajectory playback defaults, and bond detection (thresholds, **periodic search**,
  and the dashed wrap-around-bond toggle). App-wide tweaks (theme,
  anti-aliasing) apply live; the rest seed the next scene you open. Everything **persists to a JSON
  file** in your platform's config directory, written with sensible defaults on first launch.
- **Sessions** — save and reload the whole visualization state (molecules by source path, every
  representation, the camera and all view settings) as a single JSON session.
- **Export** (native) — write a molecule, or just one representation's selection, back out to
  PDB / GRO / XYZ at the displayed frame.

**Efficient by default** — the scene is only re-rendered when geometry, the camera, the
viewport size or visibility actually change. **Idle costs zero GPU**; the UI repaints on
input. The impostor pipeline scales to hundreds of thousands of atoms.

## Building

With a [Rust toolchain](https://rustup.rs) installed — `molar` and its `powersasa`
dependency are pulled directly from GitHub, so no sibling checkouts are needed:

```sh
git clone https://github.com/yesint/molar_vis
cd molar_vis
cargo build --release
```

Run it, passing one or more structure files (each file becomes one molecule):

```sh
cargo run --release -p molar_vis -- protein.pdb            # one molecule
cargo run --release -p molar_vis -- system.gro ligand.pdb  # two molecules
```

Supported input formats are whatever molar reads — including **PDB**, **GRO**, and
(with the appropriate molar features) trajectory and topology formats.

### WebAssembly — run it in the browser

The viewer also runs in the browser (single-threaded WebAssembly, WebGPU with an
automatic **WebGL2 fallback**). **Try the live demo: <https://yesint.github.io/molar_vis/>**
— it opens to a sample structure; use **Open** to load your own `.pdb`/`.gro`/`.xyz`.

Build and serve it locally with [Trunk](https://trunkrs.dev):

```sh
cargo install --locked trunk     # once
cd crates/molar_vis_web
trunk serve                      # then open the printed http://127.0.0.1:8080
```

The browser crate (`crates/molar_vis_web`) renders through `eframe`'s `WebRunner` and
reads files in memory via molar's `FileHandler::from_reader` — no server needed, so it
hosts on any static site. It's deployed to GitHub Pages from `.github/workflows/pages.yml`
on every push to `main`. molar's parallelism (rayon) runs serially on wasm; trajectories load too —
the picked file's frames stream into the viewer incrementally (no threads). File **export** and the
on-disk **settings/session** files are native-only (the browser has no filesystem).

## Selections

`molar_vis` uses molar's selection language directly. A few examples:

```
all
protein and not hydrogen
backbone and chain A
resid 1:50 and name CA
resname ALA GLY
water
within 5.0 of (resname LIG)
```

See the [molar selection-syntax reference](https://github.com/yesint/molar#selection-syntax)
for the full grammar (keywords, comparisons, geometric and distance-based selections,
logical operators, `same ... as`, PBC options, …).

## Navigation

Standard VMD-style mouse mapping inside the 3D viewport:

| Action | Mouse |
|---|---|
| Rotate (arcball) | left-drag |
| Roll (screen-plane) | Shift + left-drag |
| Pan | right-drag (or middle-drag) |
| Dolly (move along view axis) | Shift + right-drag |
| Zoom (toward the cursor) | scroll wheel |

## How it works

A few design points worth knowing if you want to hack on it:

- **"Strategy A" rendering.** The 3D scene is drawn into our *own* offscreen color +
  `Depth32Float` textures and then composited into the egui frame as an image. egui's own
  render pass has no depth attachment, so owning the targets is what gives us full depth
  control — required for the impostors.
- **Impostors.** Spheres and cylinders are camera-facing billboards; the fragment shader
  ray-casts the analytic surface and writes `frag_depth`, so geometry occludes correctly
  (against itself *and* the cartoon mesh) and never tessellates. Both perspective
  (eye-ray) and orthographic (parallel-ray) cameras are handled in the shader.
- **Depth cueing** is applied in every fragment shader from a shared camera uniform,
  fading geometry toward the background color by eye-space distance.
- **A dirty-flag scene graph.** N molecules × M representations; each rep owns a compiled
  selection and its GPU buffers. Selection recompilation and geometry rebuilds are driven
  by per-rep dirty flags, and the whole frame is skipped when nothing changed.
- **Units.** All geometry is in nanometers (molar's native unit) end to end.

The workspace is two crates: `molar_vis_core` (the WASM-safe library — all logic and
rendering) and `molar_vis` (the thin native binary: argv + logging).

## Status

**Works today (native on Linux/Windows/macOS, and in the browser):**
- Load one or more molecules; multi-molecule / multi-representation scenes.
- All six representations (Lines, Licorice, Ball-and-Stick, VDW, Cartoon, Surface).
- Every coloring scheme (incl. custom solid colors); the full molar selection language with
  in-field error highlighting and keyword suggestions.
- Eleven materials incl. order-independent transparency; perspective/orthographic; depth-cue modes;
  screen-space **ambient occlusion** and real-time **cast shadows**; solid/gradient background.
- **Trajectory** loading + VMD-style playback (smoothing, frame trim/decimate), on the desktop
  **and in the browser** (the wasm build streams frames in from the picked file incrementally).
- Atom **hover-info picking** and **lasso selection** (Atoms / Residues / Bound-H scopes, glowing
  active selection), plus the **hover detail lens** revealing the atoms under a cartoon/surface.
- **Periodic images** with dashed wrap-around bonds; per-molecule periodic-box wireframe.
- **Program settings** dialog with a persisted config file; save/load **sessions**; **export**
  molecules and selections to file.
- Undo/redo; drag-reorder reps; zoom-to-selection/molecule.
- **Browser build** — runs in the browser ([live demo](https://yesint.github.io/molar_vis/));
  WebGPU with a WebGL2 fallback; in-browser structure + trajectory loading.

**In progress / not done:**
- Browser trajectory loading reads the whole file into memory (fine for typical sizes); true
  random-access disk streaming for very large trajectories is a possible future addition.

This is a young project under active development; expect rough edges.

## Built on molar

`molar_vis` is a showcase for [molar](https://github.com/yesint/molar). Everything below
the rendering layer — reading files, building topology, guessing bonds, evaluating
selections, assigning secondary structure — is molar. If you want to *analyze* rather
than *view* molecules in Rust (or Python), start there.

## License

Distributed under the **Artistic License 2.0**, the same license as molar.

## Acknowledgements

Inspired by [VMD](https://www.ks.uiuc.edu/Research/vmd/) (Theoretical and Computational
Biophysics Group, University of Illinois) — its representations, selection language and
navigation are the model this project follows on a modern stack. Built with
[egui](https://github.com/emilk/egui), [wgpu](https://github.com/gfx-rs/wgpu) and
[molar](https://github.com/yesint/molar).
