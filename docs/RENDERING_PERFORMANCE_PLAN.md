# Remaining rendering performance work

Status: 2026-10-09. This plan follows the performance changes currently in the working tree.
The items below are proposed work unless marked complete. Priority is based on code inspection,
not a measured ranking of GPU bottlenecks. Profile representative scenes before choosing the
next implementation. Preserve the current rendering quality by default.

## Sequential implementation record

- Step 0: timing instrumentation implemented. `MOLAR_VIS_PROFILE=1` emits JSONL CPU scopes,
  adapter/viewport/effect metadata, instance upload sizes/allocation counts and asynchronous
  GPU pass timings when the adapter supports timestamps. Pending GPU readbacks are bounded
  to four. GPU timestamp validation and native/browser compilation pass. Use
  `scripts/summarize_render_profile.py` for warmup removal, median, p95 and range.
  GPU allocation counts currently cover instance buffers; CPU allocation/peak-RSS measurement
  uses an external profiler or `/usr/bin/time -v`. Hardware/browser benchmark matrix pending:
  this environment has software Vulkan and no physical GPU device.
- Step 1: fixed AO/shadow kernel implemented. Original-shader image comparisons pass for
  spheres, cartoons and surfaces, both projections, AO/hard/soft shadows at 1×/4× SSAA.
  Retains 128 samples and full
  effect resolution; half-resolution, reduced sample presets and temporal filtering remain
  experiments requiring hardware timing and image evaluation.
- Step 2: prefix/suffix SAH split evaluation and a depth-31 leaf fallback implemented.
  CPU tests confirm exact ordinary-tree/node order and conservative adversarial bounds.
  Five-run median development benchmarks show 1.04–1.05× whole-builder improvement
  (below the suggested 10% target; retained as a small exact improvement). Near-first
  GPU traversal passes original-versus-optimized mixed-hit comparisons for 1,036 rays,
  with the old right-first tie behavior preserved. Any-hit ordering remains unchanged.
- Step 3: exact preparation keys skip collection, BVH work and uploads for unchanged,
  camera-independent scenes. Stable type counts permit bound refits, with rebuilds after
  eight refits or a 1.5× normalized SAH-cost increase. Storage capacities are reused and
  empty scenes release storage. CPU mutation/refit tests and GPU buffer-reuse tests pass.
  Per-group trees, periodic instancing and changed-range uploads remain profiling candidates.
- Step 4: cartoon residue input caching and native parallel run generation implemented.
  Serial/parallel output and cached/original atomistic/Martini geometry comparisons pass,
  including coordinate frames, periodic boxes and color methods. Small/browser inputs stay
  serial. Five-run development medians for 50 builds give 1.08× on `2lao.pdb` and 1.04×
  on `2lao_cg.pdb`; these small structures do not exercise the parallel threshold.
- Step 5: parallel Z transforms and field smoothing implemented with a shared transpose.
  This adds one 4-byte-per-voxel scratch grid, capped at 256 MiB; thresholds keep small
  grids serial. Partial-tile output is bit-exact at two and four threads. With 16 threads,
  five-run development medians show 3.25×/3.72× EDT and 8.08×/6.74× three-pass smoothing
  at 128³/256³. These are stage timings, not total mesh-build or FPS gains. Extraction,
  refinement and cross-build scratch retention remain measurement-dependent candidates.
- Step 6: bounded native background surface/cartoon jobs implemented for static viewport
  rebuilds. Workers own numeric snapshots; GPU installation stays on the UI thread.
  At most two jobs are active. Edits/replacement drop the old result receiver and cancel
  the worker; worker failure uses a synchronous fallback. Captures, editing, trajectories,
  small inputs and browser builds remain synchronous. Queue/cancellation/failure tests and
  GPU edit/replacement/capture-barrier tests pass. Asynchronous trajectory playback remains
  a separate design: it must keep all displayed geometry, picking and highlights coherent.
