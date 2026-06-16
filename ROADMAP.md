# molar_vis — Roadmap

Future work, **in no particular order**. The milestones already shipped (M0–M12,
materials/OIT, surface, trajectories, browser app, picking/lasso, …) are tracked in
[CLAUDE.md](CLAUDE.md) under *Milestone status*; this file is the forward-looking list.

## File I/O & state
- Deleting trajectory frames
- Saving molecules and selections to file
- Saving / loading visualization state

## App & UI
- App settings
- Background color selection
- Selection input improvements:
  - Visual errors
  - Suggestions of available chains, residue and index ranges

## Rendering & visuals
- Different depth-cue methods
- More materials and a material editor
- On-screen labels and measurement
- Drawing geometric primitives
- High-quality rendering with raytracing
- Movies
- Rendering of bonds over PBC as dashed "half-bonds" without artifacts across the box

## Selection & picking
- Pick modes: whole residues

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
