# molar_vis examples

Runnable examples that drive the viewer headlessly from Python.

## `render_pocket_interactions.py`

Publication-style protein-ligand binding-site figures, rendered offscreen (no window):
protein **cartoon** + whole contacting residues as **lines** (width 3) + ligand as
**licorice** + protein-ligand **interactions** (dashed contacts), framed on the pocket,
white background.

```sh
# from the repo root, in the project venv
python examples/render_pocket_interactions.py                 # both bundled scenes -> PNGs in examples/
python examples/render_pocket_interactions.py FILE OUT [PAD]  # a custom protein+ligand scene
```

Exercises the Python API: `spawn(visible=False)`, `add_mol`, `add_rep(sel, style, color, width)`,
`MolHandle.add_interactions(partner_rep)`, `RepHandle.width`, `Visualizer.focus(mol, sel)`,
`Visualizer.render(path, width, height)`.

### Bundled scenes (`data/`)

Each file is one structure = protein + its ligand; the ligand is renamed to resname `LIG`.

| File | Contents |
|---|---|
| `eaat2_orthosteric_way213613.pdb` | Human EAAT2 chain A (PDB **7XR6**) + **WAY-213613**, a competitive/orthosteric blocker in the substrate pocket. |
| `eaat2_pam_gt949.pdb` | Human EAAT2 protomer (PDB **9JVX**, 3.97 Å) + **GT949**, a reported (later disputed) positive allosteric modulator at the scaffold–transport interface. |

### Notes / gotchas (useful when extending)

- molar's selection parser has **no `byres`** — expand a distance shell to whole residues with
  **`same residue as (...)`**. Selection distances are in **nanometres** (`0.45` = 4.5 Å).
- These cryo-EM PDBs carry a placeholder `CRYST1 1 1 1 P 1` cell. molar reads it as a 0.1 nm
  periodic box; molar_vis now **ignores a degenerate box (< 0.5 nm)** so PBC bond-wrapping does
  not shatter the cartoon/lines/licorice. Genuine crystal/MD boxes are untouched.
- `PAD` (focus padding, nm) sets both the zoom and the depth clip slab. If geometry in front of
  the pocket (e.g. a β-hairpin over the ligand) is clipped by the near plane, widen `PAD`.
  A dedicated **unobstructed-view** mode (rotate/fade the occluding front geometry) is future work.
- `spawn()` runs one viewer event loop per process, so render **one scene per process**. The
  no-argument "render both" path re-invokes this script as a subprocess per scene.
