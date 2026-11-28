//! Embed one persistent viewer on an existing canvas. Keep the returned handle
//! for the lifetime of the component. Dropping it tears down the graphics runner.
#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};

use molar::prelude::{Sel, SelectionExpr, State, System, Topology};
pub use molar_vis_core::MoleculeEvent;
use molar_vis_core::{eframe, App, AppJob, AppLaunch, EvalError, SharedSource};
use web_sys::HtmlCanvasElement;

thread_local! {
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// An immutable molar system shared by the host and the viewer.
pub struct SystemSource(pub Rc<System>);

impl SharedSource for SystemSource {
    fn topology(&self) -> &Topology {
        self.0.topology()
    }
    fn state(&self) -> &State {
        self.0.state()
    }
    fn coords_version(&self) -> u64 {
        0
    }
    fn evaluate(&self, text: &str) -> Result<(SelectionExpr, Sel), EvalError> {
        molar_vis_core::evaluate(&self.0, text)
    }
}

/// Owns the renderer. Molecule and representation handles expire on scene clear.
pub struct ViewerHandle {
    runner: eframe::WebRunner,
    jobs: Sender<AppJob>,
}

impl ViewerHandle {
    /// Start with the sidebar and selection toolbar hidden.
    pub async fn start(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let (_, startup) = Self::start_deferred(canvas, true)?;
        startup.await
    }

    /// Obtain the command queue before startup finishes. This also supports the
    /// synchronous JavaScript entry point, whose callers can queue work immediately.
    pub fn start_deferred(
        canvas: HtmlCanvasElement,
        embedded: bool,
    ) -> Result<(Sender<AppJob>, impl Future<Output = Result<Self, String>>), String> {
        if RUNNING.with(|running| running.replace(true)) {
            return Err("a molar_vis viewer is already running on this page".into());
        }
        let (jobs, receiver) = mpsc::channel();
        let viewer = Self {
            runner: eframe::WebRunner::new(),
            jobs: jobs.clone(),
        };
        let startup = async move {
            viewer
                .runner
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(move |cc| {
                        let mut app = App::new(cc, AppLaunch::default())?;
                        if embedded {
                            app.set_ui_visibility(false, false);
                        }
                        app.set_jobs(receiver);
                        Ok(Box::new(app))
                    }),
                )
                .await
                .map_err(|error| molar_vis_core::js_error_chain(&error))?;
            Ok(viewer)
        };
        Ok((jobs, startup))
    }

    /// Apply a host interaction synchronously and schedule one repaint.
    /// Call between viewer frames. Await earlier queued operations if their order matters.
    pub fn update_now<T>(
        &self,
        edit: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut app = self
            .runner
            .app_mut::<App>()
            .ok_or("viewer is busy or closed")?;
        app.update_from_host(edit)
    }

    /// Run an operation on the viewer thread and await its result.
    pub async fn with_app<T: 'static>(
        &self,
        f: impl FnOnce(&mut App) -> Result<T, String> + 'static,
    ) -> Result<T, String> {
        query(&self.jobs, f).await
    }

    pub async fn clear(&self) -> Result<(), String> {
        self.with_app(|app| {
            app.clear();
            Ok(())
        })
        .await
    }

    /// Add an immutable, parsed structure by reference.
    pub async fn add_system(
        &self,
        name: String,
        system: Rc<System>,
    ) -> Result<MoleculeHandle, String> {
        let id = molar_vis_core::reserve_mol_id().0;
        self.with_app(move |app| {
            app.add_shared_molecule_with_id(id, Box::new(SystemSource(system)), name)
        })
        .await?;
        Ok(MoleculeHandle {
            id,
            jobs: self.jobs.clone(),
        })
    }

    /// Load an owned molecule that the drawing editor can modify.
    pub async fn add_editable(
        &self,
        name: String,
        bytes: Vec<u8>,
    ) -> Result<MoleculeHandle, String> {
        let id = self
            .with_app(move |app| app.add_editable_bytes(&name, bytes))
            .await?;
        Ok(MoleculeHandle {
            id,
            jobs: self.jobs.clone(),
        })
    }
}

impl Drop for ViewerHandle {
    fn drop(&mut self) {
        self.runner.destroy();
        RUNNING.with(|running| running.set(false));
    }
}

/// Queue one operation and report closure if startup fails or the viewer shuts down.
pub async fn query<T: 'static>(
    jobs: &Sender<AppJob>,
    f: impl FnOnce(&mut App) -> Result<T, String> + 'static,
) -> Result<T, String> {
    let (sender, receiver) = futures_channel::oneshot::channel();
    jobs.send(Box::new(move |app| {
        let _ = sender.send(f(app));
    }))
    .map_err(|_| "viewer closed")?;
    receiver.await.map_err(|_| "viewer closed")?
}

#[derive(Clone)]
pub struct MoleculeHandle {
    id: u64,
    jobs: Sender<AppJob>,
}

impl MoleculeHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Includes the default representation created when the molecule was loaded.
    pub async fn representations(&self) -> Result<Vec<RepresentationHandle>, String> {
        let mol = self.id;
        let ids = query(&self.jobs, move |app| app.rep_ids(mol)).await?;
        Ok(ids
            .into_iter()
            .map(|rep| RepresentationHandle {
                mol,
                rep,
                jobs: self.jobs.clone(),
            })
            .collect())
    }

    pub async fn set_editing(&self, enabled: bool) -> Result<(), String> {
        let id = self.id;
        query(&self.jobs, move |app| app.set_molecule_editing(id, enabled)).await
    }

    pub async fn export_cif(&self) -> Result<String, String> {
        let id = self.id;
        query(&self.jobs, move |app| app.molecule_cif(id)).await
    }

    /// Receive editor-mode transitions, structural edits, and removal.
    pub async fn events(
        &self,
    ) -> Result<futures_channel::mpsc::UnboundedReceiver<MoleculeEvent>, String> {
        let id = self.id;
        query(&self.jobs, move |app| app.watch_molecule(id)).await
    }

    pub async fn add_rep(
        &self,
        selection: String,
        style: String,
        color: String,
    ) -> Result<RepresentationHandle, String> {
        let mol = self.id;
        let rep = query(&self.jobs, move |app| {
            app.add_styled_rep(mol, &selection, &style, &color)
        })
        .await?;
        Ok(RepresentationHandle {
            mol,
            rep,
            jobs: self.jobs.clone(),
        })
    }
}

#[derive(Clone)]
pub struct RepresentationHandle {
    mol: u64,
    rep: u64,
    jobs: Sender<AppJob>,
}

impl RepresentationHandle {
    pub async fn remove(&self) -> Result<(), String> {
        let (mol, rep) = (self.mol, self.rep);
        query(&self.jobs, move |app| app.remove_rep_by_id(mol, rep)).await
    }

    pub async fn set_visible(&self, visible: bool) -> Result<(), String> {
        let (mol, rep) = (self.mol, self.rep);
        query(&self.jobs, move |app| {
            app.set_rep_visible(mol, rep, visible)
        })
        .await
    }

    pub async fn select_text(&self, selection: String) -> Result<(), String> {
        let (mol, rep) = (self.mol, self.rep);
        query(&self.jobs, move |app| {
            let (mi, ri) = app.resolve_rep(mol, rep)?;
            app.set_rep_selection_text(mi, ri, &selection)
        })
        .await
    }
}
