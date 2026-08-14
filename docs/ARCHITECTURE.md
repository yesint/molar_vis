# molar_vis — Key architecture

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

## Key architecture

- **Strategy A rendering** — the 3D scene is drawn into our *own* offscreen color +
  depth textures, then composited into egui as an `Image`. egui's render pass has no
  depth attachment; this gives full depth control for impostors.
- **Anti-aliasing = SSAA** (`SSAA` in `render.rs`, default 2×) — the offscreen targets are
  allocated at `SSAA×` the viewport (clamped to `max_texture_dimension_2d`); egui's existing
  `FilterMode::Linear` downsamples into the 1× image rect (a 2×2 box average). This smooths
  **everything**, crucially the **impostor silhouettes** (decided per-pixel by `discard`, so MSAA
  can't touch them) as well as the cartoon mesh and lines — no MSAA targets / depth-resolve / OIT
  rework. The camera viewport param (`params.yz`) stays at the **logical** size so fat-line pixel
  widths come out correct after the downsample (a 2× target with logical viewport → line is `w`
  final px). Cost: `SSAA²`× fragments per re-render; idle still 0 GPU (render-skip unchanged).
- **Impostors** — spheres & cylinders are GPU ray-cast in fragment shaders that write
  analytic `frag_depth`, so they occlude correctly against each other (and, later, the
  cartoon mesh). The camera uniform carries a perspective flag: perspective uses an
  eye-ray from the origin; **orthographic uses a parallel ray with origin on the camera
  plane (z=0)** so the near hit has t>0 (a past bug black-screened ortho). Lines are
  **screen-space fat-line quads** — WebGPU only rasterizes 1px `LineList`, so each segment
  (a pair of `LineVertex`, which now carries a per-vertex `width` px) is reinterpreted as
  **instanced** data (stride = 2 verts) and drawn as a `TriangleStrip` quad expanded
  perpendicular to the segment by `width` px in `line.wgsl` (uses the viewport size carried
  in the camera uniform's `params.yz`); width stays constant in pixels at any zoom, like VMD.
  **Cylinders are two-tone capsules** (`cylinder.wgsl` `compute_hit`): **one instance per bond**
  (not per half-bond) ray-casts a finite wall over `h∈[0,seg_len]` **plus a hemispherical cap at
  each end** (the two atoms), and colors the `p0` half `color` / the `p1` half `color1` at the
  midpoint (VMD half-bond coloring, `CylinderInstance` carries both). So a bond is one **continuous
  smooth capped tube** — no separate atom sphere abutting a capless wall (whose hard occlusion seam
  showed as a dark "crescent"/gap at end-on views). Licorice therefore draws an atom **ball only for
  bondless atoms** (`geometry::spheres_where`/`bonded_mask`); bonded atoms are rounded by the capsule
  caps. The **billboard is built in the screen plane** (axis projected to screen `u` + screen-perp
  `w`, extents `½·projected-len+r` × `r`, ×1.4 oversize) so it robustly covers the whole capsule
  **including the caps at any angle** — crucially end-on, where the old `cross(axis,view)` perp
  degenerated to a thin strip (the atom spheres used to cover the cap there). A subtle **shadowed-side
  fill light** (`FILL_STRENGTH`, gated by `1−N·L`) in `shade_material` (sphere+cylinder) keeps joint
  creases/undersides from reading as black gaps without touching the lit side/highlight.
  **Early-Z (conservative depth) for the impostor opaque pass** — writing analytic
  `frag_depth` normally disables early depth-test, so on a screen-filling close-up every
  overlapping sphere/cylinder is shaded (deep overdraw, the reported close-up slowdown).
  When the device advertises the **`SHADER_EARLY_DEPTH_TEST`** feature (native, Vulkan/GLES
  3.1+; requested in `launch::early_z_wgpu_options`, the shared eframe device descriptor used
  by both the native bin and `molar_vis_py`, **only when the adapter supports it**), the
  renderer injects `@early_depth_test(greater_equal)` onto the opaque `fs_main` of
  `sphere.wgsl`/`cylinder.wgsl` (`render::inject_early_z`; OIT/glow/pick entries untouched),
  letting the GPU reject occluded impostor fragments **before** the ray-cast + shading. The
  attribute requires the rasterized depth to be a *lower bound* on the true hit depth, so each
  shader overrides only `clip.z` to a **per-instance constant** conservative depth (a constant
  can't overshoot the true hyperbolic surface depth the way an *interpolated* per-vertex value does
  across a foreshortened billboard — that overshoot wrongly-culled fragments as black wedges at
  grazing close-ups). NDC depth is a function of **eye-z only** (both projections), so the minimum
  NDC depth is at the maximum eye-z: **sphere** = `center.z + radius`; **cylinder** =
  `max(a0.z, a1.z) + radius`. Two guards make it safe: **(1)** if the near pole crosses the camera
  plane (`near_c.w ≤ 0` on extreme close-ups) fall back to depth 0; **(2)** `clamp(z_ndc, 0, 1)` so
  the overridden `clip.z ∈ [0, w]` never triggers extra **near-plane clipping** of the billboard
  (an unclamped `z<0` near-clipped the quad → holes/gaps — this was a real regression). The fragment
  still writes the true analytic depth, so the depth buffer is exact and early-Z ON==OFF (verified
  numerically across fit / perspective + ortho grazing close-ups). WebGL2/wasm and
  adapters without the feature never get the attribute (the injection is a no-op) and fall back
  to plain late-Z, unchanged. **Surfaces/cartoon** are plain meshes (no `frag_depth`) so the GPU
  already early-Zs them — no change needed. Set `MOLAR_VIS_NO_EARLY_Z=1` to force the feature
  off (the A/B verification + escape hatch).
- **Depth cueing (fog)** — fog fades all geometry toward the background (`BG` in
  `render.rs`, also the clear color) by eye-space distance, with three VMD-style falloff
  **`CueMode`s** (matching the OpenGL fog equations): **Linear**, **Exp** (`1−e^(−k·t)`), **Exp²**
  (`1−e^(−(k·t)²)`), all normalized to reach full fog at the far plane so switching modes keeps the
  far-fog at `strength` and only changes the ramp shape. The camera uniform carries
  `cue = [near, far, strength, mode]` (eye-space, derived per frame by `Camera::cue_uniform`
  from `distance`/`scene_radius` + the scene-relative `DepthCue { enabled, start, strength, mode }`
  on `Camera`) + `fog_color`. Every fragment shader applies the shared `apply_fog(color, eye_z)`
  (computes normalized depth `t∈[0,1]`, selects the curve by `cue.w`); line/mesh pass eye-space `z`
  as a varying, the impostors use their ray hit. Lives in `Camera` so its `PartialEq` re-renders on
  change; the top-view-toolbar depth-cue popup has the **mode tabs** (`tab_bar`) + Strength/Start
  sliders and stays open until you click outside/the button (`CloseOnClickOutside`).
  `MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2` sets it headlessly.
- **Ambient occlusion (SSAO)** — a fullscreen pass (`render/ssao.rs` + `shaders/ssao.wgsl`) inserted
  after the opaque pass: it reads the scene **depth** (the depth target now carries `TEXTURE_BINDING`
  so it's sampleable; impostors' analytic `frag_depth` makes it exact), reconstructs view-space
  positions via the inverse projection, and estimates occlusion **without normals** — for each pixel
  it counts neighbours (a fixed golden-angle spiral kernel, world-radius scaled to screen by the
  projection) that sit *in front* of it in view space, so creases/contacts darken but flat surfaces
  don't self-shadow. The AO factor is written back with a **multiply blend** (`result = dst×ao`)
  onto the opaque color before the OIT composite — no extra targets, no separate blur (the 2× SSAA
  downsample smooths the mild banding from the unrotated kernel). Settings live in `Camera::ao`
  (`Ao { enabled, strength, radius }`, off by default) → re-renders via `PartialEq`, serialized in
  sessions; `Camera::ao_uniform` feeds the pass `[radius, bias, strength, enabled]`. **Gated to full
  WebGPU** (`ssao_pipeline: Option`, built only when `oit_enabled`): WebGL2 can't reliably sample
  the depth texture, so it skips SSAO rather than risk a startup shader-compile failure. Works on
  both impostors (VDW) and meshes (surface/cartoon). `MOLAR_VIS_DEBUG_AO[=strength]` enables it.
- **Cast shadows (real-time shadow mapping, deferred)** — VMD's ray-traced shadows, but real-time.
  An extra **shadow pass** (pass 0, before opaque, only when `Camera::shadow.enabled`) renders the
  opaque geometry from a **key light** into a fixed `2048²` `Depth32Float` shadow map
  (`shadow_depth_view`); a throwaway color target (`shadow_color_view`) lets us **reuse the existing
  opaque pipelines** for the depth fill (impostors compute correct light-space analytic `frag_depth`
  because the light camera is just another `CameraUniform` entry — ortho, `perspective=false` — so
  **no depth-only pipeline variants are needed**; `draw_shadow_casters` draws spheres/cylinders/mesh
  only — lines/box don't cast). The light is directional (`SHADOW_LIGHT_DIR_VIEW`, a view-space
  upper-right key off the view axis so shadows fall on camera-visible surfaces — a near-camera
  headlight would hide them); its **orthographic frustum is fit to the scene's bounding sphere**,
  recovered from `view` + `depth_range`. The shadow is then applied **deferred in the AO pass**: the
  SSAO shader already reconstructs each pixel's view-space position, so it also projects it to the
  light's clip space (`shadow_matrix = light_proj·light_view·inv_view`, carried in `SsaoUniform`),
  does a 3×3 PCF `textureSampleCompareLevel` against the shadow map, and folds the result into the
  same multiply-blend (`output = ao × shadow_factor`). So **no lit-shader changes and no new
  pipelines** — one extra geometry pass + a shadow sample in the existing fullscreen pass. The AO
  pass now runs when *either* AO or shadows are on (AO strength 0 when AO is off). `Camera::shadow`
  (`Shadow { enabled, strength }`, off by default, serialized) → `shadow_uniform` = `[strength,
  bias, enabled, _]`. **Gated to full WebGPU** like SSAO (shares `ssao_pipeline`; WebGL2 skips it).
  Periodic images aren't baked into the shadow map (rare combo), so they may be mis-shadowed.
  `MOLAR_VIS_DEBUG_SHADOW[=strength]` enables it. Verified on VDW (impostors) + surface (mesh),
  alone and combined with AO.
- **Background** — `Camera::background` (`Background { kind: Solid|Gradient, color, top, bottom }`,
  serialized, drives re-render via `PartialEq`). The opaque pass clears to `background.clear_color()`;
  for a gradient, a fullscreen pass (`render/background.rs` + `shaders/background.wgsl`) is drawn
  **first inside the opaque pass** (color only, `depth_compare = Always`, no depth-write, so it sits
  behind the geometry without perturbing the depth the SSAO/shadow passes read). Depth-cue fog fades
  geometry toward `background.fog_color()` (the solid color, or the gradient midpoint) — passed to
  `CameraUniform` in place of the old `BG` const. `MOLAR_VIS_DEBUG_BG=gradient`.
- **Scene graph** — N molecules × M reps. Each rep has a molar **selection string**
  compiled to atom indices (`compile_selection` → `system.select`). Geometry is built
  only for selected atoms (and bonds whose endpoints are both selected).
- **Dirty flags & render-skip** — `rep.sel_dirty` (recompile selection), `rep.geom_dirty`
  (rebuild + reupload geometry). `app.rebuild_dirty()` processes them each frame.
  `render_scene` runs **only** when geometry changed, the camera moved (`Camera`
  `PartialEq` vs `last_render_camera`), the viewport resized, or `view_dirty`
  (visibility/structure). No continuous repaint → **idle = 0 GPU**; egui repaints on input.

