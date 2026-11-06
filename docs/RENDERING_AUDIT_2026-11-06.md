# Shader, rendering, and surface efficiency audit

Date: 2026-11-06. Source revision: `10c1d8c`.

Implementation progress and measured results are recorded in
[Rendering audit implementation results](RENDERING_AUDIT_RESULTS.md).

This document is an actionable implementation plan based on the current source. The primary
review covers shaders, render passes, ray tracing, uploads, and picking. A separate Astra
agent at xhigh reasoning effort reviewed surface computation and its rendering consumers in
parallel. No rendering implementation was changed during the audit.

The findings identify redundant work and candidates for measurement; they are not measured
speedup claims. This environment has no `/dev/dri` device, and this audit did not run hardware
GPU benchmarks or the application test suite. Priorities describe the recommended order of
investigation, not a measured ranking of frame-time costs. All source paths below are relative
to the repository root, and line numbers refer to the audited revision.

## Constraints and existing improvements

The acceptance target is lower execution time or memory use without a reproducible performance
regression in representative workloads. Preserve visual output, surface topology, picking,
transparency, sample counts, and export behavior. No optimization can guarantee identical
performance on every adapter from source inspection alone.

The [October performance plan](RENDERING_PERFORMANCE_PLAN.md) records work that is already
implemented: fixed AO/shadow disk offsets, near-first closest-hit traversal, bounded RT BVH
refits, unchanged-scene preparation keys, reusable GPU capacities, parallel surface EDT and
smoothing stages, bounded native geometry jobs, cached smooth-bond cubic coefficients,
shadow-map reuse, and a one-pixel picking scissor. Retain these changes. Some later sections of
that older plan describe the pre-implementation state; they must not be read as current findings.

## Implementation order

P1 items remove substantial or directly repeated work, or correct a scheduling estimate.
P2 items require a larger ownership change or evidence from a particular workload. P3 items
are experiments that should proceed only after profiling identifies the relevant bottleneck.

| Order | Work package | Priority | Acceptance evidence |
| --- | --- | --- | --- |
| 0 | Complete the measurement coverage described below. | P1 | Record complete-operation time, memory, allocations, and applicable GPU stages. |
| 1 | Remove repeated surface samples and unused intermediate normals; reuse existing mesh buffer capacity. | P1 | Compare mesh output exactly and count samples and GPU allocations. |
| 2 | Resolve RT output only after a new completed sweep or target change (R2). | P1 | Count resolves and compare intermediate and final images. |
| 3 | Gate unused normal/OIT resources and no-op effects (R1, R8). | P1 | Compare effects and material combinations, memory, and pass timings. |
| 4 | Separate surface appearance updates from shape construction. | P1 | Confirm appearance edits do not run SES construction and preserve picks. |
| 5 | Fix intrinsic-AO accounting in RT scheduling (R9). | P1 | Measure dispatch duration and time to the final image without changing samples. |
| 6 | Reject known transparent blockers early and improve envelope maintenance (R3, R5). | P2 | Compare hit predicates and containment results; record traversal counts. |
| 7 | Split RT preparation by representation and remove unused camera work (R4, R6). | P2 | Record per-buffer upload bytes and unchanged-group cache hits. |
| 8 | Improve surface memory lifetime, cancellation, and parallel stages, then picking (R7). | P2 | Measure peak RSS, cancellation latency, full-build time, and hover latency. |
| 9 | Evaluate the conditional shader and submission experiments. | P3 | Require hardware measurements before enabling a change. |

Keep each package reviewable and benchmark it separately. Surface details and their dependencies
are recorded in the surface findings later in this document.

## Measurement and acceptance gates

The existing profiler provides useful CPU scopes and asynchronous, bounded GPU timestamps.
Extend it before making comparative claims:

1. Add surface scopes for input preparation, occupancy, extraction, relaxation, refinement,
   coloring, result installation, and pick construction. Existing surface scopes cover EDT
   and field smoothing but do not isolate all of these stages.
2. Count indexed mesh, pick, envelope, and RT uploads and allocations. The counters in
   `render.rs:484–530` cover instance buffers, while `upload_indexed` and `update_indexed`
   at `render.rs:538–581` do not emit equivalent counters. RT upload at
   `render/raytrace.rs:1288–1324` also needs byte counters.
3. Record RT complete-sweep time, total convergence time, dispatch and resolve counts, and
   maximum dispatch duration. Tile GPU time alone is not frame time. Record envelope build,
   refit, and traversal costs separately.
