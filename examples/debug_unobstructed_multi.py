#!/usr/bin/env python
"""
Repro + saved session for a bug in `Visualizer.unobstructed_view_multi`.

Scene: two EAAT2 crystal protomers superimposed (7XR6 chain A + 9JVX protomer,
relabeled chains A/B), with their two ligands WAY-213613 (chain Y) and GT949 (chain Z)
as separate licorice reps. Data: data/eaat2_both_superposed.pdb (ligands renamed 'LIG').

Bug: the two ligands do NOT interpenetrate — min interatomic distance ≈ 6.6 Å, 0 WAY
atoms within 4 Å of GT949 — so a viewing direction exists that shows BOTH un-occluded
(the single-target `unobstructed_view` on WAY already reveals both). Yet
`unobstructed_view_multi([way, gt])` returns a direction that leaves WAY hidden behind
GT949 (its result ≈ the single-GT949 view). So the union-of-targets direction search
does not converge to the existing both-visible direction.

Run (repo root, project venv):
    python examples/debug_unobstructed_multi.py
Writes:
    examples/eaat2_unobstructed_multi_debug.session.json   (load to reproduce the view)
    examples/eaat2_unobstructed_multi_debug.png            (the offending render)
"""
import os, time
import molar_vis as mv

HERE = os.path.dirname(os.path.abspath(__file__))
PDB = os.path.join(HERE, "data", "eaat2_both_superposed.pdb")
SESSION = os.path.join(HERE, "eaat2_unobstructed_multi_debug.session.json")
PNG = os.path.join(HERE, "eaat2_unobstructed_multi_debug.png")

s = mv.System(PDB)
v = mv.spawn(visible=False)
v.projection("orthographic"); v.ambient_occlusion(True)
mol = v.add_mol(s, source_path=PDB)
v.background(1, 1, 1)                       # after add_mol

# both proteins (occluders): a cartoon rep each, solid-coloured (adjust the RGB freely)
r0 = mol.reps[0]; r0.select(s("protein and chain A")); r0.style = "cartoon"; r0.color = "solid:180,150,210"  # 7XR6 violet
mol.add_rep(s("protein and chain B"), style="cartoon", color="solid:150,120,95")                             # 9JVX tan
# two ligands (targets): separate licorice reps, distinct solid colours
way = mol.add_rep(s("resname LIG and chain Y"), style="licorice", color="solid:31,119,180")   # WAY  blue
gt  = mol.add_rep(s("resname LIG and chain Z"), style="licorice", color="solid:255,127,14")    # GT949 orange
way.width = 2.5; gt.width = 2.5

# the offending call — should orient so BOTH are front-most; it does not
v.unobstructed_view_multi([way, gt], zoom_out=2.5)
time.sleep(0.5)

v.render(PNG, width=1100, height=900)
print("rendered", PNG)
v.save_session(SESSION)
print("saved session", SESSION)
