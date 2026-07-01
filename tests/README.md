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

- `ligands20.sdf` — 20 diverse drug-like organic molecules in one multi-record SDF
  (each `$$$$` record a distinct molecule with its own atoms + bonds), the committed
  fixture for the **molecular group** feature (M28 — a multi-molecule SDF loads as one
  group cycled member-by-member). Sizes span metformin (9 heavy atoms) to atorvastatin
  (41) so cycling exercises the per-member camera re-fit. Regenerate from ChEMBL
  canonical SMILES (names become the SDF titles → member names) with **Open Babel**:

  ```sh
  # ligands20.smi: one "SMILES name" per line (20 drugs pulled from ChEMBL —
  # aspirin, caffeine, ibuprofen, …, atorvastatin, imatinib, penicillin_G)
  obabel ligands20.smi -O tests/ligands20.sdf --gen3d   # 3D coords + bond orders
  grep -c '^\$\$\$\$' tests/ligands20.sdf                # == 20
  ```

  Headless check: `MOLAR_VIS_DEBUG_SDF=tests/ligands20.sdf` (+ `MOLAR_VIS_DEBUG_GROUP_MEMBER=<n>`
  / `MOLAR_VIS_DEBUG_GROUP_EXPAND=1`) loads it as a group, bypassing the file dialog.

- `toy_tube.pdb` — two carbons + one bond: the minimal single-**tube** fixture for isolating
  cylinder-impostor artifacts (used to fix the end-on "crescent"/strip bugs — see the *Impostors*
  note in CLAUDE.md). Open in Licorice (`MOLAR_VIS_DEBUG_REP=licorice`) and rotate to an end-on
  view.

## Camera telemetry (debugging a specific view)

To reproduce an **exact** interactive view headlessly: launch with
`MOLAR_VIS_DEBUG_CAMERA_LOG=<path>` (writes the live camera as JSON each frame), position the view
in the window, then render that view with `MOLAR_VIS_DEBUG_CAMERA=<same path>` (+
`MOLAR_VIS_DEBUG_SAVE_IMAGE=out.png`). A magenta/white `MOLAR_VIS_DEBUG_BG` (or a hand-edited
`background` in the JSON) distinguishes real coverage holes (show the bg color) from dark-shaded
surface; `MOLAR_VIS_NO_EARLY_Z=1` isolates early-Z culling from geometry/shading.

## Quick run

```sh
cargo run -p molar_vis -- tests/2lao.pdb
cargo run -p molar_vis -- tests/large_375k.gro
```

Headless verification env hooks (native): `MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines`,
`MOLAR_VIS_DEBUG_ORBIT=<deg>`, `MOLAR_VIS_DEBUG_ORTHO=1`.
