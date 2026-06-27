//! In-app **Rhai** scripting console (scripting roadmap, first slice).
//!
//! A fluent, object-oriented surface: `mol(i)` returns a [`MolHandle`], whose methods
//! (`add_rep`, `rep`, `show`/`hide`, `frame`, `play`/`pause`, `focus`) act on that
//! molecule; `mol(i).rep(j)` returns a [`RepHandle`] whose setters (`set_style`,
//! `set_color`, `set_material`, `select`) act on that representation and **return the
//! handle** so calls chain (`mol(0).rep(0).set_style("vdw").set_color("chain")`).
//!
//! The handles are lightweight — just an index (+ a [`RepRef`]) and a clone of a
//! shared command queue. Their methods **push** [`Command`]s (no scene access during
//! eval), sidestepping the borrow problem of handing Rhai closures `&mut App`. After
//! `run` returns, `App::run_script` drains the queue through `App::execute_command`
//! (→ [`apply_scene_command`], the same field-set + dirty-flag the GUI does) on the
//! UI thread, then records one undo checkpoint. Pure-Rust + WASM-safe; only the
//! `load()` command is native-gated (in `execute_command`).

pub mod command;
pub mod console;

use std::cell::RefCell;
use std::rc::Rc;

pub use command::{parse_color, parse_material, Command, RepRef};
pub use console::{ConsoleLine, LineKind, ScriptConsole};

type Queue = Rc<RefCell<Vec<Command>>>;

/// Script handle to a molecule (`mol(i)`); its methods push molecule-level commands.
#[derive(Clone)]
pub struct MolHandle {
    mol: usize,
    queue: Queue,
}

/// Script handle to a representation (`mol(i).rep(j)` or the result of `add_rep`);
/// its setters push rep-level commands and return the handle for chaining.
#[derive(Clone)]
pub struct RepHandle {
    mol: usize,
    rep: RepRef,
    queue: Queue,
}

impl MolHandle {
    fn push(&self, cmd: Command) {
        self.queue.borrow_mut().push(cmd);
    }
}
impl RepHandle {
    fn push(&self, cmd: Command) {
        self.queue.borrow_mut().push(cmd);
    }
}

// Friendly stringification so a bare handle expression echoes meaningfully in the
// REPL (`mol(0)` → "mol(0)", `mol(0).rep(1)` → "mol(0).rep(1)") and `print(m)`/string
// interpolation work. Registered as Rhai `to_string`/`to_debug` in `ScriptSession::new`.
impl std::fmt::Display for MolHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mol({})", self.mol)
    }
}
impl std::fmt::Display for RepHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rep {
            RepRef::Index(i) => write!(f, "mol({}).rep({})", self.mol, i),
            RepRef::Last => write!(f, "mol({}).rep(last)", self.mol),
        }
    }
}