4. Record adapter, driver, release build, viewport, SSAA, effects, representation counts,
   vertex/triangle counts, periodic images, surface grid dimensions, thread count, peak RSS,
   and GPU allocation sizes. Measure interaction latency as well as throughput.

Use five or more repeated release runs with a fixed camera path and coordinate frames, after
warmup. Compare median and p95 values and report variability. Use a small molecule, a large
atom/bond scene, a large surface, a surface plus Lines or interaction paths, multiple transparent
representations, and periodic images. Exercise both projections, enabled and disabled effects,
1080p and 4K, and supported SSAA settings. Include static edits, camera motion, playback,
hover/picking, captures, and idle behavior. Validate native and browser paths; measure on both
discrete and integrated hardware when available.

Accept a change only when its target improvement exceeds measurement noise and unaffected
workloads have no reproducible slowdown outside that noise. A 10% target-stage improvement is
a useful prioritization threshold, not a requirement for retaining a small exact improvement.
Report memory tradeoffs explicitly, and reject unbounded retained grids or queues. Source-level
work removal does not establish a GPU timing gain because compilers can already remove or
reorder work.

For changes that preserve arithmetic order, require exact geometry and same-adapter image
comparisons. If arithmetic order changes, define numerical tolerances before implementation
and check geometry, depth, normals, colors, silhouettes, and picking. Do not use tolerances to
hide a change in scientific surface shape or topology.

Existing commands provide starting points:

```bash
MOLAR_VIS_PROFILE=1 RUST_LOG=molar_vis_core=info cargo run --release -p molar_vis -- tests/2lao.pdb 2>/tmp/molar-profile.log
python3 scripts/summarize_render_profile.py /tmp/molar-profile.log --warmup 5
cargo test -p molar_vis_core --lib
cargo check --workspace
cargo check -p molar_vis_core --target wasm32-unknown-unknown
```

Select GPU regressions separately on an available adapter, for example:

```bash
cargo test -p molar_vis_core render::appearance_tests -- --include-ignored --nocapture
cargo test -p molar_vis_core near_first_traversal_matches_original_mixed_hits -- --ignored --nocapture
cargo test -p molar_vis_core benchmark_parallel_z_and_field_smoothing --release -- --ignored --nocapture
```

The surface benchmark measures selected grid stages, so it must be supplemented with a complete
surface build benchmark. CPU scopes can nest and must not be summed as frame time. The current
summary script removes warmup timing samples per stage but sums upload counters across the
whole log; separate warmup counters before comparing steady-state upload totals. Software GPU
results can validate behavior but cannot establish physical GPU performance.

## Rendering and shader findings

### R1. Opaque rendering writes an unused normal target, and optional targets allocate eagerly

**P1; confirmed resource and pass behavior.** `render.rs:326–330` allocates normal, OIT
accumulation, and reveal textures at full render resolution for every target set.
`render.rs:1662–1669` attaches, clears, and stores the normal texture whenever the adapter
supports OIT, even if AO, shadows, and intrinsic material AO are absent. The effects pass has
a separate gate at `render.rs:1707–1712`. OIT draws already skip opaque-only scenes; their
textures do not.

The normal texture uses `Rgba16Float` at `render.rs:49`. At a 3840×2160 viewport with 2×
linear SSAA, its raw storage is about 253 MiB. The two OIT textures add about 316 MiB at that
render size. These are format-based allocation estimates, excluding alignment, compression,
and other render targets; they are not measured memory or bandwidth savings.

**Action.** Compute one effective-effects policy before the opaque pass. Select a compatible
color-only pipeline when no later pass consumes normals. Make normal and OIT targets optional,
or use minimal placeholders for bindings that must exist. Allocate full targets on demand and
retain them under a bounded policy to avoid repeated allocation during toggles. Stage the
attachment change separately from allocation changes to isolate its benefit.

**Validation.** Test global AO, shadows, Molecular Nodes intrinsic AO, flat/velvet materials,
transparent-only scenes, glowing paths, hover lenses, capture target swapping, resize, and
WebGL fallback. Compare images with effects on and off, and measure opaque-pass time and
allocation peaks. Preserve intrinsic AO even when global AO is disabled.

### R2. Progressive RT repeatedly resolves an unchanged completed image

**P1; confirmed redundant pass.** `render/raytrace.rs:1532–1576` advances a bounded number of
tiles and resolves whenever `total_samples > 0`. The completed-image index and sample count
change only at the end of a full tile sweep (`1562–1569`). Calls between sweep completions
therefore run `resolve_into` on the same completed accumulator. Each resolve creates and
submits a full-image render pass (`1439–1466`). The viewport advances four tile submissions
per call (`app/viewport.rs:9,322`), so large images can repeat this pass many times per sweep.