- Step 7: unshifted smooth-bond marching reuses waist positions, spans and cubic coefficients.
  The original hit epsilon and 192-iteration limit are retained. The diagnostic entry point
  is used only when explicitly requested by a diagnostic shader/test; the public solver
  returns the original hit structure. Original-solver comparisons pass for 1,536 rays across
  three smoothing levels, including inside/exit and offset strands, plus existing tangent
  normal/end-cap tests. The synthetic ray set reports mean 11.1–12.2 and p95 25–29 iterations;
  this does not represent a measured application workload. GPU timing and a new hybrid solver
  remain pending; no tolerance or quality reduction was made.
- Step 8: shadow-map reuse implemented with exact light-camera bytes and ordered caster
  geometry revisions. Unknown, dirty or pending geometry always redraws. Map resize clears
  the cache. GPU tests compare reused and forced-fresh images and verify camera, geometry,
  visibility, opacity, removal, pending-edit and resize invalidation. The key follows the current central-image
  shadow caster policy; it does not add periodic-image shadows. Culling, pick acceleration
  and upload batching remain conditional on measurements.
- Step 9: deferred as specified by its acceptance gate. No automatic quality reduction was
  added. Physical GPU/browser performance and image evaluation are required before choosing
  an interactive preset. Current sampling, resolution and export quality remain unchanged.

### Final validation for this implementation

- Native library: 195 passed; scripting: 199 passed. Each configuration has 27 ignored
  GPU/manual tests, which are selected separately rather than included in these totals.
- Native workspace compile check and wasm32 core compile check pass. The browser check
  reports the three existing loader/trajectory dead-code/unreachable-code warnings.
- Selected software-Vulkan regressions pass: timestamp readback bounds; original AO/shadow
  images; near-first mixed hits; camera-only ray-trace preparation/storage release; scene
  and accumulator replacement; background edits/replacement/capture barriers; smooth-bond
  original-solver comparisons; shadow reuse/invalidation; transparent duplicate suppression;
  molecular raster/trace appearance across materials, effects and both projections.
- Native deterministic cache/refit/parallel tests pass. The job scheduling test also passes
  with `RAYON_NUM_THREADS=1`; its intentionally blocked test workers use a separate fixed pool.
- CPU cartoon and surface development benchmarks completed. Hardware GPU timing, end-to-end
  FPS, peak RSS and browser runtime tests are not established by these results.
- `git diff --check` passes.

### Remaining measurement and design gates

The exact changes above are implemented in plan order. The following work is not implemented:
physical discrete/integrated GPU and browser benchmarks; alternate AO storage/resolution/sample
counts and temporal filtering; parallel BVH subtrees/group-specific envelope acceleration;
per-group ray-trace instancing and changed-range uploads; retained cartoon/surface scratch and
parallel mesh extraction/refinement; coherent asynchronous trajectory playback; a hybrid bond
solver and high-accuracy grazing-ray reference; conservative culling/pick acceleration/upload
batching; optional interactive quality presets. These are conditional experiments, not completed
features. Use the required tests below before enabling any of them.

Profiling commands (profiling is off by default):

```bash
MOLAR_VIS_PROFILE=1 RUST_LOG=molar_vis_core=info cargo run --release -p molar_vis -- tests/2lao.pdb 2>/tmp/molar-profile.log
python3 scripts/summarize_render_profile.py /tmp/molar-profile.log --warmup 5
```

CPU scopes can be nested: do not sum stage times to estimate frame time. GPU samples represent
passes/submissions; tile compute is not a full frame. Profile each scenario separately and
repeat at least five runs. Diagnostic timings include profiling/readback overhead. Idle does
not create queries or request animation. Browsers retain the no-query/no-CPU-clock fallback.

## Current baseline

Already implemented:

- Native surface distance-transform XY planes run in parallel, with reusable scratch.
  Small grids and WebAssembly use the serial path. Surface field conversion is in place;
  color smoothing reuses fixed neighbor weights.
- Smoothed trajectory states are shared by window within a rebuild. Steady hover does not
  request continuous animation. GPU picking uses a one-pixel scissor.
