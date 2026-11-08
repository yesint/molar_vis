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

### 4a. Isolated zero-strength effect guards accepted (2026-11-07)

The checkpoint's zero-strength changes passed validation after removal of the full-GI
experiment. A fresh reference/candidate run matched all 392 endpoint images exactly. The
comparison sheet and a six-case subset were inspected visually. The extended shadow-cache
test passed, including restoration after camera movement. All 364 regular release tests and
the wasm32 check passed; the browser check retained its three existing warnings.

Two new same-process tests alternate original and guarded shaders, preserving identical
geometry, camera, and submission work. The RT test measures 31 pairs after warmup per case;
the raster test measures 51 pairs and forces an equal shadow-map rebuild for both modes.
Both tests also require exact reference/candidate pixels. These tests separate shader costs
from the substantial between-process variation seen in the earlier experiment.

| Paired RT workload | Reference median / p95 | Guarded median / p95 | Median paired change |
| --- | --- | --- | --- |
| Zero shadows, zero AO | 39.497 / 40.732 ms | 6.736 / 7.425 ms | −82.9% |
| Zero shadows, AO strength 0.6 | 56.380 / 59.154 ms | 25.695 / 26.859 ms | −55.0% |
| Shadows 0.7, zero AO | 39.462 / 40.700 ms | 39.404 / 40.532 ms | −0.3% |
| Shadows 0.7, AO 0.6, GI 0.5 | 95.388 / 99.576 ms | 94.875 / 99.502 ms | +0.5% |
| Shadows 0.7, AO 0.6, GI 1.0 | 96.374 / 99.538 ms | 96.332 / 101.290 ms | −0.2% |
| Near-zero effects, GI 0.99999 | 98.950 / 109.364 ms | 97.870 / 106.923 ms | −1.1% |

Nonzero-shadow raster controls had median paired changes of −0.6% and −1.1%, with AO off
and on respectively. No control showed a reproducible slowdown. The full renderer's moving
camera benchmark measured zero-strength shadows at 3.489 → 1.190 ms with enabled zero AO,
3.927 → 1.261 ms with AO disabled, and 7.188 → 4.713 ms with AO strength 0.6. Corresponding
p95 values were 3.717 → 1.407, 4.464 → 1.792, and 7.669 → 6.706 ms. These frame measurements
include CPU submission and GPU completion; the shader tests isolate the unchanged controls.
No additional retained memory is introduced. Sample counts and random sequences are unchanged.

The full-GI shortcut and fog guard remain unimplemented. The accepted source is saved in
`target/render-audit/bin/step4-isolated`; logs and comparison artifacts use the
`target/render-audit/step4-isolated-*` prefix. Future shader edits must preserve the paired
tests' original-policy reference or update it explicitly.

### 5. Allocate transparency targets on demand (R1, allocation portion)

Opaque-only target sets now use 1×1 OIT textures. Full-size accumulation and reveal textures
are allocated when visible transparency or a glowing path first needs them, retained for later
toggles, and released on resize. A resize or capture that already needs transparency allocates
the full textures directly. Normal-target allocation and attachment behavior are unchanged.

The texture extents are asserted in a new GPU lifecycle test covering hidden transparent
geometry, first use, return to opaque rendering, resize, effects, and capture restoration.
All six eager/lazy image pairs and their captures matched exactly; their comparison sheet was
inspected. Existing transparent-envelope, supplied-mesh, and glowing-path GPU tests passed.
All 364 regular release tests and the wasm32 check passed (three existing browser warnings).

Three same-process runs each measured 31 alternating eager/lazy pairs per case after warmup.
The table reports medians across runs and medians of each run's p95.

| Workload | Eager median / p95 | On-demand median / p95 |
| --- | --- | --- |
| Opaque resize and completed frame | 1.400 / 1.864 ms | 1.403 / 1.850 ms |
| Opaque steady frame | 1.243 / 1.416 ms | 1.235 / 1.323 ms |
| Transparent resize and completed frame | 1.359 / 2.196 ms | 1.376 / 2.133 ms |
| Transparent steady frame | 1.192 / 1.523 ms | 1.179 / 1.361 ms |
| First transparency use after opaque allocation | 1.220 / 1.507 ms | 1.230 / 1.381 ms |

