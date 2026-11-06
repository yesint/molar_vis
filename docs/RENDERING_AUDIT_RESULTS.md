# Rendering audit implementation results

This log follows the [2026-11-06 audit plan](RENDERING_AUDIT_2026-11-06.md). Each implementation
change must pass its relevant performance and visual parity checks before it is committed and
the next change starts. Conditional experiments remain subject to the audit's measurement gates.

## Test environment and method

The physical adapter is AMD Radeon Graphics, RADV REMBRANDT, Mesa 26.2.5. The sandbox exposes
only software Vulkan; physical GPU tests run outside the sandbox with the Radeon ICD selected.
Release binaries use the repository's locked dependencies and four Rayon threads. Raw logs,
saved reference binaries, mesh snapshots, and PNG comparisons are under `target/render-audit/`.
These generated artifacts are intentionally excluded from Git.

The `audit_surface_and_render_baseline` ignored test uses `tests/2lao.pdb`, surface qualities
0/2/4, both projections, and disabled/enabled AO and shadows. It measures seven complete surface
builds after warmup, and twenty completed 640×480 raster frames after warmup at default 2× SSAA.
Frame timings include command preparation and waiting for completion, rather than just GPU pass
time. Capture and PNG encoding occur outside the measured frame loop. Geometry comparison checks
every vertex byte, index, and source owner; image comparison checks every RGBA byte.

Run timing measurements without concurrent compilation or other benchmark/test processes. Repeat
before/after binaries alternately when an apparent change is within the observed variability.
Use `MOLAR_VIS_PROFILE=1` in a separate diagnostic run because logging and queries add overhead.
The PNG comparison sheet shows the reference, candidate, and amplified pixel difference.

```bash
CARGO_TARGET_DIR=target/audit-release cargo test -p molar_vis_core --release --offline --lib --no-run
# Select the printed test executable and run it with physical GPU access:
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.x86_64.json WGPU_BACKEND=vulkan RAYON_NUM_THREADS=4 MOLAR_VIS_AUDIT_OUTPUT=target/render-audit/candidate TEST_EXECUTABLE audit_surface_and_render_baseline --ignored --nocapture
python3 scripts/compare_render_audit.py target/render-audit/reference target/render-audit/candidate
```

## Implementation record

### 0. Surface and rendering measurement baseline

Added whole-build and individual surface-stage scopes, grid/result counters, indexed mesh and
pick upload counters, envelope build/refit/upload counters, RT buffer upload counters, and RT
dispatch/resolve counters. JSON construction and clock reads remain disabled unless profiling
is enabled. Added a reproducible release benchmark and an exact artifact comparison script.
Later packages will add their own targeted benchmarks, including RT sweep/convergence timing.

Five alternating reference/candidate runs passed exact parity for all 105 paired artifacts.
The initial twelve-view comparison sheet was also inspected visually; silhouettes, colors,
surface detail, and shadows matched, and the difference images were black. All 363 regular
release tests passed, and the wasm32 core check passed with its three existing warnings.

| Workload | Reference median of run medians | Candidate median of run medians |
| --- | --- | --- |
| Complete surface, quality 0 | 22.934 ms | 22.834 ms |
| Complete surface, quality 2 | 218.177 ms | 221.872 ms |
| Complete surface, quality 4 | 662.782 ms | 653.015 ms |
| Quality 2, orthographic raster, effects enabled | 7.300 ms | 7.464 ms |
| Quality 4, perspective raster, effects enabled | 9.590 ms | 9.540 ms |

The changes in these measurements fall within the observed run variability; the instrumentation
does not establish a performance gain. For example, quality-2 reference medians ranged from
163.327 to 221.163 ms and candidate medians ranged from 219.888 to 223.715 ms. The initial single
pair showed an apparent 60% difference, which the alternating runs did not reproduce. Raw
samples and p95 values are retained in each run's `metrics.json`.

### 1. Reuse surface projection work (S3)

The refinement loop now reuses the split decision's initial field sample for its first
projection step. Relaxation computes normals only in its final iteration and counts fixed
triangle incidences once. It retains all three projection steps, displacement limits, field
arithmetic, neighbor accumulation order, and isolated-vertex behavior. The original routines
remain in a test-only reference module.

All 364 regular release tests passed. A new differential test compares every mesh vertex byte,
index, and source owner across all five quality levels and overlapping/disconnected shapes.
The wasm32 core check passed with its existing warnings. Ten physical-GPU before/after pairs,
five in each execution order, passed exact parity for all 210 paired artifacts. The comparison
sheet and a full-resolution candidate image were inspected; the surface shape, lighting, and
colors match the reference.

The paired, alternating-order stage benchmark measured relaxation at 0.9904 → 0.8684 ms
(12.3% lower) and refinement at 1.7247 → 1.4996 ms (13.1% lower). Their p95 values changed from
1.1201 → 1.0805 ms and 1.8390 → 1.6559 ms, respectively. Each stage retained byte-exact output
through all 31 measured pairs.

| Complete build | Reference median of run medians | Candidate median of run medians | Median paired change |
| --- | --- | --- | --- |
| Surface quality 0 | 22.775 ms | 22.067 ms | −3.3% |
| Surface quality 2 | 210.470 ms | 141.365 ms | −15.3% |
| Surface quality 4 | 661.693 ms | 625.995 ms | −3.8% |

The full-build results are noisier than the paired stage measurements, especially at quality 2;
they must not be treated as guaranteed application speedups. Unchanged raster workloads showed
overlapping run distributions. The largest positive median paired raster difference was 5.1%
for quality-4 orthographic rendering without effects, where reference medians ranged from
1.734–1.956 ms and candidate medians from 1.678–1.984 ms. Reverse-order controls removed several
larger apparent differences. No raster speedup is claimed for this CPU-only change.