- GPU buffers retain capacity when geometry shrinks. Mesh updates replace indices as well
  as vertices. Ray-trace ping-pong and unobstructed-view bind groups are reused.
- Shadow maps use depth-only sphere, cylinder and mesh pipelines.
- Coordinate-only updates for VDW, Licorice, Balls+Sticks and Lines cache selected atom IDs,
  colors, radii and compact bond connectivity. Positions, periodic dashes and smooth bond
  profiles still update each frame. Structural rebuilds replace the cached inputs.
- Ray-trace preparation borrows clean raster CPU geometry. Meshes share the existing
  pick/glow mesh cache. Dirty or unavailable geometry uses a full-build fallback.
  Camera-dependent line widths and bond offsets still require preparation; unchanged independent
  geometry now skips preparation, and stable primitive counts permit bounded BVH refits.

A local CPU benchmark on `tests/2lao.pdb`, 500 repeated builds, measured these speedups for
coordinate geometry only: VDW 7.10×, Licorice 1.53×, Balls+Sticks 1.70×, Lines 1.62×.
The sparse `name CA` selection measured 2.36–4.87×. These are single-run development-profile
results on one machine, excluding selection, upload, picking and drawing. They are not FPS
estimates. Repeat release benchmarks with real trajectories before making performance claims.

## Priority and dependencies

| Order | Work | Main affected workload | Expected impact / risk |
| --- | --- | --- | --- |
| 0 | Add repeatable CPU/GPU measurements | All scenes | Required to select and verify improvements |
| 1 | Reduce AO and soft-shadow work | High-resolution raster rendering, SSAA | Potentially large; lower sampling/resolution affects quality |
| 2 | Improve BVH traversal and construction | Ray tracing, especially dense scenes | Potentially large; preserve hit and transparency rules |
| 3 | Reuse ray-trace conversion, BVHs and uploads | Camera motion and trajectory playback | Potentially large CPU savings; substantial invalidation work |
| 4 | Cache cartoon inputs and parallelize independent runs | Large proteins and trajectories | Large if cartoon preparation dominates; deterministic merging required |
| 5 | Parallelize remaining surface stages | Large surfaces and changing coordinates | Large if these stages dominate; scratch-memory and bandwidth limits |
| 6 | Move long geometry jobs off the UI thread | Surface changes, large trajectory frames | Better responsiveness; does not itself reduce computation |
| 7 | Reduce smooth-bond intersection cost | Close views of smoothed Balls+Sticks | Potentially large in this specific mode; geometric correctness risk |
| 8 | Reuse GPU passes and reduce invisible work | Picking, shadows, periodic images | Scene-dependent; conservative culling and complete dependency keys |
| 9 | Optional interactive quality controls | Very large scenes and continuous motion | High potential; explicit, visible quality tradeoff |

Order 1 can proceed independently of 2–5 after measurement. Implement 2 before deciding how
much complexity 3 needs. Implement stable ownership and invalidation from 3/4 before 6.
Items 7–9 require evidence that their target workload is slow. Do not start all items at once.

## 0. Measurement and acceptance gates

Implementation:

1. Add opt-in timing spans around selection evaluation, trajectory smoothing, secondary
   structure, geometry construction, GPU upload, pick preparation, ray-trace collection and
   BVH construction. Separate cache hits from full builds.
2. Add optional GPU timestamp queries where supported. Resolve asynchronously; never block
   every frame to read results. Measure opaque geometry, transparent envelopes, shadow map,
   AO/composite, picking, ray-trace compute and resolve separately. Keep a no-query fallback.
3. Record adapter/backend, driver, build profile, viewport, SSAA, effects, primitive counts,
   surface quality, thread count, CPU time, GPU time, upload bytes, allocation count and peak
   memory. Distinguish first build, warm camera movement, playback and idle.
4. Use fixed camera paths and trajectory frames. Warm up, collect at least five runs, and
   report median and p95 frame/build times plus variability. Measure release builds.

Required scene matrix:

