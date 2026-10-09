//! Native bounded geometry jobs. GPU resources and molecule providers stay on the UI thread.
use crate::geometry::{CartoonInput, GeometryData, SurfaceInput};
use crate::material::Material;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};

pub(crate) enum Input {
    Surface(SurfaceInput, Material),
    Cartoon(CartoonInput),
}
impl Input {
    pub(crate) fn worth_offloading(&self) -> bool {
        match self {
            Self::Surface(input, _) => input.atom_count() >= 256,
            Self::Cartoon(input) => input.residue_count() >= 1024,
        }
    }
    fn build(self, cancel: &AtomicBool) -> GeometryData {
        match self {
            Self::Surface(input, material) => {
                let mut geom = GeometryData {
                    mesh: input.build(|| cancel.load(Ordering::Relaxed)),
                    ..Default::default()
                };
                crate::geometry::stamp_material(&mut geom, material);
                geom
            }
            Self::Cartoon(input) => input.build(),
        }
    }
}

struct Budget {
    active: AtomicUsize,
    limit: usize,
}
struct Permit(Arc<Budget>);
impl Budget {
    fn reserve(self: &Arc<Self>) -> Option<Permit> {
        self.active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < self.limit).then_some(n + 1)
            })
            .ok()?;
        Some(Permit(self.clone()))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) enum Poll {
    Pending,
    Complete(GeometryData),
    Failed,
}
pub(crate) struct Job {
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Option<GeometryData>>,
}
impl Job {
    fn budget() -> &'static Arc<Budget> {
        static BUDGET: OnceLock<Arc<Budget>> = OnceLock::new();
        BUDGET.get_or_init(|| {
            Arc::new(Budget {
                active: AtomicUsize::new(0),
                limit: 2,
            })
        })
    }
    pub(crate) fn capacity_available() -> bool {
        Self::budget().active.load(Ordering::Relaxed) < 2
    }
    pub(crate) fn try_start(input: Input) -> Result<Self, Input> {
        let budget = Self::budget();
        let Some(permit) = budget.reserve() else {
            return Err(input);
        };
        Ok(Self::spawn(permit, move |cancel| input.build(cancel)))
    }
    fn spawn(
        permit: Permit,
        build: impl FnOnce(&AtomicBool) -> GeometryData + Send + 'static,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, result) = mpsc::sync_channel(1);
        rayon::spawn(move || {
            let _permit = permit;
            if worker_cancel.load(Ordering::Relaxed) {
                return;
            }
            let output =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&worker_cancel)));
            if !worker_cancel.load(Ordering::Relaxed) {
                let _ = tx.send(output.ok());
            }
        });
        Self { cancel, result }
    }
    pub(crate) fn poll(&self) -> Poll {
        match self.result.try_recv() {
            Ok(Some(geom)) => Poll::Complete(geom),
            Err(mpsc::TryRecvError::Empty) => Poll::Pending,
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => Poll::Failed,
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jobs_bound_work_cancel_old_results_and_recover_from_failure() {
        // One coordinator and two deliberately blocked workers. Do not depend
        // on the user's global Rayon thread count for this scheduling test.
        rayon::ThreadPoolBuilder::new()
            .num_threads(3)
            .build()
            .unwrap()
            .install(|| {
                let budget = Arc::new(Budget {
                    active: AtomicUsize::new(0),
                    limit: 2,
                });
                let (finish_old, old_wait) = mpsc::channel();
                let old = Job::spawn(budget.reserve().unwrap(), move |_| {
                    old_wait.recv().unwrap();
                    GeometryData::default()
                });
                let cancel = old.cancel.clone();
                let (finish_new, new_wait) = mpsc::channel();
                let newest = Job::spawn(budget.reserve().unwrap(), move |_| {
                    new_wait.recv().unwrap();
                    GeometryData::default()
                });
                assert!(budget.reserve().is_none());
                drop(old);
                assert!(cancel.load(Ordering::Relaxed));
                finish_new.send(()).unwrap();
                let wait = |job: &Job| {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    loop {
                        match job.poll() {
                            Poll::Pending => {
                                assert!(std::time::Instant::now() < deadline);
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            }
                            result => break result,
                        }
                    }
                };
                assert!(matches!(wait(&newest), Poll::Complete(_)));
                finish_old.send(()).ok();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while budget.active.load(Ordering::Relaxed) != 0 {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                let broken =
                    Job::spawn(budget.reserve().unwrap(), |_| panic!("test worker failure"));
                assert!(matches!(wait(&broken), Poll::Failed));
                drop(broken);
                assert!(budget.reserve().is_some());
            });
    }
}