Steady and resize timings remain within measurement noise. First transparency activation has
a small allocation cost; its median paired difference was +2.7%, while its median completed
frame time increased by approximately 0.010 ms. Later toggles reuse the textures.

At the measured 640×480 viewport with 2× SSAA, the two texture formats require 12,288,000 bytes
at full size, versus 10 bytes of image texels for placeholders. This avoids about 11.7 MiB of
format-based texture storage for an opaque-only target set. Actual driver allocations include
alignment and pooling; no equivalent process-RSS reduction is claimed. At a 4K viewport with
2× SSAA, the corresponding format-based saving is about 316 MiB. Retention is bounded to the
current target dimensions, with a separate temporary target during capture.

The saved executable is `target/render-audit/bin/step5`; logs and images use the
`target/render-audit/step5-*` prefix. R1's unused normal attachment remains a separate change.

### 6. Omit unused normal output and share shader modules (R1)

Opaque passes now select color-only pipelines when SSAO does not consume normals. The
late-depth sphere and cylinder path retains its original normal attachment: repeated paired
measurements showed possible slowdowns with color-only output on that path. Mesh-only scenes
and devices with early depth tests use color-only output when effects are absent. The global
AO override and intrinsic Molecular Nodes AO retain their existing behavior. Normal textures
remain allocated at full size; allocation is a separate pending change.

Each geometry shader module is now created once during renderer construction and shared by
its pipeline variants. This offsets the cost of the added color-only variants. In 11 alternating
constructor pairs after warmup, median construction time fell from 178.637 to 94.691 ms without
early depth tests, and from 327.139 to 167.988 ms with early depth tests (47.0% and 48.6%).
The original per-pipeline module construction remains available only as the test reference.

Frame tests alternate attachment policies on the same renderer and targets, with 31 measured
pairs after warmup. Times include submission and GPU completion at 640×480 with 2× SSAA.

| Depth path and workload | Original median / p95 | Candidate median / p95 | Median paired change |
| --- | --- | --- | --- |
| Late depth, surface, effects off | 1.162 / 1.409 ms | 0.772 / 1.180 ms | −32.7% |
| Early depth, surface, effects off | 1.133 / 1.746 ms | 0.867 / 1.486 ms | −20.2% |
| Late depth, spheres, effects off (retained policy) | 2.012 / 2.379 ms | 1.942 / 2.847 ms | +0.7% |
| Early depth, spheres, effects off | 1.440 / 2.613 ms | 1.553 / 2.368 ms | −7.1% |
| Late depth, surface, effects on (control) | 7.419 / 10.156 ms | 7.400 / 11.157 ms | −0.1% |
| Early depth, surface, effects on (control) | 9.645 / 12.569 ms | 9.752 / 14.225 ms | −2.6% |

Sphere effects-on controls had paired changes of −2.0% and +3.4% for late and early depth.
Frame timings fluctuate, including differences between paired ratios and ratios of medians;
no sphere speedup is claimed. The retained late-depth path executes the original attachment
policy. The surface improvement and construction improvement were consistent across repeats.

Both depth modes passed 201 exact image comparisons, covering five representation styles,
five materials, two projections, four effect settings, and selection glow with a hover lens.
The final surface comparison sheet was inspected visually. Lens validation also exposed and
fixed incompatible render-pass attachment slots and a missing envelope bind group. The lens
reference uses compatible color-only pipelines; parity with the previously invalid lens pass
is not claimed. Existing shadow-depth, transparent-envelope, supplied-mesh, COSMO-style,
glowing-path, and shadow-cache GPU tests passed. All 364 regular tests and the wasm32 check
passed, with the same three browser warnings.

