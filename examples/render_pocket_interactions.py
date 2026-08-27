#!/usr/bin/env python
"""
Headless protein-ligand pocket rendering with molar_vis.

Demonstrates the Python API for a publication-style binding-site figure:
  * protein as a secondary-structure CARTOON,
  * the whole residues in contact with the ligand as LINES (width 3),
  * the ligand as LICORICE,
  * protein-ligand INTERACTIONS (PLIP-style dashed contacts),
  * zoom-to-selection, on a white background, rendered offscreen to a PNG.

Run (from the repo root, in the project venv):
    python examples/render_pocket_interactions.py                 # renders both bundled scenes
    python examples/render_pocket_interactions.py FILE OUT [PAD]  # a custom scene

Bundled scenes (examples/data/, one PDB = protein + its ligand, ligand resname 'LIG'):
    eaat2_orthosteric_way213613.pdb  human EAAT2 chain A (PDB 7XR6) + WAY-213613 (competitive blocker)
    eaat2_pam_gt949.pdb              human EAAT2 protomer  (PDB 9JVX) + GT949 (reported/disputed PAM)

Notes for anyone extending this:
  * molar's selection parser has no `byres`; use `same residue as (...)` to expand
    a distance shell to whole residues. Distances are in NANOMETRES (0.45 nm = 4.5 Å).
  * These cryo-EM inputs carry a placeholder `CRYST1 1 1 1 P 1` unit cell. molar reads it
    as a 0.1 nm periodic box; molar_vis ignores such a degenerate box (< 0.5 nm) so
    PBC bond-wrapping does not shatter the cartoon/lines/licorice (see geometry.rs).
  * PAD (focus padding, nm) controls the zoom AND the depth clip slab. Too small and
    geometry in front of the pocket (e.g. a β-hairpin over the ligand) is clipped by the
    near plane; widen PAD to include it. To instead turn the camera so the ligand is not
    obstructed, see render_unobstructed.py (Visualizer.unobstructed_view).
"""
import os, sys, time
import molar_vis as mv

HERE = os.path.dirname(os.path.abspath(__file__))
LIG = "LIG"


def pick(s, cands):
    """Return the first selection string that compiles; print each attempt."""
    for c in cands:
        try:
            sel = s(c)
            print("  sel ok  [%s]" % c)
            return sel
        except Exception as e:
            print("  sel bad [%s]: %s" % (c, e))
    raise RuntimeError("no selection worked: %r" % (cands,))


def render_pocket(scene, out, pad=0.9, width=1300, height=1050):
    """Render one protein+ligand scene as cartoon + contacting-residue lines + licorice + interactions."""
    s = mv.System(scene)
    v = mv.spawn(visible=False)          # headless: invisible window, real GPU device
    v.background(1, 1, 1)
    v.projection("orthographic")
    v.ambient_occlusion(True)

    mol = v.add_mol(s)

    # rep 0 (created with the molecule) -> protein cartoon, coloured by secondary structure
    r0 = mol.reps[0]
    r0.select(s("protein")); r0.style = "cartoon"; r0.color = "ss"

    # whole residues within 4.5 A of the ligand -> thin lines
    contacts = pick(s, ["same residue as (protein and within 0.45 of resname %s)" % LIG])
    mol.add_rep(contacts, style="lines", color="element", width=3.0)

    # the ligand -> licorice
    lig = s("resname %s" % LIG)
    lig_rep = mol.add_rep(lig, style="licorice", color="element")

    # protein-ligand interactions (dashed PLIP-style contacts), scoped to the pocket
    inter = mol.add_interactions(lig_rep)
    inter.select(s("protein and within 0.6 of resname %s" % LIG))

    # frame the pocket (pad also sets the depth clip slab; widen to avoid near-plane clipping)
    v.focus(mol, s("(resname %s) or (protein and within %s of resname %s)" % (LIG, pad, LIG)))

    time.sleep(0.5)
    v.render(out, width=width, height=height)
    print("rendered", out)


def main():
    if len(sys.argv) >= 3:
        pad = float(sys.argv[3]) if len(sys.argv) > 3 else 0.9
        render_pocket(sys.argv[1], sys.argv[2], pad)
        return
    # No args: render both bundled scenes. spawn() opens one viewer event loop per
    # process, so render each scene in its own subprocess (calling this script again).
    import subprocess
    scenes = [
        ("data/eaat2_orthosteric_way213613.pdb", "eaat2_orthosteric_way213613.png", "0.7"),
        ("data/eaat2_pam_gt949.pdb",             "eaat2_pam_gt949.png",             "1.4"),
    ]
    for rel, out, pad in scenes:
        subprocess.run([sys.executable, os.path.abspath(__file__),
                        os.path.join(HERE, rel), os.path.join(HERE, out), pad], check=True)


if __name__ == "__main__":
    main()
