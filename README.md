# molar_vis — molecular visualization and editing in Rust

[![Rust](https://img.shields.io/badge/rust-1.85+-blue.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Artistic--2.0-blue.svg)](#license)
[![Built on MolAR](https://img.shields.io/badge/built%20on-MolAR-orange.svg)](https://github.com/yesint/molar)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20WASM-green.svg)](#building)

molar_vis is a molecular visualization and editing program written in Rust. It is
very fast and handles small molecules and large biomolecular systems equally well.
It combines workflows for MD trajectory analysis, computational chemistry and drug
discovery: inspecting simulation trajectories, editing molecular structures,
comparing docking poses and preparing molecular images.

The viewer uses VMD-style atom selections and supports the usual molecular
representations, including cartoons and molecular surfaces. The molecular view,
selections and representation controls are kept in a single window. A built-in
GPU ray tracer renders images with ambient occlusion, shadows and optional global
illumination.

molar_vis is built on [MolAR](https://github.com/yesint/molar), which provides
molecular file handling, atom selections and analysis algorithms. The interface
uses [egui](https://github.com/emilk/egui), and rendering uses
[wgpu](https://github.com/gfx-rs/wgpu). It is available as a desktop application and
also runs in the browser through WebAssembly.
[Try the browser version online](https://yesint.github.io/molar_vis/).

Python bindings combine the desktop viewer with MolAR's analysis tools, allowing
changes to molecular coordinates to appear directly in the viewer. A JavaScript
API provides programmatic control of the browser viewer.

![molar_vis](https://raw.githubusercontent.com/yesint/molar_vis/main/docs/screenshot.png)

## Features

### Molecular visualization

Build a scene from several molecules and show different parts of each molecule
in different representations. For example, display a protein as a cartoon, its
binding site as sticks and the ligand as a molecular surface. Each representation
has its own selection, style, colors and material, and can be hidden, duplicated
or reordered independently.

| Representation | Typical use |
|---|---|
| Lines | An overview of bonds in a large structure |
| Licorice | Bonds and local geometry, including binding sites |
| Ball and Stick | Small molecules, atom identities and bond connectivity |
| VDW | Atomic packing and space-filling views |
| Cartoon | Protein folds and secondary structure |
| Surface | Molecular shape, cavities and solvent-excluded surfaces |

Cartoons support all-atom and coarse-grained proteins. Secondary structure can be
assigned with DSSP or the PyMOL-derived `dss` algorithm. Molecular surfaces have
adjustable probe radius, resolution and smoothing, so you can choose the level of
detail appropriate for the system.

Atom and bond rendering is GPU accelerated and supports systems with hundreds of
thousands of atoms. The scene is redrawn when something changes, rather than
continuously rendering an unchanged view.

### Selections and inspecting structures

Select atoms by molecular properties or geometry using MolAR's VMD/Pteros-style
selection language. The selection editor highlights syntax errors and suggests
chain names, residue names and available ranges. You can also select atoms or
whole residues by clicking or drawing a lasso in the molecular view.

The hover detail lens reveals atoms beneath a cartoon or surface without changing
the representation. This is useful for inspecting a side chain or a surface
pocket while keeping the surrounding structure visible. Zoom to a selection,
choose an atom as the center of rotation, or search for a view that exposes a
selected region with less obstruction.

### MD trajectories and periodic systems

Load a trajectory onto a structure, play it back or step through individual frames.
Playback speed, frame skipping and looping are adjustable. Trajectory smoothing
can reduce thermal jitter for visualization, and unwanted frames can be trimmed
or decimated. Align a trajectory to a reference structure or one of its own frames
and calculate RMSD.

Periodic boxes and neighboring cell images can be displayed alongside the system.
Optional periodic bond detection shows bonds across box boundaries, while cartoons
split at the boundary instead of drawing long connections across the cell.

Trajectory playback is available on the desktop and in the browser. Browser files
are read into memory, so available memory limits the size of trajectories you can
load there.

### Structure editing and computational chemistry

The basic structure editor lets you add or delete atoms and bonds, change bond
orders, toggle hydrogens and rotate dihedral angles. Geometry cleanup helps tidy
structures after editing. Changes can be undone and redone.

Color atoms by formal or partial charge to inspect charge distributions. The
desktop viewer can assign partial charges with Espaloma to molecules with suitable
bond information, such as ligands loaded from SDF/MOL files. Structural alignment
supports both matching atom lists and a common subset paired by atom names, with
RMSD reported after fitting or measured without moving the structures.

For custom analysis, the Python module provides MolAR's analysis tools and a live
view of the same molecular data. You can modify coordinates, perform measurements
and inspect the result in one Python or Jupyter session.

### Drug discovery and docking

Load a multi-record SDF file as a molecular group and browse its members using a
shared set of representations. The docking loader brings a receptor and ligand
poses into the same scene and sets up an interaction representation. Flexible
docking results can pair each ligand pose with its corresponding receptor
conformation.

Inspect hydrogen bonds, hydrophobic contacts, salt bridges, π-stacking, π-cation
contacts and halogen bonds between selected parts of a structure or between
separate molecules. Contacts appear as colored dashed lines, and detection cutoffs
can be adjusted for each interaction type.

### Colors, materials and images

Color molecules by element, chain, residue, atom index, secondary structure,
B-factor or charge, or choose a single color for a representation. In
**Edit → Settings → Colors**, edit element and secondary-structure colors, the
shared continuous gradient and the discrete palette. The continuous gradient has
editable endpoints and a movable midpoint; the default runs from blue through
white to red, with positive charges shown in red. The discrete palette starts with
18 colors and allows adding, removing and editing entries. Color-picker presets
use the same palette.

Materials range from opaque and transparent to glass, metal, flat outlines and
Molecular Nodes-style shading. Live previews help choose a material. Lighting,
ambient occlusion, shadows, depth cueing and solid or gradient backgrounds can be
adjusted independently, with perspective and orthographic camera options.

Press **R** in the viewport to ray-trace the current view, or use
**Render → Image…** to save a high-resolution image. Ray tracing refines the image
progressively and supports soft shadows, ambient occlusion and optional global
illumination. It requires a compute-capable GPU backend: native or WebGPU.
WebGL2 uses a capture of the interactive view for image export.

### Custom 3D meshes

Use a molecule's menu → **Load mesh…** to attach a PLY mesh with vertex colors and
normals. Display it as **Points**, **Wireframe** or **Surface**, with adjustable
point size, line width or surface smoothing, and choose a material just as for a
molecular representation. This allows externally generated geometry to be viewed
alongside the molecular structure and included in rendered images.

Meshes belong to a molecule, follow its visibility and remain static during
trajectory playback. They can be saved separately or included in a session.
Saving preserves the original geometry and vertex colors; display smoothing does
not modify the saved mesh. ASCII and binary PLY are supported. Mesh coordinates
must be in **nanometers**, matching MolAR's coordinate units.

### Settings, sessions and export

Settings control appearance, rendering, camera behavior, representation defaults,
materials and colors. Changes apply immediately; **Save** makes the current
settings the defaults for future runs. Desktop preferences are stored in a config
file, and browser preferences are stored locally in the browser.

On the desktop, save a session to restore molecules, representations, camera,
view settings, color preferences and custom meshes. Meshes are embedded in the
session, while source structure and trajectory files must remain available.
You can also export a molecule or a selected part of it at the displayed frame.
The browser supports image and PLY downloads; molecular file export and session
files are desktop features.

## Building

Install the published desktop viewer with a [Rust toolchain](https://rustup.rs):

```sh
cargo install molar_vis --version 0.2.2 --locked
molar_vis protein.pdb
```

To build from source:

```sh
git clone https://github.com/yesint/molar_vis
cd molar_vis
cargo build --release -p molar_vis
cargo run --release -p molar_vis -- protein.pdb
```

Load a structure and its trajectory together, or use `-m` to start another molecule:

```sh
molar_vis system.gro trajectory.xtc
molar_vis -m system.gro trajectory.xtc -m ligand.sdf
```

Dependencies come from crates.io; no sibling repository checkouts are required.
Supported molecular formats are provided by MolAR, including PDB, GRO, XYZ,
SDF/MOL and trajectory formats such as XTC, TRR and DCD. Availability of some
formats depends on the platform and enabled MolAR features.
See the [build and development reference](https://github.com/yesint/molar_vis/blob/main/docs/BUILD.md) for further details.

### WebAssembly — run it in the browser

The [online viewer](https://yesint.github.io/molar_vis/) opens with a sample
structure. Use **Open** to load your own files. It uses WebGPU where available,
with a WebGL2 fallback, and runs locally in the browser without uploading molecular
files to a server.

To build and serve the browser application locally with [Trunk](https://trunkrs.dev):

```sh
cargo install --locked trunk
cd crates/molar_vis_web
trunk serve
```

To embed a viewer in a web page and control it from JavaScript, use the separate
[JavaScript module](https://github.com/yesint/molar_vis/blob/main/crates/molar_vis_js/README.md).

## Scripting it from Python

The Python module re-exports MolAR's Python bindings, so one import provides both
molecular analysis and viewer control. The viewer window runs separately from the
Python prompt, and coordinate changes appear live without an explicit refresh.

```python
import molar_vis as mv

system = mv.System("protein.pdb")
vis = mv.spawn()
mol = vis.add_mol(system)
mol.add_rep(system("protein"), style="cartoon", color="ss")

system("name CA").translate([1.0, 0.0, 0.0])
```

This example is intended for an interactive Python or Jupyter session. In a plain
script, keep the process alive while using the viewer. The Python viewer currently
supports Linux and Windows; macOS support is pending.

See the [Python guide](https://github.com/yesint/molar_vis/blob/main/docs/PYTHON.md) for installation, examples and the API
reference, including meshes, colors and camera controls. The
[JavaScript API](https://github.com/yesint/molar_vis/blob/main/crates/molar_vis_js/README.md) exposes browser viewer controls;
it does not expose MolAR's full analysis API or live coordinate editing.

An optional in-app [Rhai](https://rhai.rs) console is available when building with
`--features scripting`.

## Selections

Selections use MolAR's syntax. For example:

```text
all
protein and not hydrogen
backbone and chain A
resid 1:50 and name CA
resname ALA GLY
water
within 0.5 of (resname LIG)
```

Distances are in nanometers. See the
[selection language reference](https://yesint.github.io/molar_vis/selection-language.html)
for copyable examples and explanations of property, geometric and periodic
selections. It is also available through **Help → Selection language**.

## Navigation

The molecular view uses VMD-style mouse controls:

| Action | Mouse |
|---|---|
| Rotate | Left drag |
| Roll | Shift + left drag |
| Pan | Right or middle drag |
| Move along the view axis | Shift + right drag |
| Zoom toward the cursor | Scroll wheel |

## Development and status

molar_vis is under active development. The desktop application targets Linux,
Windows and macOS; the browser version shares its visualization and editing core.
Platform differences are described above, particularly for Python, file handling
and GPU ray tracing.

The workspace contains the core library, desktop and browser applications, and
Python and JavaScript bindings. For implementation details, see the
[architecture](https://github.com/yesint/molar_vis/blob/main/docs/ARCHITECTURE.md), [module overview](https://github.com/yesint/molar_vis/blob/main/docs/MODULES.md) and
[build reference](https://github.com/yesint/molar_vis/blob/main/docs/BUILD.md). For molecular analysis in Rust or Python,
see [MolAR](https://github.com/yesint/molar).

## License

Distributed under the Artistic License 2.0, the same license as MolAR.

## Acknowledgements

Inspired by [VMD](https://www.ks.uiuc.edu/Research/vmd/), particularly its
representations, selection language and navigation. Built with
[egui](https://github.com/emilk/egui), [wgpu](https://github.com/gfx-rs/wgpu) and
[MolAR](https://github.com/yesint/molar).