The final executable is `target/render-audit/bin/step6-cached`. Accepted measurements and
images use `step6-accepted-standard` and `step6-accepted-early`; regular test results are in
`step6-accepted-unit.log`, and the inspected sheet is `step6-accepted-inspection.png`.

### 7. Allocate normal textures on demand (R1, remaining allocation)

A target set now keeps a 1×1 normal texture until its render path needs full-size normals.
The allocation decision uses the same effects and late-depth-impostor policy as the opaque
pass. First use replaces the SSAO binding, and later toggles retain the texture until resize.
Capture allocates its own matching targets and restores both live textures and bindings.

The new lifecycle test compares eager and on-demand targets through first use, retention,
resize, intrinsic Molecular Nodes AO, and the enabled-zero-AO override at 1× and 2× SSAA.
It also compares captures and renders again after capture to check restored bindings. All
comparisons passed, and the six-case surface sheet was inspected visually. The broader
material/representation test passed all 402 image pairs across late and early depth modes.
The wasm32 check passed with its three existing warnings.

Five same-process runs measured 31 alternating pairs per case after warmup, including two
runs with early depth tests. The table gives medians of run medians and run p95 values at 2×
SSAA. The host was under greater load than during step 6, so these absolute times must not be
compared across steps. All comparisons below use eager and on-demand targets in the same run.

| Viewport and workload | Eager median / p95 | On-demand median / p95 | Median paired change |
| --- | --- | --- | --- |
| 640×480, effects off, resize + frame | 2.895 / 3.937 ms | 2.894 / 4.818 ms | +0.1% |
| 640×480, effects off, steady frame | 2.746 / 4.752 ms | 2.745 / 5.141 ms | +0.4% |
| 640×480, effects on, resize + frame | 21.017 / 28.863 ms | 21.337 / 28.123 ms | +0.5% |
| 640×480, first effects activation | 23.536 / 29.166 ms | 21.765 / 30.833 ms | +0.9% |
| 1920×1080, effects off, resize + frame | 39.928 / 59.485 ms | 9.991 / 12.502 ms | −72.1% |
| 1920×1080, effects off, steady frame | 9.632 / 12.652 ms | 9.634 / 11.166 ms | 0.0% |
| 1920×1080, effects on, resize + frame | 99.986 / 104.790 ms | 100.280 / 104.154 ms | +0.2% |
| 1920×1080, first effects activation | 100.158 / 108.686 ms | 99.755 / 105.658 ms | −0.3% |

One small-viewport resize run had a +19.2% paired difference; the other four were −0.2% to
+0.4%. No reproducible slowdown was found. First use adds an allocation and binding rebuild,
while subsequent toggles reuse them. The large-viewport resize improvement reproduced in all
five runs; no steady-frame speedup is claimed.

The unused normal format would require 9,830,400 bytes at 640×480 with 2× SSAA. The placeholder
uses eight bytes of image texels, avoiding about 9.4 MiB of format-based storage. At a 4K
viewport with 2× SSAA this saves about 253 MiB. These are texture-format calculations, not
measured driver allocation or RSS reductions. The retained texture is bounded by current target
size, and late-depth opaque impostors keep their full normal target under the accepted policy.

The executable is `target/render-audit/bin/step7`. Allocation logs use `step7-normal`,
`step7-early`, and `step7-repeat*`; broader parity logs use `step7-parity-*`. The inspected
comparison sheet is `step7-inspection.png`.
All 364 regular release tests passed (`step7-unit.log`).

### 8. Cache primitive bounds during envelope construction (R5, accepted 2026-11-08)

The change computes each primitive's conservative bounds once during initial tree
construction, then reuses those values for node bounds and split comparisons. The arithmetic,
comparison keys, primitive ordering, tree layout, upload contents, and refit code are unchanged.
The temporary array occupies 24 bytes per primitive and is released when construction returns;
it adds no retained tree or GPU storage. The audit's separate refit-quality policy is not part
of this change.

