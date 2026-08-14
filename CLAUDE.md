# CLAUDE.md — molar_vis

A modern, legacy-free molecular viewer modeled after VMD, in **pure Rust**,
targeting Linux/Windows/macOS/WebAssembly. It builds on **molar** (the user's Rust
molecular library: IO, selections, topology, DSSP) and renders on **eframe/egui +
wgpu** with hand-written WGSL GPU ray-cast impostors.

The user, Semen Yesylevsky, is the author of molar. The full approved plan lives at
`~/.claude/plans/we-are-going-to-rippling-wreath.md`; per-session memory at
`~/.claude/projects/-home-semen-work-Projects-molar-vis/memory/`.

## Language and writing style

Only report to me in ASD-STE100 Simplified Technical English.

## Documentation map

This file is the lean index. The detailed reference is split by domain under `docs/`, so the
always-loaded context stays small. **Read the doc for the area your task touches** before you
work in it:

- **[docs/BUILD.md](docs/BUILD.md)** — build/run/test commands, the `scripting` feature flag, the
  native Python (`molar_vis_py`) and browser-JS (`molar_vis_js`) builds, test assets, the full
  `MOLAR_VIS_DEBUG_*` headless-verification hooks, pinned crate versions, and the
  installable / local-checkout dev workflow.
- **[docs/MODULES.md](docs/MODULES.md)** — the workspace crates and every core module, file by file
  (`launch.rs`, `app.rs` + `app/`, `scene.rs`, `geometry/`, `render/` + `raytrace`, `history.rs`,
  `session.rs`, `settings.rs`, `script/`, `pick.rs`, `interactions.rs`, `docking.rs`, `analysis.rs`,
  `charges.rs`, …).
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** — the rendering pipeline and cross-cutting
  design: Strategy-A offscreen render, GPU impostors, SSAA, depth cueing, SSAO, cast shadows,
  background, the scene graph, and the dirty-flag render-skip that keeps idle at 0 GPU.
- **[docs/MOLAR.md](docs/MOLAR.md)** — the molar 2.1 API (the SoA migration) and molar-integration
  details: selections, disjoint binds, trajectories, bond resolution, and the per-frame rebuild
  paths.
- **[docs/UI.md](docs/UI.md)** — the egui UI layout: left panel, menu bar, top view toolbar,
  per-rep two-row blocks, and the dialogs.
- **[docs/CONVENTIONS.md](docs/CONVENTIONS.md)** — coding conventions, egui 0.34 / wgpu 29 API
  gotchas, and the theming rules (never `override_text_color`, read semantic colours from the
  style, the Wayland IME workaround, …).
- **[docs/MILESTONES.md](docs/MILESTONES.md)** — the full milestone history (M0–M33): the
  authoritative record of what shipped, and the design reasoning behind each piece.

## Build / run / test

Quick start; the full detail (Python & JS builds, test assets, headless hooks, tech-stack
versions) is in **[docs/BUILD.md](docs/BUILD.md)**.

```sh
cargo build
cargo run -p molar_vis -- tests/2lao.pdb            # one molecule
cargo run -p molar_vis -- a.pdb a.xtc               # VMD-style: a.pdb + a.xtc traj = ONE molecule
cargo run -p molar_vis -- -m a.pdb a.xtc -m b.pdb   # `-m` starts a new molecule → two molecules
cargo test -p molar_vis_core
cargo build -p molar_vis_core --target wasm32-unknown-unknown   # WASM-readiness check (now green)
cargo build -p molar_vis_py                                     # native Python module (compile check)
wasm-pack build crates/molar_vis_js --target web --out-dir web/pkg   # browser JS API (M27)

cargo run -p molar_vis --features scripting          # + the in-app Rhai console (M31)
cargo test -p molar_vis_core --features scripting    # 96 tests (92 without)
```

**Feature flags.** The one feature is **`scripting`** (M31, **off by default**): the in-app Rhai
console. Every crate has a pass-through of it (`molar_vis`, `molar_vis_web`, `molar_vis_py`,
`molar_vis_js` → `molar_vis_core/scripting`). Verify **both** configurations after touching
anything near `script*` or the `App` console fields — with it off, `rhai` is out of the
dependency graph entirely (`cargo tree -p molar_vis | grep rhai` → nothing).

## Roadmap

Forward-looking feature list (deleting traj frames, save/load state, app settings, more depth-cue
methods, background color, material editor, labels/measurement, Python bindings, embedded command
language, geometric primitives, raytracing, movies, whole-residue pick mode, CG/Martini bonds+SS,
plugins, selection-input improvements, drug-discovery goodies (PLIP interactions, SDF reading),
dashed PBC half-bonds, and visual structure editing) lives in **[ROADMAP.md](ROADMAP.md)** — in no
particular order. Move items into **[docs/MILESTONES.md](docs/MILESTONES.md)** (*Milestone status*)
as they ship.