**Action.** Track the last resolved completed-image generation and destination generation.
Resolve only when a sweep completes or the destination needs initialization. Preserve the
last complete image during a partial sweep. Cover captures and the blocking export path as
well as the viewport.

**Validation.** Assert that a multi-tile partial sweep causes no repeated resolve to the same
target. Compare every displayed completed image and the final captured image. Test cancellation,
restart, resize, a new destination, and completion within one call. Measure resolve counts,
GPU time, and total time to the requested sample count.

### R3. Secondary rays intersect primitives that cannot block them

**P2; confirmed late filtering, workload-dependent benefit.** In
`render/shaders/raytrace.wgsl:374–405`, `any_hit` performs sphere, cylinder, or triangle
intersection before rejecting opacity below 0.999. Smooth cylinders may run the 192-step
solver before this rejection. Opaque-only closest-hit queries also filter after intersection
at `raytrace.wgsl:331–349`.

**Action.** Add a conservative pre-intersection rejection for primitives that are fully below
the opacity threshold. A sphere has one opacity; a cylinder needs both endpoint opacities
checked; a triangle needs an opacity upper bound across its vertices. Keep the existing
position-dependent check for mixed-opacity primitives. Consider per-node opaque-candidate
metadata only after measuring the simpler rejection.

**Validation.** Compare any-hit and opaque-only results against the current shader for mixed
alpha values, gradient triangles, smooth bonds, source-triangle exclusions, and closed-surface
exit rules. Measure intersection counts in opaque and transparency-heavy scenes; extra
classification must not slow the common opaque case.

### R4. One view-dependent representation invalidates preparation of the whole RT scene

**P2; confirmed scope of invalidation.** `PreparedKey::new` at
`render/raytrace.rs:1005–1033` includes the view if any visible geometry depends on it, and
returns no reusable key for Interactions. A miss recollects the whole scene at `1277–1279`,
then writes all six storage arrays at `1318–1323`. Consequently, adding a small Lines
representation, box, or interaction path can make camera motion reconvert and upload a large
unchanged surface. Existing BVH refits and capacity reuse do not remove this conversion or
transfer work.

**Action.** Cache converted data per representation with explicit shape, appearance, view,
periodic-image, and cross-molecule interaction dependencies. Begin with unchanged mesh and
sphere ranges while retaining the existing global BVH. Track changed ranges for uploads.
Use separate bounds revisions so recoloring does not force geometric refits. Consider a
top-level instance tree only if periodic duplication remains important after this simpler work.

**Validation.** Compare full rebuild and cached output after camera motion, coordinate changes,
recoloring, visibility, molecule removal, periodic-box edits, and interaction partner edits.
Measure a large surface alone and the same surface plus a small view-dependent representation.
Keep accumulation invalidation independent of whether geometry buffers require an upload.

### R5. Raster envelope trees can lose quality indefinitely, and initial construction repeats bounds work

**P2; confirmed maintenance policy, unmeasured traversal degradation.**
`render/envelope.rs:242–248` refits whenever the total primitive count matches, with no refit
limit or tree-quality check. Large coordinate rearrangements can therefore retain poor spatial
partitions indefinitely. This is distinct from the RT tree, which already has bounded refits.
During initial construction, `subtree` recomputes primitive bounds in each subtree scan and
again in the selection comparator (`90–123`); bounds include distance/hypotenuse calculations
at `79–87`.

**Action.** First precompute bounds and centers once per build, preserving comparator keys and
tie behavior. Then measure refitted tree quality and add a rebuild trigger based on a calibrated
cost or refit count. Reuse the CPU primitive vector and rebuild on meaningful topology changes.
Account for both rebuild cost and the fragment traversal cost it can save.

**Validation.** Compare containment decisions and transparent images against fresh trees over
long trajectories, frame jumps, and count-preserving rearrangements. Include coincident caps,
offset strands, smoothing, and periodic images. Record node visits, OIT time, build/refit time,
and the worst rebuild frame; a fixed rebuild interval can otherwise introduce avoidable stalls.

### R6. Camera preparation includes hidden representations and duplicate image transforms

**P2 for scenes with many representations; confirmed CPU work.** `render.rs:1518–1549`
creates periodic camera entries for every molecule and representation before the draw loop
filters visibility at `2473–2482`. Equivalent image transforms are repeated across
representations. Each rendered frame also allocates nested vectors and a zero-filled upload
array at `1594`.