Five paired CPU benchmark runs completed before testing was stopped. Each run compared 31
alternating reference/candidate builds after warmup and required identical primitive and node
bytes. Median paired reductions ranged from 64.8–70.9% for eight primitives, 82.0–86.4% for
1,024 primitives, and 83.6–96.2% for 16,384 primitives. Host contention caused very large timing
outliers, especially for the largest case; these measurements establish repeated work savings
but do not establish representative interactive latency or a raster-frame speedup. Raw medians
and p95 values are retained in `target/render-audit/step8-cpu-1.log` through `step8-cpu-5.log`.
The 16,384-primitive case uses 384 KiB of temporary bound storage.

Native release compilation and the wasm32 check passed; the latter retained its three existing
warnings. All 365 regular CPU tests passed at low priority on one CPU (`step8-unit.log`).
The differential regression test covers empty, single, mixed, coincident, shifted-lane, smoothed,
and degenerate primitives, and verifies exact tree bytes after refitting moved coordinates.

After the user authorized short GPU checks on 2026-11-08, the reference and candidate each
rendered three 320×240 captures at 2× SSAA, with no benchmark loop. The tests completed in
0.52 and 0.43 seconds respectively on the physical Radeon GPU. All three RGBA image pairs
matched exactly, and the reference/candidate/difference sheet was inspected visually. A
separate short GPU containment test passed for internal faces, shared caps, spheres, rotated
views, and shifted bond lanes. The byte-exact CPU tree tests cover larger and coincident inputs.
No GPU frame-time improvement is claimed for this CPU construction change.

The accepted candidate is `target/render-audit/bin/step8`. GPU logs and images use the
`target/render-audit/step8-short-*` prefix; the inspected sheet is
`step8-short-inspection.png`. Earlier molecule-preview reference images remain available in
`step8-before-images`, but that sustained capture/benchmark test was not repeated. Future GPU
checks should remain short, with sustained benchmark loops avoided while the desktop is in use.

### 9. Reuse surface geometry for material edits (S1, accepted 2026-11-08)

Material-only edits now mark a displayed Surface representation for an appearance update.
The update restamps cached mesh vertices, reuses GPU capacity, and advances the geometry
revision for ray-trace and shadow invalidation. Positions, normals, connectivity, source owners,
and RGB values remain unchanged. Picking and glow geometry do not rebuild for this update.
The picker, script/API command, ordinary representation undo, live material defaults, and
settings restoration use the same material setter. Group-history reconstruction retains its
existing full-build behavior.

Exact opacity replacement requires known original alpha. A cached vertex with nonzero
material opacity can prove an original alpha of 255 when its stored alpha equals that opacity.
If every vertex satisfies this condition, later opacity changes are exact, including a change
through zero opacity. Otherwise only equal-opacity lighting changes reuse the cache; opacity
changes rebuild the surface. This preserves custom color alpha without retaining another mesh
or an alpha array. Only fixed per-representation metadata is added.

Selection, coordinate, color, surface-parameter, and gray-mode changes retain full rebuilds.
Material edits during a pending background build also use the existing cancellation/rebuild
path. Material updates cannot consume stale selection or bond-policy caches. Exact color replay
is still pending; this change does not optimize color edits.

The CPU benchmark uses the complete `2lao.pdb` molecule, ten alternating reference/cached
pairs after two warmup pairs, and one CPU at reduced process priority. The cached timing
includes material stamping and CPU cache installation; the fresh timing includes complete
geometry construction. GPU uploads are excluded. Every pair requires exact vertex bytes,
indices, and source-owner arrays.

| Surface quality | Vertices | Fresh build median / p95 | Cached update median / p95 |
| --- | --- | --- | --- |
| 0 | 5,368 | 48.855 / 54.949 ms | 0.044 / 0.081 ms |
| 2 | 31,985 | 818.474 / 1,660.529 ms | 0.248 / 0.459 ms |

Host contention affected the full-build tail. These timings demonstrate the removed surface
construction work, not a guaranteed application latency or a steady-rendering speedup. Raw
results are in `target/render-audit/step9-cpu-full.log`. The earlier small-surface measurement
is in `step9-cpu-bench.log` and independently showed exact output with lower edit cost.