- Small molecule and `tests/2lao.pdb`; a reproducible large atom/bond scene; a large protein
  cartoon; a large surface; sparse selections; multiple molecules and periodic images.
- Orthographic and perspective; opaque and transparent; solid and smoothed multiple bonds;
  AO/shadows individually and together; supported SSAA levels; 1080p and 4K.
- Static camera, orbit/zoom, trajectory playback, changing selections and smoothing windows.
- Native discrete and integrated GPUs, plus a WebGPU-capable browser when available.
  Software Vulkan verifies correctness but cannot establish hardware GPU performance.

Acceptance:

- Exact optimizations must retain geometry/hit semantics. Use byte comparisons where the
  arithmetic order is unchanged; define a numerical tolerance before changing that order.
- Suggested performance gate: at least 10% improvement in the targeted expensive stage,
  with no reproducible regression above 5% in unaffected representative workloads. These
  are review gates, not promised speedups; account for measurement noise and end-to-end gains.
- Record memory costs. Reject unbounded caches or scratch proportional to worker count times
  an entire surface grid. Confirm that idle remains idle.
- Quality-changing alternatives require a separate preset and image evaluation. Keep the
  current output available as the reference and export-quality path.

## 1. AO and soft-shadow shader work

Locations: `render/shaders/ssao.wgsl`, `lighting.wgsl`, and pass setup in `render.rs`.
Current AO uses a 128-point kernel. Soft-shadow filtering uses 128 samples when softness is
nonzero. Both can be expensive at SSAA resolution.

Implementation, in separate changes:

1. Measure AO and shadow filtering separately. Verify that disabled effects skip their work.
2. Precompute fixed disk offsets used by AO and shadow kernels. Compare a constant shader
   table and a small uploaded buffer; extra reads can offset the savings from removing trig.
3. Test effect resolution independent of color SSAA. Use depth/normal-aware upsampling;
   decide how to select downsampled depth so thin foreground bonds are not lost.
4. Add named sample-count/resolution presets only if the previous changes are insufficient.
   Temporal accumulation is a separate, later experiment with explicit history invalidation.

Required tests:

- AO disabled, shadows disabled, hard shadows, and zero softness; correct pixel/world units
  under resize, zoom, perspective, orthographic projection and every SSAA level.
- Concave ribbons, narrow gaps, thin bonds, silhouettes, smooth surfaces, depth discontinuities
  and transparent geometry. Check halos, bands, light leaks, shadow acne and edge bleeding.
- Fixed-seed image comparisons against the existing full-quality path. For reduced quality,
  record image-error metrics and inspect close-up crops and camera-motion sequences.
- If temporal filtering is added: no ghosting after camera cuts, playback jumps, resize,
  selection edits, lighting changes or molecule removal.
- Naga shader validation and real GPU timing. Accept quality changes only as a separate mode.

## 2. BVH traversal, construction and envelope queries

Locations: `render/raytrace.rs`, `render/shaders/raytrace.wgsl`, and `render/envelope.rs`.
Current traversal pushes children in a fixed order. Transparent containment walks the global
BVH and filters by envelope group at leaves. CPU SAH construction repeatedly scans bins for
candidate split costs. Shader traversal stacks have 32 entries.

Implementation:

1. Return AABB entry distance and visit the nearer child first for closest-hit rays. Prune
   the farther child using the updated closest distance. Benchmark any-hit ordering separately.
2. Establish a construction depth bound compatible with shader stack capacity. Add a safe
   fallback for pathological trees; do not silently drop nodes when a stack fills.
3. Replace repeated SAH bin scans with prefix/suffix bounds and counts, preserving split
   and tie behavior where possible. Consider parallel subtree construction only above a
   measured threshold and merge nodes in deterministic order.
4. If transparent containment dominates, build group-specific roots/trees or conservative
   group metadata that can skip unrelated subtrees. Preserve per-image group isolation.

Required tests:

- Compare closest-hit and any-hit results against brute force over seeded random scenes and
  rays. Include spheres, capsules, smoothed bonds, triangles and mixed scenes.
