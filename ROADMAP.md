# molar_vis — Roadmap

Future work, **in no particular order**. The milestones already shipped (M0–M12,
materials/OIT, surface, trajectories, browser app, picking/lasso, …) are tracked in
[docs/MILESTONES.md](docs/MILESTONES.md) (*Milestone status*); this file is the forward-looking list.

## File I/O & state
- ~~Deleting trajectory frames~~ — **shipped** (M15; Range/Decimate dialog from the molecule menu)
- ~~Saving molecules and selections to file~~ — **shipped** (M15; molecule menu + per-rep save button)
- ~~Saving / loading visualization state~~ — **shipped** (M13; see docs/MILESTONES.md)

## App & UI
- ~~App settings~~ — **shipped** (M21; settings dialog + persisted config in the platform config dir)
- ~~Background color selection~~ — **shipped** (M20; solid color or gradient)
- ~~Selection input improvements~~ — **shipped** (M14; see docs/MILESTONES.md):
  - ~~Visual errors~~ — erroring span highlighted red in the field (molar caret) + message
  - ~~Suggestions of available chains, residue and index ranges~~ — hint under the field per keyword

## Rendering & visuals
- ~~Different depth-cue methods~~ — **shipped** (M17; Linear/Exp/Exp² cue modes)
- More materials and a material editor
- On-screen labels and measurement
- Drawing geometric primitives
- High-quality rendering with raytracing
- Movies
- ~~Rendering of bonds over PBC as dashed "half-bonds" without artifacts across the box~~ — **shipped** (M16)

## UI
- **Bold text is not currently possible.** egui's default fonts are Ubuntu-*Light* and Hack — there
  is no bold face in the app — and `RichText::strong()` only swaps in a brighter colour. Rendering
  genuinely bold text (e.g. to make molecule names stand out in the tree) requires embedding a bold
  TTF and registering it as a font family: `DejaVuSans-Bold` (~700 kB) or `NotoSans-Bold` (~450 kB)
  whole, or ~30 kB subset to Latin-1 with `pyftsubset` (fonttools isn't installed here). The cost is
  a binary asset in the repo plus that much added to the wasm bundle. Needs a decision.

## Selection & picking
- ~~Pick modes: whole residues~~

## Coarse-grained
- ~~CG (Martini) secondary-structure **display** (cartoon)~~ — **shipped** (M22; geometric SS from
  the BB trace + flat ribbon wrapped on the helix cylinder surface, no bonds needed)
- CG (Martini) bond guessing — **partly shipped** (M31): a CG structure that records its
  connectivity (a PDB's `CONECT` — the usual case for a martinized system) now keeps it, since
  `bonds::resolve` unions the file's table with the distance guess instead of discarding it
  (`2lao_cg.pdb` 290 → 858 bonds). Still open for CG files with **no** recorded bonds: distance
  search doesn't transfer to CG bead sizes, so that needs a Martini-aware criterion (the cartoon
  sidesteps this by grouping per-residue BB/SC beads)

## Scripting & extensibility
- Python bindings with exposed visualizer objects
- Explore a possible embedded internal command language
- Plugins architecture

## Drug-discovery goodies
- ~~Loading docking results (receptor + ligand poses)~~ — **shipped** (M32; `Molecule ▸ Load
  docking data…`: poses as a `MolGroup` with an `Interactions` rep auto-linked to the receptor,
  rigid or flexible receptor, and pose ⇄ receptor-frame stepping for the flexible case). Later:
  per-pose score display + sorting, and recording the pairing in a session as *docking* rather
  than as its parts.
- ~~PLIP-like interactions and their visualization~~ — **shipped** (M29; the `Interactions`
  rep style — H-bonds + hydrophobic contacts between a rep and a chosen partner rep, drawn as
  Discovery-Studio-style dashed lines). Later: π-stacking / salt bridges / halogen bonds,
  distance labels.
- Reading SDF files (molar)
- ~~Partial-charge coloring + on-demand charge computation~~ — **shipped** (M31; the `Charge`
  color scheme on a diverging red–white–blue ramp, partial or formal, with espaloma prediction on
  the selection via `molar_ff` behind the **[Color]** tab's *Compute charges* button). Later:
  GAFF/GAFF2 atom typing is already in `molar_ff` and could surface the same way; computed charges
  aren't yet persisted in a session (molecules reload from disk), and espaloma needs Kekulé bond
  orders so PDB/GRO inputs can't be charged.

## Altering structures visually
- Deleting / Moving / rotating atoms, residues, molecules
- Rotating bonds and dihedrals
- ~~Simple UFF minimization~~ — **shipped** (M23; lightweight UFF-style cleanup FF —
  harmonic bond/angle + weak torsion + WCA repulsive vdW — with a FIRE minimizer; see
  `minimize.rs`)
- ~~Drawing molecules with atoms/bonds palette a-la Marvin JS with on-the-fly minimization~~ —
  **shipped** (M23; Draw mode: vertical icon toolbar + viewport place-atom / drag-to-bond /
  cycle-order / erase, debounced FIRE cleanup + a "Clean up" button). **Deferred follow-ups:**
  ring/fragment templates · automatic hydrogens · formal charges · change-element-of-existing-atom ·
  multi-order *bond rendering* (double/triple still draw as one cylinder) · SMILES import/export ·
  embedding drawn molecules in sessions
