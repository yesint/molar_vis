# Test structures

- `2lao.pdb` — small protein (1911 atoms), copied from the molar test suite.
  Used for quick visual checks of every representation.

- `2lao_cg.pdb` — `2lao` coarse-grained with **martinize2** (238 residues, mixed α/β); the
  committed fixture for the **CG cartoon** (M22 — geometric SS + wrapping-ribbon helices).
  Regenerate (martinize2; the 4 LYS side-chain beads that came out with NaN coords from
  incomplete crystal side chains were stripped):

  ```sh
  martinize2 -f tests/2lao.pdb -x tests/2lao_cg.pdb -ff martini3001 -dssp
  grep -vi ' nan ' tests/2lao_cg.pdb > tmp && mv tmp tests/2lao_cg.pdb   # drop NaN beads
  ```

  `cg.pdb` (a Martini membrane bundle, all-helix) is also handy for CG checks but is **not tracked
  in git** (~4 MB, user-supplied).

- `large_375k.gro` — large system (375,548 atoms) for performance / responsiveness
  testing. **Not tracked in git** (~25 MB, generated). Regenerate with GROMACS by
  tiling molar's largest test system 2×2×1:

  ```sh
  gmx genconf -f ../../molar/molar/tests/cpt.gro -o tests/large_375k.gro -nbox 2 2 1
  ```

- `2lao_traj.pdb` — a 20-frame multi-MODEL trajectory of `2lao` (rigid drift + breathing
  wobble) for verifying trajectory loading/playback. **Not tracked in git** (generated).
  Regenerate:

  ```python
  import math
  atoms = [l.rstrip("\n") for l in open("tests/2lao.pdb") if l.startswith(("ATOM","HETATM"))]
  with open("tests/2lao_traj.pdb","w") as g:
      for fr in range(20):
          g.write(f"MODEL     {fr+1:>4}\n")
          amp = 1.5*math.sin(fr/19*math.pi)
          for idx,l in enumerate(atoms):
              x,y,z = float(l[30:38]),float(l[38:46]),float(l[46:54])
              ph = (idx%50)/50*2*math.pi
              x += 0.6*fr + amp*math.sin(ph); y += amp*math.cos(ph)
              g.write(f"{l[:30]}{x:8.3f}{y:8.3f}{z:8.3f}{l[54:]}\n")
          g.write("ENDMDL\n")
  ```

## Quick run

```sh
cargo run -p molar_vis -- tests/2lao.pdb
cargo run -p molar_vis -- tests/large_375k.gro
```

Headless verification env hooks (native): `MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines`,
`MOLAR_VIS_DEBUG_ORBIT=<deg>`, `MOLAR_VIS_DEBUG_ORTHO=1`.
