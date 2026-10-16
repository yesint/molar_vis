# Using molar_vis from Python

The Python module combines the viewer with [MolAR's Python bindings](https://github.com/yesint/molar).
It works on Linux and Windows; the Python viewer is not yet supported on macOS.

## Installation

Build the module from a repository checkout using a Rust toolchain and
[maturin](https://www.maturin.rs). Activate a virtual environment first:

```sh
# From the repository root
python3 -m venv .venv
source .venv/bin/activate
pip install maturin numpy
cd crates/molar_vis_py
maturin develop --release
cd ../..
```

On Windows, activate the environment with `.venv\Scripts\activate` instead.
Re-run `maturin develop --release` after changing the Rust source.

## Working with a live viewer


Beyond the GUI, you can drive `molar_vis` from a **Python script or a Jupyter / IPython
notebook**: build molecules and selections with molar's full (numpy-backed) analysis API,
open a live viewer window, and keep working — the window runs on its own thread, so the
REPL never blocks, and it renders **directly from your pymolar objects** (zero-copy), so
edits show up without an explicit refresh.

```python
import molar_vis as mv

s   = mv.System("protein.pdb")      # a pymolar System — full analysis API (numpy)
sel = s("name CA")                  # a pymolar selection

vis = mv.spawn()                    # open the viewer; returns immediately
mol = vis.add_mol(s)                # render s — shared, no coordinates are copied
rep = mol.add_rep(sel, style="cartoon", color="ss")

sel.translate([1.0, 0.0, 0.0])      # move atoms in Python → the view updates live

for rep in vis.mols[0].reps:        # introspect the scene…
    rep.style = "lines"             # …property setters apply live
```

**Running it.** Activate the venv you built into, and run **from the repo root** so
relative paths like `tests/2lao.pdb` resolve. Interactively — IPython, Jupyter, or
`python -i` — the window stays open and updates as you type. In a plain
`python script.py` the process exits at the end and the window closes with it, so keep
it alive:

```python
import molar_vis as mv
s = mv.System("tests/2lao.pdb")
vis = mv.spawn(); mol = vis.add_mol(s)
mol.add_rep(s("protein"), style="cartoon", color="ss")
input("viewer open — press Enter to quit\n")   # or a loop / time.sleep(...)
```

Because the viewer shares the molecule's memory with Python, anything you do to it —
translate, superpose, edit coordinates, step a trajectory — is reflected on screen with
no copy and no manual redraw. The same objects carry molar's analysis API (RMSD, SASA,
DSSP, geometric selections, …), so you analyze and visualize in one session.

**API at a glance**

| Call | Effect |
|---|---|
| `mv.System(path)`, `s("protein")`, `rmsd(a, b)`, … | the re-exported **pymolar** API (analysis) |
| `mv.spawn() -> Visualizer` | open a viewer window (non-blocking) |
| `vis.add_mol(system) -> MolHandle` | render a molecule, zero-copy |
| `vis.mols` | all live molecules, including those opened in the UI or restored from sessions |
| `vis.load_session(path)` | load saved representations, colors, view settings and molecule sources |
| `mol.add_rep(sel=None, style=…, color=…, material=…) -> RepHandle` | add a representation |
| `mol.reps` | all live representations belonging to that molecule |
| `mol.visible = False` | hide a molecule; set `True` to show it |
| `vis.load_group(path)` / `vis.groups` | load or list molecular groups with shared representations |
| `group.members` / `group.reps` / `len(group)` | inspect a group's members and shared representations |
| `group.current = index` | switch the shown member without moving the camera |
| `group.add_interactions(partner)` | add one shared contact representation that follows the shown member |
| `rep.style = …` · `rep.color = …` · `rep.material = …` | live property setters |
| `rep.select(sel)` | set a representation's selection from a pymolar `Sel` |
| `rep.width = pixels` | set Lines or Interactions width (at least 1 pixel) |
| `mol.add_interactions(partner)` | add contacts against a representation, or a molecule's first representation |
| `vis.focus(mol, sel)` | frame a selection |
| `vis.unobstructed_view(rep, zoom_out=1.0, resolution=256)` | search for a clear view that maximizes visible ligand area |
| `vis.unobstructed_view_multi(reps, zoom_out=1.0, resolution=256)` | search with a nonempty list of representations as one target |
| `vis.rotation_center(x, y, z)` | set the orbit pivot in nm without moving the view |
| `mol.load_mesh(path)` / `mol.add_mesh_ply(name, bytes)` | attach a PLY mesh in nm; return a mesh handle |
| `mol.meshes` | the molecule's mesh handles |
| `mesh.style`, `mesh.material`, `mesh.visible` | control mesh display |
| `mesh.point_size`, `mesh.line_width`, `mesh.smoothing` | pixel size/width (1–10), or Surface smoothing (0–5) |
| `mesh.ply_bytes()` / `mesh.save(path)` / `mesh.delete()` | export source PLY data or remove the mesh |
| `vis.set_color_preset(name)` / `vis.color_preset()` | apply or identify a viewer color preset; manual edits report `Custom` |
| `vis.set_element_color(number, rgba)` | override an element color with RGBA8 |
| `vis.set_secondary_color(index, rgba)` | edit a secondary-structure color (indices 0–8) |
| `vis.set_continuous_gradient(stops, midpoint)` | set three RGBA8 stops and midpoint (0.01–0.99) |
| `vis.colors_json()` | read the current color preferences as JSON |
| `vis.set_palette(colors)` | replace the discrete palette with a nonempty list of RGBA tuples |
| `vis.set_palette_color(index, rgba)` | edit an entry in the active discrete palette |
| `vis.add_palette_color(rgba)` / `vis.remove_palette_color(index)` | add or remove a palette entry; retain at least one |
| `vis.reset_colors()` | restore factory colors |
| `vis.save_settings()` | save current startup defaults explicitly |


Mesh and color controls use the same live state as the GUI. For example, after
creating `vis` and `mol` above:

```python
mesh = mol.load_mesh("surface.ply")
mesh.style = "surface"              # also "points" or "wireframe"
mesh.smoothing = 2
mesh.material = "Opaque"
vis.set_element_color(6, (90, 90, 90, 255))
vis.set_continuous_gradient(
    [(40, 80, 230, 255), (255, 255, 255, 255), (230, 40, 40, 255)],
    0.4,
)
mesh.save("surface-copy.ply")       # original geometry and vertex colors
```

Colors use byte channels (0–255). Secondary-structure indices are α helix, 3₁₀ helix,
π helix, β sheet, β bridge, turn, bend, polyproline, and coil, in that order.
`vis.reset_element_color(number)` removes an element override. Color edits apply live
and are included in saved sessions; call `vis.save_settings()` to store startup defaults.

`style`, `color` and `material` take the same names as the GUI pickers — e.g. `"vdw"`,
`"licorice"`, `"ballstick"`, `"cartoon"`, `"surface"`; `"element"`, `"chain"`, `"resid"`,
`"ss"`; `"Opaque"`, `"Transparent"`, `"Glossy"`, `"Glass"`, …

**View & camera** — the following camera and display controls apply live:

```python
vis.rotate(yaw, pitch)            # orbit, degrees   ·  vis.roll(angle)
vis.pan(dx, dy)                   # fraction of the viewport (+x right, +y up)
vis.zoom(1.5)                     # >1 closer, <1 farther
vis.reset_view()                  # re-frame all molecules (zoom-to-fit)
vis.focus(mol, s("protein"))      # frame this selection
vis.unobstructed_view(rep)        # optimize orientation and frame the target
vis.rotation_center(1.0, 2.0, 3.0)  # orbit pivot in nm

vis.projection("orthographic")   # or "perspective"
vis.background(0.0, 0.0, 0.05)   # solid RGB (0–1)
vis.background_gradient((0.1, 0.12, 0.3), (0.0, 0.0, 0.0))   # top, bottom

vis.axes(True, corner="top_left")            # orientation gizmo; corner optional
vis.depth_cue(mode="exp2", strength=0.6, start=0.3)   # or mode="none" to disable
vis.ambient_occlusion(True, strength=0.9, radius=0.4)
vis.shadows(True, strength=0.6)
```

Unobstructed-view searches treat other visible representations as occluders.
`zoom_out=2.0` doubles the framed extent; `resolution` sets the coarse search's
number of directions. The multi-target form can combine representations from
different molecules.

The [JavaScript API](../crates/molar_vis_js/README.md) also exposes these camera,
interaction, mesh, and palette controls. Browser file I/O uses byte arrays and
asynchronous readbacks; native Python additionally provides path-based mesh I/O,
image rendering, and session saving. JavaScript interaction partners must be
representation handles.

Molecule and representation listings query the live scene. Python uses `vis.mols`
and `mol.reps`; JavaScript uses `await vis.mols()` and `await mol.reps()`.
Handles identify objects independently of their order in the sidebar. Removing an
object makes its handle stale; it cannot refer to the next object in the list.

The viewer detects coordinate changes automatically. Its window runs on a background
thread on Linux and Windows; macOS Python viewer support is pending.


See the [README](../README.md) for the feature overview and desktop installation.

## Camera animation and movies

Camera methods accept `duration=...` and `easing="smooth"` or `"linear"`, and return
an `Animation` handle with `wait()`, `cancel()` and progress properties. Build a
sequence with `vis.movie()`, then preview, seek or render it through an FFmpeg pipe.
See [the animation guide](ANIMATION.md) for complete examples and export options.