/// Apply a **scene-mutating** command (everything except [`Command::Load`], which
/// needs the App's loader and is handled in `App::execute_command`). Performs the
/// same field-set + dirty-flag the GUI does, so the change converges on the normal
/// `rebuild_dirty` path. Returns a message to show in the console on a bad index /
/// value / selection. Kept free of `App` so it's unit-testable without a GPU.
pub fn apply_scene_command(
    scene: &mut crate::scene::Scene,
    camera: &mut crate::camera::Camera,
    rep_defaults: &crate::settings::RepDefaults,
    cmd: Command,
) -> Result<(), String> {
    use crate::scene::EvalError;
    match cmd {
        Command::Load(_) => Err("load() must be applied by the app".to_string()),
        Command::Select { mol, rep, text } => {
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            // Pre-validate for an immediate error; an empty match is a (non-fatal)
            // warning the field flags, so let it through.
            if let Err(EvalError::Invalid { message, .. }) =
                scene.molecules[mi].data.evaluate(&text)
            {
                return Err(message);
            }
            let r = &mut scene.molecules[mi].reps[ri];
            r.sel_text = text;
            r.sel_dirty = true;
            Ok(())
        }
        Command::Color { mol, rep, method } => {
            let cm = parse_color(&method).ok_or_else(|| format!("unknown color scheme '{method}'"))?;
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            let r = &mut scene.molecules[mi].reps[ri];
            r.color = cm;
            r.geom_dirty = true;
            Ok(())
        }
        Command::Style { mol, rep, kind } => {
            let k = crate::geometry::RepKind::from_name(&kind)
                .ok_or_else(|| format!("unknown style '{kind}'"))?;
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            let r = &mut scene.molecules[mi].reps[ri];
            r.kind = k;
            r.params = crate::geometry::RepParams::for_kind(k);
            r.geom_dirty = true;
            Ok(())
        }
        Command::Material { mol, rep, name } => {
            let mat = parse_material(&name).ok_or_else(|| format!("unknown material '{name}'"))?;
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            let r = &mut scene.molecules[mi].reps[ri];
            r.material = mat;
            r.geom_dirty = true;
            Ok(())
        }
        Command::AddRep { mol } => {
            let mi = resolve_mol(scene, mol)?;
            scene.molecules[mi]
                .reps
                .push(crate::scene::Representation::from_defaults(rep_defaults));
            Ok(())
        }
        Command::DeleteRep { mol, rep } => {
            let (mi, ri) = resolve_rep(scene, mol, RepRef::Index(rep))?;
            scene.molecules[mi].reps.remove(ri);
            Ok(())
        }
        Command::ShowMol { mol, visible } => {
            let mi = resolve_mol(scene, mol)?;
            scene.molecules[mi].visible = visible;
            Ok(())
        }
        Command::Frame { mol, index } => {
            let mi = resolve_mol(scene, mol)?;
            let m = &mut scene.molecules[mi];
            let n = m.trajectory.n_frames();
            if n == 0 {
                return Err("molecule has no trajectory".to_string());
            }
            m.trajectory.set_current(index.min(n - 1));
            m.apply_current_frame();
            Ok(())
        }
        Command::Play { mol, on } => {
            let mi = resolve_mol(scene, mol)?;
            scene.molecules[mi].trajectory.set_playing(on);
            Ok(())
        }
        Command::Focus { mol, text } => {
            let mi = resolve_mol(scene, mol)?;
            let m = &scene.molecules[mi];
            let (_, sel) = m.data.evaluate(&text).map_err(|e| match e {
                EvalError::Empty => "selection matched no atoms".to_string(),
                EvalError::Invalid { message, .. } => message,
            })?;
            let (min, max) = m.sel_bbox(&sel);
            camera.focus_bbox(min, max);
            Ok(())
        }
    }
}

/// Validate a molecule index.
fn resolve_mol(scene: &crate::scene::Scene, mol: usize) -> Result<usize, String> {
    if scene.molecules.is_empty() {
        return Err("no molecules loaded".to_string());
    }
    if mol >= scene.molecules.len() {
        return Err(format!("no molecule {mol} (have {})", scene.molecules.len()));
    }
    Ok(mol)
}

/// Resolve `(molecule, rep)` indices, mapping `RepRef::Last` → the molecule's last rep.
fn resolve_rep(
    scene: &crate::scene::Scene,
    mol: usize,
    rep: RepRef,
) -> Result<(usize, usize), String> {
    let mi = resolve_mol(scene, mol)?;
    let n = scene.molecules[mi].reps.len();
    let ri = match rep {
        RepRef::Index(i) => i,
        RepRef::Last => n
            .checked_sub(1)
            .ok_or_else(|| format!("molecule {mi} has no representations"))?,
    };
    if ri >= n {
        return Err(format!("molecule {mi} has no representation {ri} (have {n})"));
    }
    Ok((mi, ri))
}

/// Result of evaluating one script: the mutations to apply, and the text/errors to
/// show in the console.
pub struct EvalOutcome {
    pub commands: Vec<Command>,
    pub output: Vec<ConsoleLine>,
}

/// A persistent Rhai scripting session — the console REPL's backing state.
///
/// It owns the engine **and a `Scope`**, so variables declared on one input line
/// survive into the next: `let m = mol(0)` then, on a later line, `m.rep(0)…`. The
/// fluent handles capture a clone of a **persistent** command queue (the `Rc` is
/// owned here and only ever *drained*, never replaced), so a handle stored in a
/// variable still pushes into the same queue we drain on a subsequent line. Each
/// [`eval`](Self::eval) runs one input and returns the commands to apply + the
/// console output it produced.
///
/// Pure-Rust + WASM-safe (single-threaded; the captured `Rc`/`RefCell` and the
/// non-`sync` Rhai engine make this `!Send`, which is fine — the app is UI-thread).
pub struct ScriptSession {
    engine: rhai::Engine,
    scope: rhai::Scope<'static>,
    /// Commands the handle methods push during an eval; drained after each run.
    queue: Queue,
    /// `print`/`debug`/`list` output + the result echo; drained after each run.
    out: Rc<RefCell<Vec<ConsoleLine>>>,
    /// The scene listing `list()` prints, refreshed before each `eval` so it
    /// reflects the current (pre-line) scene.
    summary: Rc<RefCell<String>>,
}

