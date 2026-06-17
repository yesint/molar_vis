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
- CG (Martini) bonds and secondary-structure display

## Scripting & extensibility
- Python bindings with exposed visualizer objects
- Explore a possible embedded internal command language
- Plugins architecture

## Drug-discovery goodies
- PLIP-like interactions and their visualization
- Reading SDF files (molar)

## Altering structures visually
- Moving / rotating atoms, residues, molecules
- Rotating bonds and dihedrals
- Simple UFF minimization
