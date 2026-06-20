# molar_vis — Roadmap

Future work, **in no particular order**. The milestones already shipped (M0–M12,
materials/OIT, surface, trajectories, browser app, picking/lasso, …) are tracked in
[CLAUDE.md](CLAUDE.md) under *Milestone status*; this file is the forward-looking list.

## File I/O & state
- ~~Deleting trajectory frames~~ — **shipped** (M15; Range/Decimate dialog from the molecule menu)
- ~~Saving molecules and selections to file~~ — **shipped** (M15; molecule menu + per-rep save button)
- ~~Saving / loading visualization state~~ — **shipped** (M13; see CLAUDE.md *Milestone status*)

## App & UI
- ~~App settings~~ — **shipped** (M21; settings dialog + persisted config in the platform config dir)
- ~~Background color selection~~ — **shipped** (M20; solid color or gradient)
- ~~Selection input improvements~~ — **shipped** (M14; see CLAUDE.md *Milestone status*):
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

## Selection & picking
- ~~Pick modes: whole residues~~

## Coarse-grained
- ~~CG (Martini) secondary-structure **display** (cartoon)~~ — **shipped** (M22; geometric SS from
  the BB trace + flat ribbon wrapped on the helix cylinder surface, no bonds needed)
- CG (Martini) bond guessing — distance search doesn't transfer to CG bead sizes; needs a
  Martini-aware criterion (the cartoon sidesteps this by grouping per-residue BB/SC beads)

## Scripting & extensibility
- Python bindings with exposed visualizer objects
- Explore a possible embedded internal command language
- Plugins architecture

## Drug-discovery goodies
- PLIP-like interactions and their visualization
- Reading SDF files (molar)

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