All 368 regular release tests passed, including exact material updates across all presets,
custom alpha, zero opacity, stale-cache rejection, and settings restoration. The wasm32 check
passed with its three existing warnings. The short physical-GPU integration test completed in
0.29 seconds: five material/edit states, including script commands and undo, each compared
160×120 raster images and 96×72 two-sample ray-traced images. All ten image pairs were exact,
and their comparison sheet was inspected. The test verifies that the vertex allocation is
retained and that cached and explicitly rebuilt ray-trace inputs follow separate paths.

The accepted binary is `target/render-audit/bin/step9`; validation logs are `step9-unit.log`,
`step9-wasm-final.log`, and `step9-gpu-verified.log`. Images are in `step9-images`, and the
inspected sheet is `step9-inspection.png`. No sustained GPU benchmark loop was used.

### 10. Omit unused surface glow color work (S6, accepted 2026-11-08)

Surface selection and hover glows now use a uniform placeholder color. This skips atom-color
lookup work and mesh color diffusion, which the glow shader does not consume. The builder still
constructs the same selected-atom surface, applies the same normal displacement, and retains
the representation's material tag. Other representation styles retain their existing colors.
No parent-mesh subset approximation or additional retained cache is introduced.

Exact CPU comparisons passed for single-atom, residue, and larger selections at qualities 0,
2, and 4, including a mixed Surface/Licorice case. Positions, normals, connectivity, source
owners, and material tags match the original builder. Only unused glow RGB values differ.

Five CPU runs each measured seven alternating full-molecule reference/candidate pairs after
two warmup pairs at quality 2, with one Rayon worker and reduced process priority. The table
reports medians of run medians and run p95 values; the final column is the median of each run's
median paired percentage change.

| Source color scheme | Original median / p95 | Candidate median / p95 | Median paired change |
| --- | --- | --- | --- |
| Element colors | 126.071 / 172.481 ms | 124.084 / 156.873 ms | −6.6% |
| Uniform color (control) | 121.350 / 125.795 ms | 120.107 / 126.188 ms | −3.5% |

No run showed a positive median paired change. Whole-build timings varied substantially, and
the uniform control also shifted, so the complete-build percentage must not be treated as an
isolated color-diffusion speedup. A separate profiled run measured the color stage at a median
2.967 ms for element colors and about 0.00005 ms when skipped. Both uniform-color paths were
about 0.00004 ms. This establishes removal of the unused color stage; no idle-rendering or GPU
frame-time improvement is claimed. The color-filter scratch arrays are no longer allocated
for surface glows, but no peak-RSS reduction is claimed.

CPU logs use `target/render-audit/step10-cpu*`; stage measurements are in `step10-profile.log`.

All 369 regular release tests and the wasm32 check passed (three existing browser warnings).
The short physical-GPU test completed in 0.26 seconds. It compared six 160×120 image pairs
for single-atom hover, residue selection, and the larger selection in both projections. All
pairs matched exactly, and the comparison sheet was inspected. The whole-selection case also
requires a visible difference from the unhighlighted scene. The test reads the live viewport
with a nonzero pulse because file captures intentionally suppress the pending-selection glow.

The executable is `target/render-audit/bin/step10`; remaining validation logs are
`step10-shape.log`, `step10-unit.log`, `step10-wasm.log`, and `step10-gpu.log`. Images are in
`step10-images`, and the inspected sheet is `step10-inspection.png`.

### 11. Reject CPU picking rays outside mesh bounds (S7, accepted 2026-11-08)

Each installed mesh now stores finite vertex bounds. CPU picking first tests the ray against
expanded bounds using double-precision slab arithmetic. Rays that pass this test use the
unchanged triangle scan, including its two-sided intersections, source-owner choice, missing
owners, and first-triangle tie rule. Periodic images share the bounds and translate the ray
as before. Invalid positions or rays disable the rejection. Native GPU picking is unchanged.

