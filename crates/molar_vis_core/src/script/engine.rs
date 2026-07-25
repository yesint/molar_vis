//! The **Rhai** engine behind the scripting console (the `scripting` feature).
//!
//! A fluent, object-oriented surface: `mol(i)` returns a [`MolHandle`], whose methods
//! (`add_rep`, `rep`, `show`/`hide`, `frame`, `play`/`pause`, `focus`) act on that
//! molecule; `mol(i).rep(j)` returns a [`RepHandle`] whose setters (`set_style`,
//! `set_color`, `set_material`, `select`) act on that representation and **return the
//! handle** so calls chain (`mol(0).rep(0).set_style("vdw").set_color("chain")`).
//!
//! The handles are lightweight — just an index (+ a [`RepRef`]) and a clone of a shared
//! command queue. Their methods **push** [`Command`]s (no scene access during eval),
//! sidestepping the borrow problem of handing Rhai closures `&mut App`. After `eval`
//! returns, `App::run_script` drains the queue through `App::execute_command` (→
//! [`apply_scene_command`](super::apply_scene_command), the same field-set + dirty-flag
//! the GUI does) on the UI thread, then records one undo checkpoint.
//!
//! Everything Rhai-specific lives here; the command vocabulary and its application to the
//! scene are in the parent module and stay available with the feature off, since the
//! Python/JS bindings and the app's own menu actions go through them.

use std::cell::RefCell;
use std::rc::Rc;

use super::command::{parse_color, Command, RepRef};
use super::console::{ConsoleLine, LineKind};

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
}