**Action.** Skip hidden molecules and representations during camera preparation, while retaining
the indices expected by draw consumers. Deduplicate identical nonzero image transforms within
a molecule, reuse CPU vector capacity, and use bounded geometric growth for the GPU camera
buffer. Keep central, hover, and light-camera entries explicit.

**Validation.** Test visibility toggles, repeated image offsets, cell-wireframe display, multiple
molecules with different boxes, and captures. Compare dynamic offsets and rendered output.
Measure camera preparation time and uniform bytes with many hidden or periodic representations.

### R7. A one-pixel pick still processes all submitted mesh vertices

**P2; confirmed submitted work, benefit requires measurement.** The pick pass uses full-viewport
targets and clears them (`render.rs:1221–1253`), then applies a one-pixel scissor at `1262`.
It submits all visible pick spheres and mesh indices at `1264–1282`. The scissor reduces
fragment work but does not provide a CPU or GPU spatial candidate list. Attachment clear cost
is backend-dependent and must not be assumed to equal a full uncompressed memory write.

**Action.** Add conservative pick bounds and draw ranges, then submit only groups intersecting
the cursor ray or pixel footprint. Investigate a small pick target with a projection that maps
the same original pixel only if target allocation/clear cost is material. Share surface pick
data and periodic transforms as described in the surface findings.

**Validation.** Compare IDs and depth with the current picker at viewport edges, near-plane
clipping, both projections, periodic images, occluding supplied meshes, and mesh boundaries.
Measure hover latency, vertex workload, CPU preparation, and memory separately. Avoid adding
spatial maintenance that costs more than it saves during playback.

### R8. Zero-strength effects and full-strength GI retain avoidable shader work

**P1 for simple guards; confirmed source paths, GPU benefit unmeasured.** Raster shadow
enablement at `render.rs:1557` checks the enabled flag but not strength. The shader at
`render/shaders/ssao.wgsl:28–78` can perform 128 comparison samples and then mix the result
with weight zero. RT `shadow_at` at `raytrace.wgsl:574–599` has the equivalent zero-strength
case. `shade_surface` at `689–701` computes tier-1 lighting, AO, and shadows even when the GI
mix weight is exactly one. Fog at `lighting.wgsl:275–284` also evaluates its curve at zero
strength; a compiler may already optimize some of this work.

**Action.** Add exact no-op guards for valid zero-strength effects and skip shadow-map updates
when their result cannot contribute. Preserve intrinsic AO semantics when global AO is enabled
with strength zero. Add a full-GI branch that bypasses tier-1 work only for GI-eligible materials.
Retain the same random-number sequence and all sample counts. Treat the fog guard as a smaller
follow-up after profiling.

**Validation.** Compare exact endpoint settings and values just above zero or below one, all
specialized materials, fog, opacity, and captures. Preserve shader validation on every backend.
Measure both endpoint and ordinary mixed-effect scenes to detect branch-related regressions.

### R9. RT dispatch budgeting omits intrinsic material AO

**P1 scheduling correction; confirmed mismatch.** `render/raytrace.rs:1494–1500` budgets AO
rays only when global AO is enabled. `ambient_visibility` at
`render/shaders/raytrace.wgsl:614–625` also casts four rays for Molecular Nodes when global AO
is disabled. In an opaque Molecular Nodes scene without other effects, the nominal estimate
can therefore be one traversal per sample while a shaded hit requests five. Actual cost also
depends on misses and traversal complexity; the current ray budget is already a heuristic.

**Action.** Track whether the prepared scene can use intrinsic AO, and include it in dispatch
budgeting. Keep shader effect counts and CPU estimates in one tested contract. Calibrate the
budget with maximum dispatch duration; do not reduce total samples or enlarge submissions
without measuring responsiveness and watchdog risk.

**Validation.** Cover global AO on/off/zero strength, intrinsic AO materials, mixed scenes,
transparency, and GI. Check sample totals and final images, then measure maximum dispatch time,
UI latency, and total convergence time. Smaller dispatches can increase submission overhead,
so report that tradeoff rather than claiming an automatic throughput improvement.

## Conditional shader and submission experiments

These are lower-priority candidates, not established bottlenecks.