impl Default for ScriptSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptSession {
    /// Build the engine once, registering the fluent API. The registered closures
    /// capture the **persistent** queue / output / summary cells, so the same
    /// engine + scope can be reused across REPL lines.
    pub fn new() -> Self {
        let queue: Queue = Rc::new(RefCell::new(Vec::new()));
        let out: Rc<RefCell<Vec<ConsoleLine>>> = Rc::new(RefCell::new(Vec::new()));
        let summary: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

        let mut engine = rhai::Engine::new();
        // Bound runaway scripts (counts operations / call depth / expression nesting).
        engine.set_max_operations(2_000_000);
        engine.set_max_call_levels(64);
        engine.set_max_expr_depths(128, 64);

        engine.register_type_with_name::<MolHandle>("Molecule");
        engine.register_type_with_name::<RepHandle>("Representation");
        // So `print(m)`, string interpolation, and the result echo render handles nicely.
        engine.register_fn("to_string", |m: &mut MolHandle| m.to_string());
        engine.register_fn("to_string", |r: &mut RepHandle| r.to_string());
        engine.register_fn("to_debug", |m: &mut MolHandle| m.to_string());
        engine.register_fn("to_debug", |r: &mut RepHandle| r.to_string());

        // print / debug → output buffer.
        {
            let o = out.clone();
            engine.on_print(move |s| o.borrow_mut().push(ConsoleLine { kind: LineKind::Output, text: s.to_string() }));
        }
        {
            let o = out.clone();
            engine.on_debug(move |s, _src, pos| {
                o.borrow_mut().push(ConsoleLine { kind: LineKind::Output, text: format!("{s}  ({pos:?})") })
            });
        }
        // list() — print the current scene summary (refreshed by `eval` each run).
        {
            let o = out.clone();
            let sum = summary.clone();
            engine.register_fn("list", move || {
                for line in sum.borrow().lines() {
                    o.borrow_mut().push(ConsoleLine { kind: LineKind::Output, text: line.to_string() });
                }
            });
        }
        // mol(i) — entry point to the fluent API. Captures the command queue.
        {
            let q = queue.clone();
            engine.register_fn("mol", move |i: i64| MolHandle { mol: i.max(0) as usize, queue: q.clone() });
        }
        // load(path) — top-level action (native; wasm errors at apply).
        {
            let q = queue.clone();
            engine.register_fn("load", move |p: &str| q.borrow_mut().push(Command::Load(std::path::PathBuf::from(p))));
        }

        let idx = |n: i64| n.max(0) as usize;

        // --- MolHandle methods (return the handle for chaining where it reads well). ---
        engine.register_fn("rep", move |m: &mut MolHandle, i: i64| RepHandle {
            mol: m.mol,
            rep: RepRef::Index(idx(i)),
            queue: m.queue.clone(),
        });
        engine.register_fn("add_rep", |m: &mut MolHandle| -> RepHandle {
            m.push(Command::AddRep { mol: m.mol });
            RepHandle { mol: m.mol, rep: RepRef::Last, queue: m.queue.clone() }
        });
        engine.register_fn("add_rep", |m: &mut MolHandle, style: &str| -> RepHandle {
            m.push(Command::AddRep { mol: m.mol });
            m.push(Command::Style { mol: m.mol, rep: RepRef::Last, kind: style.to_string() });
            RepHandle { mol: m.mol, rep: RepRef::Last, queue: m.queue.clone() }
        });
        engine.register_fn("delete_rep", move |m: &mut MolHandle, i: i64| -> MolHandle {
            m.push(Command::DeleteRep { mol: m.mol, rep: idx(i) });
            m.clone()
        });
        engine.register_fn("show", |m: &mut MolHandle| -> MolHandle {
            m.push(Command::ShowMol { mol: m.mol, visible: true });
            m.clone()
        });
        engine.register_fn("hide", |m: &mut MolHandle| -> MolHandle {
            m.push(Command::ShowMol { mol: m.mol, visible: false });
            m.clone()
        });
        engine.register_fn("frame", move |m: &mut MolHandle, n: i64| -> MolHandle {
            m.push(Command::Frame { mol: m.mol, index: idx(n) });
            m.clone()
        });
        engine.register_fn("play", |m: &mut MolHandle| -> MolHandle {
            m.push(Command::Play { mol: m.mol, on: true });
            m.clone()
        });
        engine.register_fn("pause", |m: &mut MolHandle| -> MolHandle {
            m.push(Command::Play { mol: m.mol, on: false });
            m.clone()
        });
        engine.register_fn("focus", |m: &mut MolHandle, sel: &str| -> MolHandle {
            m.push(Command::Focus { mol: m.mol, text: sel.to_string() });
            m.clone()
        });

        // --- RepHandle setters (return the handle for chaining). ---
        engine.register_fn("set_style", |r: &mut RepHandle, s: &str| -> RepHandle {
            r.push(Command::Style { mol: r.mol, rep: r.rep, kind: s.to_string() });
            r.clone()
        });
        engine.register_fn("set_color", |r: &mut RepHandle, c: &str| -> RepHandle {
            r.push(Command::Color { mol: r.mol, rep: r.rep, method: c.to_string() });
            r.clone()
        });
        engine.register_fn("set_material", |r: &mut RepHandle, m: &str| -> RepHandle {
            r.push(Command::Material { mol: r.mol, rep: r.rep, name: m.to_string() });
            r.clone()
        });
        engine.register_fn("select", |r: &mut RepHandle, t: &str| -> RepHandle {
            r.push(Command::Select { mol: r.mol, rep: r.rep, text: t.to_string() });
            r.clone()
        });

        ScriptSession { engine, scope: rhai::Scope::new(), queue, out, summary }
    }

