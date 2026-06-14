# Test structures

- `2lao.pdb` — small protein (1911 atoms), copied from the molar test suite.
  Used for quick visual checks of every representation.

- `large_375k.gro` — large system (375,548 atoms) for performance / responsiveness
  testing. **Not tracked in git** (~25 MB, generated). Regenerate with GROMACS by
  tiling molar's largest test system 2×2×1:

  ```sh
  gmx genconf -f ../../molar/molar/tests/cpt.gro -o tests/large_375k.gro -nbox 2 2 1
  ```

## Quick run

```sh
cargo run -p molar_vis -- tests/2lao.pdb
cargo run -p molar_vis -- tests/large_375k.gro
```

Headless verification env hooks (native): `MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines`,
`MOLAR_VIS_DEBUG_ORBIT=<deg>`, `MOLAR_VIS_DEBUG_ORTHO=1`.