- Empty/single-primitive trees, coincident centroids, highly unbalanced distributions,
  zero-length bonds, thin triangles, inside origins, axis-parallel rays and grazing hits.
- Validate parent bounds, leaf ranges, primitive permutation, maximum depth and stack use.
- Define stable behavior for equal-distance hits; preserve transparent-envelope tie rules,
  duplicate primitive suppression, source-triangle skipping and closed-surface exits.
- Run the existing envelope, material, AO and shadow appearance regressions. Measure node
  visits, primitive tests, construction time and rays/second for opaque and transparent scenes.

## 3. Reuse ray-trace conversion, BVHs and uploads

The new geometry cache removes repeat surface/cartoon construction. `RtScene::gather` still
converts all primitives, creates AABBs, builds the BVH and uploads the scene after camera changes.

Implementation:

1. Introduce explicit revisions for topology/selection, coordinates, appearance, periodic
   images and camera-dependent geometry. Include smoothing, secondary-structure policy,
   material/opacity, visibility, box changes and dashed-PBC settings in the dependency model.
2. Separate camera-independent spheres, meshes and single bonds from screen-width lines and
   camera-offset bond strands. A camera change must still update the latter and their bounds.
3. Reuse converted buffers and BVHs for unchanged groups. Consider a top-level instance tree
   over per-representation trees to avoid duplicating large meshes for periodic images.
4. For stable primitive topology, refit bounds after coordinate changes. Rebuild when primitive
   counts/order change or a measured tree-quality threshold degrades. Surface topology and
   periodic dash counts can change even if atom selection is fixed.
5. Reuse storage capacities and upload changed ranges. Reset progressive accumulation whenever
   any visible result changes, including material-only edits that do not require a BVH rebuild.

Required tests:

- Compare cached/refitted scenes with full gather/rebuild after every mutation: coordinates,
  frame and smoothing window, selection, atom/bond edits, material, charge coloring, SS policy,
  periodic box/images, visibility, undo/redo, shared-source update, removal and session reload.
- Camera orbit, zoom and viewport resize with lines and double/triple/aromatic bonds.
- Empty-to-nonempty and shrinking/growing buffers; unchanged counts with changed connectivity.
- Ray results against fresh BVHs throughout long trajectories. Track refit quality and rebuilds.
- Assert that a camera-only move with exclusively camera-independent geometry does not rebuild
  or upload that geometry. Check memory is released on removal and no stale image accumulates.

## 4. Cartoon input caching and CPU parallelism

Location: `geometry/cartoon.rs`. The builder currently groups atoms into a `BTreeMap` and
rediscovers trace/orientation atoms on each build.

Implementation:

1. Cache ordered residue records: trace and orientation atom indices, chain/residue IDs,
   source-atom ownership, colors and reusable SS classification. Support CA/O and Martini BB/SC1.
2. Fetch current coordinates from the displayed/smoothed state. Recompute PBC breaks and
   coordinate-dependent frames; do not cache run boundaries that depend on positions.
3. Generate independent chains/runs in native worker tasks using owned numeric inputs.
   Merge vertices, indices and ownership tags in original run order. Keep a serial path for
   small work and WebAssembly. Do not send borrowed molecular provider objects across threads.
4. Reuse scratch buffers and fixed connectivity only where run length and shape rules prove
   it is stable. Preserve full rebuilds when SS or selection changes.

Required tests:

- Compare cached and uncached output across trajectories, chain gaps, missing orientation
  atoms, isolated residues, helix/sheet transitions, PBC wrapping and placeholder boxes.
- Cover atomistic and Martini inputs, smoothing, both SS algorithms and per-frame SS on/off.
- Compare serial and parallel vertices/normals/indices and exact residue/atom ownership tags.
- Verify picking, lasso and selection glow still follow the displayed ribbon.
- Benchmark one long chain, many short chains and small inputs. Check thread scheduling does
  not slow small molecules or allocate a full molecule copy per worker.