    /// Reset the REPL: drop all variables (and any leftover queue/output). Used when
    /// the document is replaced (New / Load session) so handles don't outlive their
    /// scene. (No document-reset flow on wasm yet, so it's unused there.)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn reset(&mut self) {
        self.scope.clear();
        self.queue.borrow_mut().clear();
        self.out.borrow_mut().clear();
    }

    /// Evaluate one REPL input line in the **persistent scope** (so `let` bindings
    /// persist across calls). `summary` is the current scene listing `list()` prints.
    /// The value of the last expression is echoed (unless it's unit), so a bare
    /// `mol(0)` shows what it evaluated to. Returns the commands to apply + output.
    pub fn eval(&mut self, source: &str, summary: String) -> EvalOutcome {
        *self.summary.borrow_mut() = summary;
        match self.engine.eval_with_scope::<rhai::Dynamic>(&mut self.scope, source) {
            Ok(val) if !val.is_unit() => {
                self.out
                    .borrow_mut()
                    .push(ConsoleLine { kind: LineKind::Output, text: repl_display(&val) });
            }
            Ok(_) => {}
            Err(e) => self
                .out
                .borrow_mut()
                .push(ConsoleLine { kind: LineKind::Error, text: e.to_string() }),
        }
        let commands = std::mem::take(&mut *self.queue.borrow_mut());
        let output = std::mem::take(&mut *self.out.borrow_mut());
        EvalOutcome { commands, output }
    }
}

