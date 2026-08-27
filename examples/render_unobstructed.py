#!/usr/bin/env python
"""
Unobstructed-view demo for molar_vis (headless, offscreen PNGs).

Builds the same publication-style binding-site scene as
`render_pocket_interactions.py` -- protein secondary-structure CARTOON, the whole
residues that contact the ligand as LINES, the ligand as LICORICE, plus PLIP-style
INTERACTIONS -- then renders THREE views of each scene:

  * `<name>_before.png`  -- the plain pocket focus (the ligand is partly hidden behind
    the front helices / strands),
  * `<name>_after.png`   -- after `Visualizer.unobstructed_view(lig_rep)`, which rotates
    and scales the camera so the most of the LIGAND rep is directly on screen (hidden by
    neither the protein nor its own back atoms), and
  * `<name>_context.png` -- the same optimal orientation with `zoom_out=2.5`, pulled back
    to show the surrounding pocket.

Run (from the repo root, in the project venv):
    python examples/render_unobstructed.py                     # both bundled scenes
    python examples/render_unobstructed.py FILE OUTBASE [PAD]  # a custom scene

Bundled scenes (examples/data/, one PDB = protein + its ligand, ligand resname 'LIG'):
    eaat2_orthosteric_way213613.pdb  human EAAT2 chain A (PDB 7XR6) + WAY-213613 (deep pocket)
    eaat2_pam_gt949.pdb              human EAAT2 protomer  (PDB 9JVX) + GT949 (interface pocket)

Note: set the background AFTER `add_mol`. Adding the first molecule reseeds the camera
(including the background) from the theme default, so a `background(...)` call made
before `add_mol` is overwritten (that is why the figures otherwise come out dark).
"""
import os, sys, time
import molar_vis as mv

HERE = os.path.dirname(os.path.abspath(__file__))
LIG = "LIG"


def render_unobstructed(scene, out_base, pad=0.9, width=1300, height=1050):
    """Render a `_before` (pocket focus) and an `_after` (unobstructed view) PNG pair."""
    s = mv.System(scene)
    v = mv.spawn(visible=False)          # headless: invisible window, real GPU device
    v.projection("orthographic")
    v.ambient_occlusion(True)

    mol = v.add_mol(s)
    v.background(1, 1, 1)                 # AFTER add_mol (see the module docstring)

    # rep 0 (created with the molecule) -> protein cartoon, coloured by secondary structure
    r0 = mol.reps[0]
    r0.select(s("protein")); r0.style = "cartoon"; r0.color = "ss"

    # whole residues within 4.5 A of the ligand -> thin lines
    contacts = s("same residue as (protein and within 0.45 of resname %s)" % LIG)
    mol.add_rep(contacts, style="lines", color="element", width=3.0)

    # the ligand -> licorice: this is the TARGET rep for the unobstructed view
    lig = s("resname %s" % LIG)
    lig_rep = mol.add_rep(lig, style="licorice", color="element")

    # protein-ligand interactions (dashed PLIP-style contacts), scoped to the pocket
    inter = mol.add_interactions(lig_rep)
    inter.select(s("protein and within 0.6 of resname %s" % LIG))

    # BEFORE: the plain pocket focus (obstructed default), matching the pocket example
    v.focus(mol, s("(resname %s) or (protein and within %s of resname %s)" % (LIG, pad, LIG)))
    time.sleep(0.5)
    before = out_base + "_before.png"
    v.render(before, width=width, height=height)
    print("rendered", before)

    # AFTER: rotate + scale so most of the ligand is directly visible
    v.unobstructed_view(lig_rep)
    time.sleep(0.5)
    after = out_base + "_after.png"
    v.render(after, width=width, height=height)
    print("rendered", after)

    # CONTEXT: same optimal orientation, but zoomed out to show the surroundings
    v.unobstructed_view(lig_rep, zoom_out=2.5)
    time.sleep(0.5)
    context = out_base + "_context.png"
    v.render(context, width=width, height=height)
    print("rendered", context)


def main():
    if len(sys.argv) >= 3:
        pad = float(sys.argv[3]) if len(sys.argv) > 3 else 0.9
        render_unobstructed(sys.argv[1], sys.argv[2], pad)
        return
    # No args: render both bundled scenes. spawn() opens one viewer event loop per
    # process, so render each scene in its own subprocess (calling this script again).
    import subprocess
    scenes = [
        ("data/eaat2_orthosteric_way213613.pdb", "eaat2_orthosteric_way213613", "0.7"),
        ("data/eaat2_pam_gt949.pdb",             "eaat2_pam_gt949",             "1.4"),
    ]
    for rel, out_base, pad in scenes:
        subprocess.run([sys.executable, os.path.abspath(__file__),
                        os.path.join(HERE, rel), os.path.join(HERE, out_base), pad], check=True)


if __name__ == "__main__":
    main()