Bounds require one vertex scan when installing new geometry and fixed metadata per mesh;
there is no triangle tree or retained duplicate geometry. Material-only updates now preserve
the source-atom list and bounds, avoiding their allocation and reconstruction. Tests verify
that both the mesh allocation and source-atom allocation survive these updates.

Five CPU benchmark runs each measured 15 alternating pairs after three warmup pairs. Each
pair picked 32 rays against 20,002 triangles. Missed bounds reduced the median of run medians
from 9.011 ms to 0.000831 ms. For rays that entered the bounds, the corresponding medians were
9.051 and 8.999 ms; median paired changes ranged from −1.29% to +2.08%, with no consistent
regression. Host scheduling produced large timing tails. These are native CPU measurements
of the shared picking algorithm, not measured browser hover latency. Material-update spot
checks also showed lower cost after retaining metadata; they are not a separate speedup claim.

All 371 regular release tests passed. Differential tests compare exact pick distances and
owners for 6,000 seeded random rays, boundary and degenerate cases, three coordinate scales,
both camera projections, and translated periodic images of a real surface. The wasm32 check
passed with its three existing warnings. The physical-GPU material integration check took
0.35 seconds. All ten raster/ray-traced image pairs matched exactly, also matched the saved
pre-change captures, and passed visual inspection.

The executable is `target/render-audit/bin/step11`. Logs use the `step11-pick`,
`step11-material`, `step11-unit`, `step11-wasm`, and `step11-gpu` prefixes. Images are in
`step11-images`; the inspected comparison is `step11-inspection.png`. A triangle BVH remains
conditional on evidence that rays entering mesh bounds cause unacceptable picking latency.

### 12. Shared surface transpose and early volume release (S5, rejected 2026-11-08)

An experiment shared the transient transpose allocation between EDT and field filtering,
released it before extraction, and released the nearest-atom volume immediately after
extraction. All 372 regular tests passed, including exact complete meshes and partial-tile
grid comparisons with one and two workers. The wasm32 check also passed.

Five paired CPU benchmark runs at two Rayon workers showed effectively unchanged full-build
times at qualities 0 and 3. Their median paired changes were +0.40% and −1.07%, respectively.
However, five alternating separate-process quality-4 probes showed a repeatable slowdown:
median elapsed time increased from 2.12 to 2.38 seconds. Three further profiled pairs showed
median full-build time increasing from 2.071 to 2.382 seconds, with the largest increase in
refinement. These profiles locate the regression but do not establish its allocator or cache
cause.

The quality-4 grid contained 4,400,291 voxels. Median peak process RSS fell from 76.79 to
66.91 MiB, measured with `/usr/bin/time`; the reference range was 71.06–77.27 MiB and the
candidate range was 66.84–67.13 MiB. The memory reduction does not meet the user's requirement
when accompanied by the observed slowdown. Both implementation changes were therefore
removed before proceeding, and no GPU test was needed for this failed performance gate.

The experimental executable remains in `target/render-audit/bin/step12`. CPU, memory, and
stage logs use `step12-cpu`, `step12-memory`, and `step12-profile` prefixes. The two lifetime
changes can be investigated separately later; neither is accepted here. Byte-based job
admission and cancellation improvements remain separate pending work.

### 13. Skip hidden periodic cameras and reuse camera storage (R6, accepted 2026-11-08)

Camera preparation now omits hidden molecules and representations while keeping the nested
molecule/representation indices used by draws. Visible transforms, central and hover cameras,
light-camera placement, draw order, and repeated box rendering retain their existing behavior.
The renderer also reuses its camera-entry vector and packed upload-byte vector. Each retained
allocation is capped at 2 MiB; larger allocations are released after use. This adds at most
4 MiB of retained CPU capacity per renderer. GPU camera-buffer growth is unchanged.

