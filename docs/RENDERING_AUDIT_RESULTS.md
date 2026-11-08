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