## 5. Remaining surface parallelism and memory traffic

Location: `geometry/surface.rs`. XY distance-transform planes are already parallel. Measure
Z transforms, field smoothing, surface extraction, projection/refinement and normal/color work.

Implementation:

1. For Z transforms, compare blocked gather/scatter scratch with a transposed grid. Choose
   only after measuring cache behavior and peak memory; avoid unsafe aliasing of strided writes.
2. Parallelize independent stencil outputs using separate input/output buffers. Keep boundary
   rules and arithmetic order stable. Reuse scratch between passes.
3. If extraction dominates, use slab-local output plus deterministic prefix offsets and an
   explicit shared-boundary ownership rule. Reconcile indices before emitting final geometry.
4. Reuse scratch capacity across rebuilds with a bounded retention policy. Spatial nearest-atom
   acceleration is a separate candidate if source/color assignment is a measured bottleneck.

Required tests:

- EDT versus brute-force nearest seeds on rectangular/degenerate dimensions, no/all seeds
  where supported, and grids around serial/parallel thresholds.
- Serial/parallel field equality and finite values for every quality level and smoothing mode.
- Mesh winding, shared-edge connectivity, normals, source atoms, level-set error and absence
  of slab-boundary cracks. Test isolated atoms, overlaps, cavities and zero probe radius.
- Repeat builds across thread counts for deterministic results. Measure 64³, 128³ and 256³
  grids plus larger grids within memory limits; report peak memory and total build time.

## 6. Background geometry jobs and bounded playback work

Locations: `app/build.rs`, `scene.rs`, `trajectory.rs`, and a new owned-job worker module.
This follows the cache/revision work; it is a responsiveness change, not an automatic FPS gain.

Implementation:

1. Snapshot only required owned numeric data on the UI thread. Keep GPU uploads and scene
   mutation on their current owner thread; do not assume molar providers are Send/Sync.
2. Tag jobs with molecule identity and relevant revisions. Install a result only if its
   complete input revision still matches. Coalesce queued frames and cancel obsolete jobs.
3. Bound queued work and scratch memory. During playback, define whether to retain the last
   complete frame or drop intermediate frames. Keep picking and highlights on the displayed
   geometry, not a newer frame whose geometry has not arrived.
4. Use the existing synchronous path for small workloads and browser builds until a browser
   worker design is separately validated. Do not introduce nested unbounded thread pools.

Required tests:

- Force out-of-order completion after seek, selection/style edit, molecule deletion/reload,
  undo/redo, visibility change and source replacement. No stale result may be installed.
- Verify cancellation, shutdown, worker failure, bounded queues and sustained playback memory.
- Check UI response latency, time to first complete frame, frame dropping and total throughput.
- Ensure pick IDs, glow and ray-trace accumulation match the frame actually displayed.

## 7. Smooth bond intersections

Location: `render/shaders/bond_profile.wgsl`, shared by raster and ray-trace paths.
Optional flared bonds use up to 192 conservative sphere-tracing iterations per intersection.

Implementation:

1. Add opt-in iteration-count diagnostics. Measure primary, shadow and containment work.
2. Tighten conservative bounds and reuse per-bond profile constants. Add analytic handling
   for provably simple segments before changing the general solver.
3. Explore bracketed refinement or a hybrid analytic/marching solver. Retain a conservative
   fallback. Do not simply lower the iteration cap or increase the hit epsilon.

Required tests:

- Compare hit distance and normal to a high-accuracy reference over seeded rays, including
  tangent, grazing, inside/exit, end-cap and near-degenerate cases.
- Cover smoothing extremes, unequal radii, short/long bonds, offset multi-order strands,
  large coordinate scales and perspective/orthographic views.
- Compare raster depth, shadow depth, transparent envelopes and traced silhouettes. No holes,
  false hits or changed endpoint colors. Report average/p95 iterations and GPU time.

## 8. Pass reuse, conservative culling and picking

Locations: `render.rs`, `app/build.rs`, `app/viewport.rs`, `pick.rs`, `render/unobstructed.rs`.