| Candidate and evidence | Action and acceptance requirement |
| --- | --- |
| SSAO performs up to 128 depth loads and reconstructions per shaded pixel, and soft shadows add 128 comparisons (`shaders/ssao.wgsl:65–70,146–162`). | Keep sampling and resolution fixed. Measure precomputed uniform projection factors or equivalent reconstruction algebra; preserve sample locations, tangent-plane rejection, and boundary behavior. Do not assume a change to kernel storage is faster. |
| RT containment walks the global BVH and filters envelope groups at leaves (`shaders/raytrace.wgsl:236–278`). | If containment dominates, test group metadata or per-group trees. Preserve image/representation isolation, tolerance, and coincident-primitive ownership; include extra memory and construction costs. |
| Closest-hit traversal tests child bounds for ordering and tests bounds again when popping (`shaders/raytrace.wgsl:313–328`). | Test storing entry distances with pending nodes. Extra stack storage can hurt occupancy or spill, so keep the current path unless hardware timing improves. Recheck tie rules and the depth-31 construction bound. |
| Sphere and cylinder billboards deliberately oversize their extents (`shaders/sphere.wgsl:87–93`, `shaders/cylinder.wgsl:137–145`). | Measure discarded fragments before deriving tighter conservative projected bounds. Validate camera-inside, near-plane, grazing, end-on, and shifted smooth bonds. Do not simply reduce the multipliers. Front-to-back drawing also needs equal-depth and transparency-order tests. |
| Smooth-bond tracing retains a 192-step bound; offset fields recompute atom radii in `bond_offset_surface` (`shaders/bond_profile.wgsl:43–54,165–183`). | Check compiler output and iteration distributions before caching more values. Additional cached vectors can increase registers. A hybrid solver needs a high-accuracy reference; changing epsilon or the iteration cap is outside this plan. |
| Each RT tile writes one uniform and submits its own encoder (`render/raytrace.rs:1544–1558`). | Batch only if submission overhead dominates after R2. Give each dispatch distinct uniform storage or dynamic offsets; repeated writes to one uniform followed by one submission are not a correct substitute. Bound queue depth and cancellation latency. |
| Renderer initialization recreates shader modules and layouts for pipeline variants (`render.rs:905–944`, `render/cylinder.rs:105–120`). | Cache modules/layouts by actual shader variant and measure cold/warm startup and first use. Preserve early-depth and envelope feature variants; do not move a startup stall into the first interaction without measuring it. |
| Unobstructed scoring waits for each four-direction batch and copies a fixed correction buffer (`render/unobstructed.rs:272–374`). | Profile search latency before adding a bounded readback ring. Start with clearing only the correction count and processing only valid entries without cloning the full mapped buffer. Preserve exact CPU boundary corrections and cancellation behavior; the wait currently bounds work and runs in the background job path. |

Reduced AO or shadow samples, lower effect resolution, half-precision RT accumulation, looser
surface tolerances, mesh decimation, shorter transparency walks, and automatic quality changes
are excluded. They change quality or numerical behavior and are not substitutes for the exact
work-removal opportunities above.

## Surface computation and rendering findings

The following findings come from the parallel Astra xhigh review. Its measurement prerequisite
(S0) is merged into the common measurement gates above. The findings retain separate priorities
for material updates, recoloring, and larger algorithm changes.


### S1. Avoid a complete SES rebuild for material and color edits

**Priority and confidence.** P1 for material-only updates; P2 for exact color replay. The redundant geometry construction is confirmed.

**Evidence.** Material selection sets `geom_dirty` (`crates/molar_vis_core/src/app/pickers.rs:695`), as do color choices and solid-color drags (`:404`, `:425`). This enters the full builder at `crates/molar_vis_core/src/app/build.rs:908`, including the surface construction at `crates/molar_vis_core/src/geometry.rs:437`. Material stamping occurs only after geometry construction (`geometry.rs:446`, `:450`), while grid occupancy, EDT, positions, normals, and refinement decisions do not depend on color or material (`surface.rs:130`, `:794`, `:840`).

**Action.** First add an appearance dirty state and restamp material/opacity on the cached mesh without rerunning SES construction. Then consider an exact recoloring cache that retains the coarse vertex source labels, coarse triangle connectivity, and the midpoint-parent recipe from refinement. Recompute base colors and the existing diffusion on the coarse mesh, then replay midpoint averages in the original order. This avoids retaining the full scalar grid merely to recolor a surface.

**Risks.** Recoloring final vertices directly from `vert_atom` is not equivalent: color diffusion precedes refinement (`surface.rs:208`), midpoint colors average both endpoints (`:882`), and the midpoint pick owner inherits only one endpoint (`:896`). Material/opacity changes still require the relevant raster, ray-trace, transparency, shadow, and accumulation invalidation. Cover UI, API, undo, settings, and source-update mutation paths; preserve the draw-mode gray policy.