Visibility filtering alone showed a small repeated overhead in the all-visible control.
The accepted implementation includes storage reuse, which removed that regression. Five
release CPU runs each measured 21 alternating pairs after two warmup pairs, with 32 preparations
per timing sample. Timings include camera construction, image-index lists, and byte packing;
they exclude GPU uploads and rendering. The table reports medians of run medians and run p95s.

| Scene | Original median / p95 | Candidate median / p95 | Median paired change |
| --- | --- | --- | --- |
| Eight molecules, 24 periodic representations each, mostly hidden | 0.816 / 0.848 ms | 0.127 / 0.133 ms | −84.4% |
| Same scene, all visible | 0.815 / 0.834 ms | 0.770 / 0.781 ms | −5.5% |
| One visible representation, no periodic box | 0.658 / 0.667 µs | 0.623 / 0.626 µs | −5.2% |

All five runs improved in every case. The hidden case reduced uploaded camera entries from
4,994 to 834 and packed bytes from 1,278,464 to 213,504, excluding a light entry when shadows
are enabled. Run-to-run frequency variation affected absolute times. These measurements do
not establish a whole-frame or GPU timing improvement.

All 373 regular tests passed, including exact visible camera uniforms, unchanged index lookup,
box removal, visibility changes, storage growth/shrink, and zero upload padding. The wasm32
check passed with its three existing warnings. The final physical-GPU test took 0.47 seconds.
Eight 160×120 image pairs covered both projections, visibility changes, different periodic
boxes, opaque and transparent representations, AO, shadows, and box wireframes. All pairs
matched exactly, also matched captures made before storage reuse, and passed visual inspection.

The accepted executable is `target/render-audit/bin/step13-storage`. Final logs and images
use the `step13-storage` prefix; the comparison sheet is `step13-storage-inspection.png`.
Deduplication of equal periodic transforms, reuse of nested image-index vectors, and geometric
GPU buffer growth remain separate candidates. They are not included in this change.

### 14. Intrinsic-AO dispatch accounting (R9, not accepted yet, 2026-11-08)

An experiment detected factory and edited Molecular Nodes material tags in uploaded RT data
and included their intrinsic AO when global AO was disabled. The original transparency and
GI cost estimates remained unchanged. A spatial-only variant preserved accumulation grouping
but increased completion time by 17–29% in the first short test, so it was rejected.

A second variant reduced sample chunks according to the actual tile extent, shrinking the
tile only when one sample exceeded the estimated ray budget. It retained every sample and
its random sequence but changed floating-point summation grouping. Before testing, the image
limit was set to one 8-bit channel level in at most 0.1% of pixels. All tested images were
actually byte-exact: factory and edited intrinsic-AO materials, perspective projection,
enabled global AO at zero strength, ordinary opaque material, transparency, and GI.

All 374 regular tests and the wasm32 check passed. GPU tests used three alternating pairs
per process, discarded the first pair for timing, paused briefly between pairs, and used
one-second gaps between processes. Each process took 0.58–0.93 seconds. No sustained large
viewport benchmark was used.

Five 160×120 runs gave median convergence times of 23.83 ms before and 23.99 ms after, with
mixed paired changes. Five 256×256 runs, where dispatch count increased from two to six,
gave 58.08 and 59.85 ms; every run increased by 0.5–6.1%. A separate timestamped run measured
median maximum GPU dispatch durations of 55.07 and 11.80 ms after warmup. Thus dispatch
latency improved substantially, but the repeatable convergence cost did not satisfy the
no-regression requirement. The scheduler change was removed pending a separate investigation
of submission overhead. No completed implementation of R9 is claimed.

The experimental executables are `target/render-audit/bin/step14`, `step14-final`, and
`step14-sized`. Final small-image logs use `step14-final`; full-tile logs use `step14-large`.
The timestamped run is `step14-large-profile.log`. Saved PNGs are in `step14-final-images`
and `step14-large-images`. The last executable adds only a test viewport-size switch; its
scheduler regression test also passed independently.

### 15. Bounded RT submission batching (rejected 2026-11-08)