Implementation:

1. Measure shadow-map reuse. Key it on geometry, light-space transform, relevant material and
   map resolution. A camera move may change the eye-relative light; do not assume it is reusable.
2. Cache per-representation bounds and conservatively cull invisible periodic images/groups.
   Include sphere radii, bond smoothing/strand extent and screen-width lines. Camera-frustum
   culling must not remove offscreen shadow casters or ray-traced secondary-ray occluders.
3. Measure CPU pick preparation after the existing one-pixel GPU scissor optimization.
   Reuse source IDs/topology and refit spatial data only when its dependencies permit it.
4. Batch small compatible uploads only if measurements show API overhead dominates.

Required tests:

- Shadow invalidation for camera/light changes, motion, visibility, opacity and box/image edits.
- Frustum-edge, near-plane and large-radius cases; compare culling on/off images and picks.
- Offscreen objects casting visible shadows or AO; both projections and multiple images.
- Pick results versus uncached preparation during playback, removal and geometry shrink/growth.
- Measure draw calls, upload bytes, CPU preparation and GPU time separately.

## 9. Optional interactive quality modes

Only pursue after exact optimizations and profiling. Candidates are lower AO/shadow resolution
while moving, lower surface/cartoon detail, temporary SSAA reduction and bounded progressive
ray-trace work per frame. Expose these as explicit interactive settings with full quality on
settle/export. Preserve the existing full-quality behavior as a selectable mode.

Tests must cover settle detection, rapid stop/start, resize, captures made during motion,
progressive reset, stable selection/picking and transitions without flicker. Measure response
latency and final convergence time as well as frame rate. Do not use a preview frame silently
for final image export.

## Validation commands and delivery process

Use one reviewable change per numbered work item, splitting larger items by their numbered
steps. Record the baseline, chosen design, correctness result, performance result and memory
cost in each change. Update `ARCHITECTURE.md` when cache ownership or pass dependencies change.

```bash
CARGO_TARGET_DIR=/tmp/molar-vis-audit-target cargo test -p molar_vis_core --offline --lib
CARGO_TARGET_DIR=/tmp/molar-vis-audit-target cargo test -p molar_vis_core --offline --lib --features scripting
CARGO_TARGET_DIR=/tmp/molar-vis-wasm-check cargo check -p molar_vis_core --offline --target wasm32-unknown-unknown
CARGO_TARGET_DIR=/tmp/molar-vis-audit-target cargo test -p molar_vis_core --offline benchmark_coordinate_cache -- --ignored --nocapture
```

The current ignored GPU regressions can be selected individually with this pattern:

```bash
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json XDG_RUNTIME_DIR=/tmp CARGO_TARGET_DIR=/tmp/molar-vis-audit-target cargo test -p molar_vis_core --offline depth_only_shadows_match_opaque_pipeline_depth -- --ignored --nocapture
```

Relevant existing tests include `molecular_appearance_matches_for_materials_effects_and_projections`,
`materials_match_in_linear_and_srgb_targets`, `transparent_envelope_renders_duplicate_primitives_once`,
`traced_ao_detects_blockers_absent_from_the_depth_buffer`,
`traced_shadows_keep_lit_surfaces_clean_and_soften_the_terminator`,
`capsule_shadow_rays_find_the_valid_exit`, `smooth_surface_secondary_rays_do_not_create_triangle_patches`,
`mesh_update_replaces_indices_when_counts_match`, `pick_buffers_reuse_capacity_and_drop_removed_atoms`,
and `raytrace_bindings_follow_scene_and_accumulator_replacement`.

Run the regressions affected by each change, then the required native and browser checks.
For CPU geometry/cache changes also retain `cached_frames_match_full_build_with_sparse_selection_and_periodic_bonds`
and `cached_geometry_matches_fresh_trace_and_rejects_dirty_reps`. Add the missing targeted tests
listed under each work item before replacing its reference path. Use hardware adapters, without
forcing lavapipe, for performance measurements.