**Validation.** Compare cached updates with fresh builds for exact positions, normals, connectivity, source IDs, and packed colors/materials. Test uniform-to-nonuniform and reverse changes, secondary-structure/charge colors, opacity changes, pending background work, gray editing, and undo. Confirm that material edits execute zero EDT/extraction work and that final raster/trace images and picking match fresh builds. Record color-cache memory and edit latency.

### S2. Reuse GPU buffer capacity after synchronous surface rebuilds

**Priority and confidence.** P1. The repeated allocation and available reuse path are confirmed.

**Evidence.** Every trajectory frame marks a Surface representation `geom_dirty`; its comment says changing topology prevents an in-place GPU update (`crates/molar_vis_core/src/scene.rs:915`). The synchronous full-build path assigns `rep.gpu = renderer.upload(...)` (`crates/molar_vis_core/src/app/build.rs:950`), which recreates mesh vertex and index buffers (`crates/molar_vis_core/src/render.rs:538`). However, `SceneRenderer::update` already calls `update_indexed`, which writes both vertices and indices, updates active counts, grows storage when needed, and releases empty geometry (`render.rs:563`, `:1385`). Background completion already uses this path (`app/build.rs:862`).

**Action.** Install synchronous rebuilt surfaces through `SceneRenderer::update`, while continuing to rebuild their CPU geometry and upload their new indices. Correct the outdated topology comment. Consider geometric buffer-capacity growth only after measuring size oscillation and memory cost.

**Risks.** The optimization must not assume stable topology or skip index uploads when counts match. Clear obsolete primitive buffers when a representation changes kind, and preserve envelope creation/removal and geometry revisions.

**Validation.** Extend the existing `mesh_update_replaces_indices_when_counts_match` coverage with real surfaces that grow, shrink, become empty, change connectivity without changing counts, and alternate representation kinds. Compare rendered images and picks with fresh uploads. Count indexed-buffer allocations and bytes per frame; require bounded retained GPU memory.

### S3. Remove repeated cubic sampling and invariant work

**Priority and confidence.** P2. The redundant source-level work is confirmed; its runtime contribution needs a release-build profile.

**Evidence.** Each cubic field/gradient evaluation accesses a 4×4×4 neighborhood (`crates/molar_vis_core/src/geometry/surface.rs:735`). Refinement evaluates the initial midpoint to choose an edge split (`:866`), sets `p = initial`, then immediately evaluates the same point again in the first projection iteration (`:873`). Relaxation computes normals at the end of both iterations (`:797`, `:830`), although the second iteration reads only vertex positions and field samples before overwriting those normals. It also clears and rebuilds the same integer neighbor counts on both iterations (`:799`, `:810`).

**Action.** Reuse the already computed midpoint value and gradient for the first projection step, while preserving the existing three-step projection sequence. Compute relaxed normals only after the final relaxation iteration, retaining the original behavior for isolated vertices. Count fixed topology incidences once outside the relaxation loop. Keep the initial Surface Nets normals because they determine triangle winding (`:665`).

**Risks.** Removing a projection iteration, reusing a gradient after a position update, changing floating-point reduction order, or removing the initial winding normals would change geometry or shading. Compiler optimization may already remove some work; measure the generated release path rather than promising a fixed saving.

**Validation.** Require bitwise equality of all final vertex fields and indices across quality/smoothing levels, isolated atoms, overlapping atoms, thin necks, cavities, and zero probe radius. Record cubic sample counts and relaxation/refinement timings; accept the change only if total build time does not regress outside measurement noise.

### S4. Base background scheduling on grid work and make cancellation responsive inside stages

**Priority and confidence.** P1 for interactive responsiveness. The admission rule and cancellation gaps are confirmed; their latency is unmeasured.

**Evidence.** Offload eligibility is only `atom_count() >= 256` (`crates/molar_vis_core/src/geometry_jobs.rs:15`), although surface work depends on the bounding-box voxel count and quality (`crates/molar_vis_core/src/geometry/surface.rs:96`). A small selection with distant atoms can therefore cause a large synchronous grid build. Preparation also gathers an owned input before testing the atom threshold, then discards that input and calls the synchronous builder for small selections (`crates/molar_vis_core/src/app/build.rs:693`, `:701`, `:709`). Cancellation is polled during atom rasterization and between major stages (`surface.rs:135`, `:174`, `:190`, `:195`, `:207`, `:210`), but not within EDT, all smoothing passes, extraction, relaxation/color filtering, or refinement. A canceled worker keeps its budget permit until the build returns (`geometry_jobs.rs:93`).