/// Format a script result for the REPL echo: friendly text for our handle types
/// (which `Dynamic::to_string` would print as just the type name), else Rhai's own
/// stringification for primitives.
fn repl_display(val: &rhai::Dynamic) -> String {
    if let Some(m) = val.read_lock::<MolHandle>() {
        return m.to_string();
    }
    if let Some(r) = val.read_lock::<RepHandle>() {
        return r.to_string();
    }
    val.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One-shot evaluation in a fresh session (the console keeps a persistent one).
    fn evaluate_script(source: &str, summary: String) -> EvalOutcome {
        ScriptSession::new().eval(source, summary)
    }

    fn run(src: &str) -> Vec<Command> {
        evaluate_script(src, String::new()).commands
    }

    #[test]
    fn parses_fluent_commands() {
        assert_eq!(
            run(r#"mol(0).rep(0).set_color("chain")"#),
            vec![Command::Color { mol: 0, rep: RepRef::Index(0), method: "chain".into() }]
        );
        assert_eq!(
            run(r#"mol(0).rep(1).set_style("vdw")"#),
            vec![Command::Style { mol: 0, rep: RepRef::Index(1), kind: "vdw".into() }]
        );
        // add_rep("cartoon") = append + style the new (Last) rep.
        assert_eq!(
            run(r#"mol(0).add_rep("cartoon")"#),
            vec![
                Command::AddRep { mol: 0 },
                Command::Style { mol: 0, rep: RepRef::Last, kind: "cartoon".into() },
            ]
        );
    }

    #[test]
    fn chaining_and_loops_work() {
        // Chained setters off rep(0).
        assert_eq!(
            run(r#"mol(0).rep(0).set_style("vdw").set_color("element").select("protein")"#),
            vec![
                Command::Style { mol: 0, rep: RepRef::Index(0), kind: "vdw".into() },
                Command::Color { mol: 0, rep: RepRef::Index(0), method: "element".into() },
                Command::Select { mol: 0, rep: RepRef::Index(0), text: "protein".into() },
            ]
        );
        // Real language: a loop over molecules.
        let cmds = run(r#"for i in 0..3 { mol(i).rep(0).set_color("chain") }"#);
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[2], Command::Color { mol: 2, .. }));
    }

    /// The REPL keeps a persistent scope: a `let` binding on one `eval` is in scope
    /// on the next, and a handle stored in a variable still pushes commands into the
    /// (persistent) queue we drain. A bare expression echoes its value.
    #[test]
    fn repl_scope_persists_across_evals() {
        let mut s = ScriptSession::new();
        // Binding a handle pushes no command on its own.
        let o1 = s.eval("let m = mol(0)", String::new());
        assert!(o1.commands.is_empty());
        // On the next line `m` is still in scope and drives a command.
        let o2 = s.eval(r#"m.rep(0).set_color("chain")"#, String::new());
        assert_eq!(
            o2.commands,
            vec![Command::Color { mol: 0, rep: RepRef::Index(0), method: "chain".into() }]
        );
        // A bare handle expression echoes meaningfully.
        let o3 = s.eval("m.rep(1)", String::new());
        assert!(o3.commands.is_empty());
        assert!(o3.output.iter().any(|l| l.text == "mol(0).rep(1)"));
        // reset() drops the scope: `m` is gone afterwards.
        s.reset();
        let o4 = s.eval("m", String::new());
        assert!(o4.output.iter().any(|l| l.kind == LineKind::Error));
    }

    #[test]
    fn syntax_error_is_reported_not_panicked() {
        let outcome = evaluate_script("mol(0).rep(", String::new());
        assert!(outcome.commands.is_empty());
        assert!(outcome.output.iter().any(|l| l.kind == LineKind::Error));
    }

    #[test]
    fn color_parser_matches_labels() {
        assert_eq!(parse_color("Chain"), Some(crate::color::ColorMethod::Chain));
        assert_eq!(parse_color("ss"), Some(crate::color::ColorMethod::SecStruct));
        assert!(parse_color("bogus").is_none());
    }

    fn load_scene() -> crate::scene::Scene {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(std::path::Path::new(path)).expect("load 2lao.pdb");
        let mut scene = crate::scene::Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene.selected_mol = Some(0);
        scene
    }

    /// Commands mutate the rep + set the right dirty flag; `add_rep("cartoon")` grows
    /// the list and styles the new rep; selection resolves against molar's grammar.
    #[test]
    fn commands_mutate_the_scene() {
        let mut scene = load_scene();
        let mut camera = crate::camera::Camera::default();
        let defaults = crate::settings::RepDefaults::default();
        let apply = |scene: &mut crate::scene::Scene, camera: &mut crate::camera::Camera, src: &str| {
            for cmd in evaluate_script(src, String::new()).commands {
                apply_scene_command(scene, camera, &defaults, cmd).expect("command ok");
            }
        };

        apply(&mut scene, &mut camera, r#"mol(0).rep(0).set_style("vdw").set_color("chain").select("name CA")"#);
        let rep = &scene.molecules[0].reps[0];
        assert_eq!(rep.color, crate::color::ColorMethod::Chain);
        assert_eq!(rep.kind, crate::geometry::RepKind::Vdw);
        assert!(rep.geom_dirty);
        assert_eq!(rep.sel_text, "name CA");
        assert!(rep.sel_dirty);
        // `evaluate` returns Err(Empty) for a zero-atom match, so Ok ⇒ matched ≥1 atom.
        scene.molecules[0].data.evaluate(&rep.sel_text)
            .expect("name CA evaluates to a non-empty selection");

        // add_rep("cartoon") appends + styles the new (Last) rep.
        apply(&mut scene, &mut camera, r#"mol(0).add_rep("cartoon")"#);
        assert_eq!(scene.molecules[0].reps.len(), 2);
        assert_eq!(scene.molecules[0].reps[1].kind, crate::geometry::RepKind::Cartoon);

        // Bad value / index → clean Err, not a panic.
        assert!(apply_scene_command(
            &mut scene,
            &mut camera,
            &defaults,
            Command::Color { mol: 0, rep: RepRef::Index(0), method: "bogus".into() },
        )
        .is_err());
        assert!(apply_scene_command(
            &mut scene,
            &mut camera,
            &defaults,
            Command::Color { mol: 9, rep: RepRef::Index(0), method: "chain".into() },
        )
        .is_err());
    }
}
