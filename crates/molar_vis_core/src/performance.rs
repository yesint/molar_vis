//! Opt-in JSONL timings. No queries, buffers, or clock reads when disabled.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

pub(crate) fn enabled() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("MOLAR_VIS_PROFILE").is_some())
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

pub(crate) struct Span {
    name: &'static str,
    #[cfg(not(target_arch = "wasm32"))]
    start: std::time::Instant,
}

pub(crate) fn span(name: &'static str) -> Option<Span> {
    enabled().then(|| Span {
        name,
        #[cfg(not(target_arch = "wasm32"))]
        start: std::time::Instant::now(),
    })
}

impl Drop for Span {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        emit(
            "cpu",
            self.name,
            self.start.elapsed().as_secs_f64() * 1000.0,
        );
        #[cfg(target_arch = "wasm32")]
        let _ = self.name;
    }
}

fn emit(kind: &str, name: &str, ms: f64) {
    eprintln!(
        "MOLAR_VIS_PROFILE {}",
        serde_json::json!({"kind": kind, "stage": name, "ms": ms})
    );
}

pub(crate) fn counters(stage: &str, values: serde_json::Value) {
    if enabled() {
        eprintln!(
            "MOLAR_VIS_PROFILE {}",
            serde_json::json!({"kind": "counters", "stage": stage, "values": values})
        );
    }
}

/// Each instrumented submission owns its query/readback buffers. At most four
/// readbacks can be pending globally; a busy GPU drops measurements, never work.
pub(crate) struct GpuFrame {
    queries: wgpu::QuerySet,
    labels: Vec<&'static str>,
    pending: Arc<AtomicUsize>,
}
const QUERY_CAPACITY: u32 = 32;
const PENDING_LIMIT: usize = 4;

impl GpuFrame {
    pub(crate) fn begin(device: &wgpu::Device) -> Option<Self> {
        Self::begin_if(device, enabled())
    }

    fn begin_if(device: &wgpu::Device, enabled: bool) -> Option<Self> {
        if !enabled || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        static PENDING: OnceLock<Arc<AtomicUsize>> = OnceLock::new();
        let pending = PENDING
            .get_or_init(|| Arc::new(AtomicUsize::new(0)))
            .clone();
        pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < PENDING_LIMIT).then_some(count + 1)
            })
            .ok()?;
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("performance-timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: QUERY_CAPACITY,
            }),
            labels: Vec::new(),
            pending,
        })
    }

    fn indices(&mut self, label: &'static str) -> Option<(u32, u32)> {
        let first = self.labels.len() as u32 * 2;
        if first + 1 >= QUERY_CAPACITY {
            return None;
        }
        self.labels.push(label);
        Some((first, first + 1))
    }

    pub(crate) fn render(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let (first, last) = self.indices(label)?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(first),
            end_of_pass_write_index: Some(last),
        })
    }

    pub(crate) fn compute(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let (first, last) = self.indices(label)?;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(first),
            end_of_pass_write_index: Some(last),
        })
    }

    /// Encode query resolution before submission; map only after it was submitted.
    pub(crate) fn resolve(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<wgpu::Buffer> {
        let count = self.labels.len() as u32 * 2;
        if count == 0 {
            return None;
        }
        let size = count as u64 * 8;
        let resolved = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("performance-resolved"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("performance-readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.resolve_query_set(&self.queries, 0..count, &resolved, 0);
        encoder.copy_buffer_to_buffer(&resolved, 0, &readback, 0, size);
        Some(readback)
    }

    pub(crate) fn read(self, queue: &wgpu::Queue, buffer: wgpu::Buffer) {
        let period = queue.get_timestamp_period() as f64;
        let mapped = buffer.clone();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                if result.is_ok() {
                    {
                        let data = mapped.slice(..).get_mapped_range();
                        for (i, label) in self.labels.iter().enumerate() {
                            let start =
                                u64::from_le_bytes(data[i * 16..i * 16 + 8].try_into().unwrap());
                            let end = u64::from_le_bytes(
                                data[i * 16 + 8..i * 16 + 16].try_into().unwrap(),
                            );
                            emit("gpu", label, end.wrapping_sub(start) as f64 * period / 1e6);
                        }
                    }
                    mapped.unmap();
                }
                // Dropping self releases the pending slot, including on map errors.
            });
    }
}

impl Drop for GpuFrame {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn submit(
    rs: &eframe::egui_wgpu::RenderState,
    mut encoder: wgpu::CommandEncoder,
    frame: Option<GpuFrame>,
) {
    let readback = frame
        .as_ref()
        .and_then(|f| f.resolve(&rs.device, &mut encoder));
    rs.queue.submit([encoder.finish()]);
    if let (Some(frame), Some(buffer)) = (frame, readback) {
        frame.read(&rs.queue, buffer);
        // Nonblocking: progress callbacks for earlier submissions as well.
        let _ = rs.device.poll(wgpu::PollType::Poll);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a timestamp-capable GPU adapter"]
    fn timestamp_readbacks_are_bounded_and_release_slots() {
        let (device, queue) = pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::TIMESTAMP_QUERY,
                    ..Default::default()
                })
                .await
                .unwrap()
        });
        assert!(GpuFrame::begin_if(&device, false).is_none());
        let mut frames: Vec<_> = (0..PENDING_LIMIT)
            .map(|_| GpuFrame::begin_if(&device, true).unwrap())
            .collect();
        assert!(GpuFrame::begin_if(&device, true).is_none());
        let mut frame = frames.pop().unwrap();
        let pending = frame.pending.clone();
        drop(frames);
        assert_eq!(pending.load(Ordering::Relaxed), 1);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("timestamp-test"),
                timestamp_writes: frame.compute("timestamp-test"),
            });
        }
        let buffer = frame.resolve(&device, &mut encoder).unwrap();
        queue.submit([encoder.finish()]);
        frame.read(&queue, buffer);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert_eq!(pending.load(Ordering::Relaxed), 0);
        drop(GpuFrame::begin_if(&device, true).unwrap());
        assert_eq!(pending.load(Ordering::Relaxed), 0);
    }
}