**Action.** Estimate dimensions and memory before scheduling and use a combined work estimate, rather than atom count alone. Avoid gathering a surface input twice when choosing the synchronous path. Add cancellation checks at bounded plane, slab, pass, vertex, or triangle batch boundaries, with a shared flag for Rayon work. Keep the current synchronous capture barrier and the explicit trajectory policy.

**Risks.** Per-voxel atomics can erase the benefit; poll at coarse boundaries. A partially built mesh must never enter the display cache. This change should not silently enable asynchronous trajectories, whose displayed frame, picking, and highlights need a separate coherence design.

**Validation.** Force cancellation in each stage and verify prompt permit release, no stale installation, and bounded pending work. Measure the p95 delay from a slider change to the newest completed mesh and the CPU time spent on canceled builds. Include sparse selections below the old threshold, single-thread Rayon, two occupied permits, worker failure, and synchronous captures.

### S5. Reduce temporary volume memory and budget simultaneous builds by bytes

**Priority and confidence.** P1 for large systems; P2 for more invasive memory-layout changes. The buffer sizes and lifetimes are confirmed; peak process RSS is not measured.

**Evidence.** The 32-million-voxel limit caps voxel count, not memory (`crates/molar_vis_core/src/geometry/surface.rs:30`). EDT/filter stages can simultaneously hold `nearest`, `field`, and one transpose, each four bytes per voxel (`:132`, `:172`, `:304`, `:391`): approximately 366 MiB at the cap before atoms, mesh storage, allocator overhead, or the previous displayed mesh. Extraction instead holds `nearest`, `field`, and a nearly full-volume `cell_vert` map (`:549`) while producing geometry. Two allowed jobs can therefore require roughly 732 MiB for these arrays alone. `nearest` is last used by extraction (`:196`) but has no explicit early drop before relaxation and refinement. EDT and smoothing separately allocate their transpose volumes (`:304`, `:391`), and refinement builds an edge map (`:854`).

**Action.** First explicitly release `nearest` after extraction and use a shared transient transpose allocation across EDT and field filtering, releasing it before extraction. Add a byte estimate to job admission so two near-cap jobs do not have to run together. Measure a bounded reusable workspace for repeated builds. Only then consider replacing the full `cell_vert` volume with rolling slabs while preserving output order and boundary ownership.

**Risks.** Keeping a reusable full-volume scratch buffer alive during extraction can increase the peak memory it was intended to reduce. Limit retained capacity and release it under memory pressure/removal. Do not reduce resolution, smoothing, projection, or refinement tolerances to meet the budget; serialize oversized jobs instead. A rolling cell map must preserve all cross-slab quads and deterministic indexing.

**Validation.** Measure allocation count and peak RSS for one/two near-cap jobs, repeated quality changes, cancellation, and repeated grow/shrink builds. Require unchanged field and mesh output for lifetime/scratch changes. If rolling slabs are implemented, verify watertightness, edge incidence, winding, pick ownership, and exact triangle order or an explicitly validated tie policy.

### S6. Skip unconsumed surface color work in selection and hover glows

**Priority and confidence.** P2. The color work has no rendering consumer in the glow path.

**Evidence.** `build_glow` calls the ordinary geometry builder with the representation's color method (`crates/molar_vis_core/src/app/build.rs:152`). For a surface this includes color diffusion, despite the function's own statement that element colors/materials are irrelevant (`:103`). The surface chooses 4–64 color passes for nonuniform atom colors (`crates/molar_vis_core/src/geometry/surface.rs:201`, `:245`). The mesh glow fragment shader uses normal, view position, and the camera glow color; it does not read the per-vertex color/material (`crates/molar_vis_core/src/render/shaders/mesh.wgsl:138`).

**Action.** Add a surface build mode for glow geometry that omits colorizer work and color diffusion while retaining the identical field, positions, normals, triangle decisions, and normal inflation. A uniform placeholder color can be used if it preserves the builder contract; a dedicated flag makes the dependency clearer.

**Risks.** Replacing subset SES construction with a parent-mesh subset changes the highlight geometry and is not an exact optimization. Keep the existing subset geometry and `GLOW_INFLATE` behavior (`app/build.rs:155`) for this change. The shared vertex shader changes its normal transform for the reserved glowing-path material tag (`render/shaders/mesh.wgsl:68`), so retain the ordinary surface classification if material stamping is also omitted.

