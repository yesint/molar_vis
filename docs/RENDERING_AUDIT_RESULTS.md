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

### 2. Reuse synchronous surface GPU storage (S2)

Synchronous surface builds now use the existing update path, which uploads both vertex and
index arrays, retains sufficient capacity, reallocates on growth, and releases empty geometry.
CPU surface construction is unchanged. Storage retains the largest installed mesh capacity
until growth or removal; this change adds no geometric over-allocation or retained mesh copies.

The new physical-GPU integration test rebuilds real surfaces through qualities 2/0/2/3/0 with
alternating smoothing. It verifies buffer identities and capacities, compares each rendered
image with a fresh upload, and covers empty selections and switching to an atomic representation.
All five image pairs were byte-exact and their comparison sheet was inspected visually.
The existing equal-count changed-index image test also passed. All 364 regular release tests
passed, and the wasm32 check passed with its three existing warnings.

Five same-process benchmark runs each measured 31 alternating fresh/reused upload pairs per
mesh after warmup. Timing includes queue submission and waiting for transfer completion.
The following values are medians across runs; p95 columns are medians of each run's p95.

| Mesh step | Fresh median | Reused median | Fresh p95 | Reused p95 |
| --- | --- | --- | --- | --- |
| Initial quality 2 | 0.4809 ms | 0.4428 ms | 0.7285 ms | 0.7607 ms |
| Quality 0 after shrink | 0.1565 ms | 0.1539 ms | 0.2549 ms | 0.2190 ms |
| Quality 2 after regrowth | 0.4942 ms | 0.4421 ms | 0.7703 ms | 0.6885 ms |
| Quality 3 | 1.0092 ms | 0.7908 ms | 1.4402 ms | 1.2674 ms |
| Quality 0 after second shrink | 0.1758 ms | 0.1710 ms | 0.3541 ms | 0.2546 ms |

The reduction is 8–11% at quality 2 and 22% at quality 3; quality-0 changes are small relative
to timing variability. Tail latency remains noisy, including a 4% increase in the initial
quality-2 p95. Repeated installation of a fitting mesh retains both buffers, eliminating two
GPU buffer allocations per installation while transferring the same bytes. These are upload
measurements, not claimed full surface-build or frame-rate improvements.

### 3. Resolve progressive RT only when its completed image changes (R2)

The tracer records the completed sample count and last destination view after a resolve.
It skips repeated resolves during a partial sweep, initializes a changed destination, and
clears the record on restart. It retains one texture-view handle, with no additional texture
allocation. Dispatches, sample counts, random sequences, and accumulation arithmetic are unchanged.

A physical-GPU differential test reproduces the former policy by clearing only this record
before each reference step. All 33 displayed intermediate/final image comparisons passed for
orthographic and perspective views, AO/shadows, and full GI. The test also covers destination
switches during a partial sweep, cancellation, restart, resize, and completion in one call.
The three saved final image pairs were inspected visually and match exactly. The existing scene
and accumulator replacement GPU test passed, as did all 364 regular release tests and the
wasm32 check (three existing warnings).

Three benchmark runs each measured eleven alternating policy pairs after warmup, including GPU
completion, at 64 samples per pixel. Final images also match after every measured pair.

| Output and submissions per call | Reference median | Candidate median | Reference p95 | Candidate p95 |
| --- | --- | --- | --- | --- |
| 1536×1024, four tiles | 137.402 ms | 132.860 ms | 146.050 ms | 143.647 ms |
| 1536×1024, 32 tiles (export) | 123.424 ms | 122.701 ms | 132.791 ms | 126.020 ms |
| 256×192, 32 tiles | 4.694 ms | 4.341 ms | 5.912 ms | 5.650 ms |

Values are medians across runs. The four-tile case reduced resolves from 13 to 3 and completed
3.3% sooner. The controls retained three and one resolves respectively; their timing differences
do not establish a speedup. A separate timestamp-enabled diagnostic measured large-image
resolve passes at 0.413 ms median and 0.459 ms p95. Profiling overhead was excluded from the
convergence measurements. These results establish the benefit for a simple nine-sphere scene;
the relative gain will be smaller when ray intersection or shading dominates convergence.

### 4. Effect endpoint checkpoint (R8; validation incomplete)

Work stopped at the user's request, and this checkpoint is committed for later continuation.
The current changes skip zero-strength shadow-map rendering and shadow sampling, and skip
the deferred effects pass when neither global nor intrinsic material AO nor shadows contributes.
Enabled global AO at zero strength still suppresses intrinsic material AO. The checkpoint also
adds `audit_effect_endpoints`, covering all 14 material presets, both projections, zero and
near-zero strengths, full and mixed GI, transparency, fog, and gradient backgrounds. The shadow
cache regression now checks zero-strength suppression and restoration after camera movement.

An experimental full-GI shortcut was compiled and tested together with these guards, but it
was removed before this checkpoint. The first combined before/after run matched all 392 images
exactly; selected comparison images were inspected visually. Three before/after timing pairs
completed, but mixed-GI controls showed possible slowdowns alongside substantial between-run
variation. These measurements do not establish acceptance of the current isolated guards.
The shader after removal of the full-GI shortcut has not yet been rebuilt or validated, and
the extended shadow-cache test, regular suite, and browser check remain pending for this step.

Resume by validating the isolated zero-strength changes. A same-process alternating shader
benchmark was proposed to separate GPU clock variation from shader regressions, but it was
not implemented. Run the endpoint image comparisons, the extended shadow-cache GPU test,
regular release tests, and wasm32 check before marking this step accepted or starting another
optimization. The remaining audit items are still pending.

Saved binaries `target/render-audit/bin/step4-before` and `step4` contain the original policy
and the combined experimental candidate respectively; `step4` does not represent this
checkpoint's final source. Logs and images are in `target/render-audit/step4-before-1..3` and
`step4-after-1..3` (logs use the same names with `.log`). The first comparison report is
`step4-comparison-1.log`, and the inspected subset is `step4-inspection.png`.