An experiment packed up to four unchanged compute dispatches into each command encoder,
using aligned dynamic uniform records. It preserved the caller's dispatch limit, sample
chunks, tile order, accumulator transitions, and resolve policy. Six dispatches used two
submissions instead of six; the opaque control used one submission instead of two.

All 373 regular tests and the wasm32 check passed. A short GPU test compared partial sweeps,
accumulator swaps, target changes, and cancellation/restart at three viewport sizes. All
displayed complete images matched exactly. Final opaque and intrinsic-AO captures also
matched exactly and passed visual inspection. The first GPU process, including pipeline
initialization, took 5.61 seconds. Subsequent timing processes took 0.84–1.36 seconds, with
one-second gaps and a 15-second timeout for each process.

Five 256×256 intrinsic-AO runs used six alternating pairs each and discarded the first pair.
CPU submission time improved in all five runs: medians of run medians were 1.426 and 1.077 ms.
Full completion times were 57.765 and 58.176 ms, with mixed changes from −1.3% to +1.9%.
Eight opaque control runs showed increased completion time in six runs, despite generally
lower CPU submission time. The initial five runs and three follow-up runs both showed this
tradeoff. These measurements do not support a whole-render improvement, and the change does
not pass the no-regression gate. The implementation and experimental tests were removed.
Intrinsic-AO scheduling therefore remains unaccepted; batching has not resolved its cost.

The experimental binaries are `target/render-audit/bin/step15` and `step15-timed`. Timing
logs use `step15-timed-large` and `step15-timed-opaque`; partial-sweep evidence uses
`step15-partial`. The inspected comparison is `step15-inspection.png`. The saved experiment
is `/tmp/step15-batching.patch`; it is not part of the accepted source.

### 16. Reuse prepared inputs for small geometry builds (S4, accepted 2026-11-08)

When native background building is enabled, small surfaces and cartoons now consume their
already prepared owned input synchronously. Previously the offload-size check discarded
that input and the synchronous builder gathered coordinates and colors again. The offload
thresholds, background-job admission, cancellation behavior, and synchronous-only/browser
paths are unchanged. This change adds no retained cache or GPU allocation.

Five release CPU runs used one Rayon worker and 21 alternating measured pairs after two
warmup pairs, with eight complete builds per sample. Timings exclude fixture loading and
the unchanged cartoon topology/secondary-structure caches. The table reports medians of
run medians and run p95 values.

| Scene | Original median / p95 | Candidate median / p95 |
| --- | --- | --- |
| Eight-atom surface, quality 0 | 0.272 / 0.279 ms | 0.213 / 0.221 ms |
| 128-atom surface, quality 0 | 2.992 / 3.244 ms | 2.940 / 3.115 ms |
| Twenty-residue cartoon | 0.126 / 0.132 ms | 0.125 / 0.133 ms |
| Complete 2lao cartoon | 1.725 / 1.838 ms | 1.724 / 1.801 ms |
| 128-atom Vdw control | 0.0499 / 0.0518 ms | 0.0500 / 0.0547 ms |

Surface builds improved in every run: 21.3–23.5% for eight atoms and 1.7–1.9% for 128 atoms.
Cartoon changes were mixed, from −7.4% to +2.0% for the complete molecule, so no cartoon
speedup is claimed. Vdw median changes were −0.04% to +0.34%. Absolute times varied with
system conditions; the paired surface gains persisted across that variation.

All 374 regular tests passed, including exact complete geometry, material tags, source IDs,
gray-out results, synchronous fallback, and the 255/256-atom offload boundary. The wasm32
check passed with its three existing warnings. A 0.52-second physical-GPU check compared
12 pairs at 160×120, covering surfaces/cartoons, both projections, opaque/glass/intrinsic-AO
materials, AO, and shadows. Every image matched exactly and passed visual inspection.

The executable is `target/render-audit/bin/step16`. Logs use the `step16` prefix; saved images
are in `step16-images`, and the comparison sheet is `step16-inspection.png`. Grid-based job
scheduling, byte-based admission, and cancellation within long surface stages remain pending.