**Validation.** Compare final glow positions/normals/indices and rendered glow pixels for single-atom hover, whole residues, large lasso selections, multiple representations, and both projections. Measure selection-glow/hover rebuild latency; do not expect an idle-frame FPS improvement because glow geometry is already dirty-driven.

### S7. Accelerate CPU surface picking on browser builds

**Priority and confidence.** P2, or P1 if browser hover profiles show stalls. The linear scan is confirmed; its user-visible cost is workload dependent.

**Evidence.** The browser hover path calls CPU picking (`crates/molar_vis_core/src/app/viewport.rs:461`). Each mesh image invokes `ray_mesh` (`crates/molar_vis_core/src/pick.rs:510`), which tests every triangle without even a mesh AABB rejection (`:574`). The native hover path already uses an asynchronous GPU pick and should not be replaced on this evidence.

**Action.** Store conservative mesh bounds in `MeshCache` and add an AABB rejection first. If needed, build a compact per-mesh triangle BVH once per mesh revision and transform the ray by the periodic image offset, sharing the tree across images. Reuse the result on unchanged cursor/view/geometry state as the native path already does.

**Risks.** Preserve two-sided intersections, nearest-vertex source ownership, missing-source behavior, and the current first-triangle result for equal distances. Avoid building a BVH for small meshes where construction costs dominate. Refit only if triangle identity/connectivity is known to remain stable.

**Validation.** Compare accelerated and brute-force pick distances/source IDs for seeded random rays, edge/vertex ties, inside origins, degenerate triangles, both projections, and periodic images. Measure browser hover p95 latency, BVH build time, and retained memory over trajectories and selection changes.

### Further surface candidates that need profiling or a broader design

- **Invisible surface builds are confirmed but need lifecycle review.** The dirty-rebuild loop checks visibility for picking, but the geometry loop itself has no molecule/representation visibility gate (`crates/molar_vis_core/src/app/build.rs:802`, `:840`, `:908`). Deferring invisible surface jobs could save work for hidden ensemble members. Keep dirty revisions until a visible consumer requests geometry, and make capture/API behavior explicit; test hide/edit/show, group-member changes, removal, and pending jobs.
- **Surface Nets extraction remains serial and volume-wide.** It scans all cells for vertices and all grid points for faces (`crates/molar_vis_core/src/geometry/surface.rs:580`, `:676`). If stage timings show that it dominates after parallel EDT/filtering, prototype deterministic slab extraction with prefix offsets and shared-boundary rules. Its sequential form alone is not evidence that a parallel rewrite will improve total time.
- **Refinement edge bookkeeping and color gathering may become bottlenecks.** Refinement caches every unique edge, including unsplit edges, in a `HashMap` (`surface.rs:854`); color filtering repeatedly scatters triangle-mate values (`:263`). Preallocated edge storage or a stable CSR neighbor list may reduce overhead, but needs a profile and must preserve triangle multiplicities and accumulation order for exact color output.
- **Periodic mesh copies multiply downstream cost.** Raster rendering reuses mesh buffers across image transforms, but pick preparation copies positions/indices for every image (`crates/molar_vis_core/src/app/build.rs:243`), and ray-trace conversion duplicates vertices/triangles (`crates/molar_vis_core/src/render/raytrace.rs:429`). Per-instance picking and a top-level/per-mesh ray-trace hierarchy are plausible larger changes. Preserve per-image pick IDs, transparent-envelope isolation, and secondary-ray visibility; benchmark instance count scaling before committing to them.

## Completion checklist for implementation

- [ ] Each accepted change records its baseline, affected workload, image/geometry comparison,
  median and p95 timings, variability, and memory cost.
- [ ] The implementation preserves the existing surface EDT, field smoothing, zero-probe,
  shared-edge color, changed-index upload, shadow reuse, transparent-envelope, and
  smooth-surface secondary-ray regression coverage.
- [ ] New cache tests cover UI, scripting/API, undo, visibility, coordinates, periodic images,
  pending jobs, and capture barriers wherever those mutations affect the changed cache.
- [ ] Native and browser builds pass, and affected GPU tests run on a suitable adapter.
- [ ] The measured gain survives an end-to-end workload, with no reproducible regression
  outside measurement noise in unaffected cases. Idle remains idle.
- [ ] The documentation distinguishes implemented changes from experiments and retains a
  reference path until the corresponding behavior and performance gates pass.

This audit is complete as a source review and implementation plan. Implementation, new
regression tests, and comparative performance measurements remain future work.
