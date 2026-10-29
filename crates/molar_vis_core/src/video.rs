//! Native frame sink: a bounded queue feeding raw RGBA to an external FFmpeg.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub raytrace: bool,
    pub samples: u32,
    pub crf: u8,
    pub ffmpeg: PathBuf,
    /// Write numbered PNGs to a new directory instead of starting FFmpeg.
    pub png_sequence: bool,
    /// Native: snapshot the scene and render independently of the interactive view.
    pub background: bool,
}
impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            raytrace: false,
            samples: 128,
            crf: 18,
            ffmpeg: "ffmpeg".into(),
            png_sequence: false,
            background: false,
        }
    }
}
impl VideoOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > 16384 || self.height > 16384 {
            return Err("video dimensions must be between 1 and 16384".into());
        }
        if !(1..=240).contains(&self.fps) {
            return Err("fps must be between 1 and 240".into());
        }
        if self.samples == 0 || self.crf > 51 {
            return Err("samples must be positive and CRF must be 0–51".into());
        }
        Ok(())
    }
    pub(crate) fn frame_count(&self, duration: f64) -> Result<u64, String> {
        let count = (duration * self.fps as f64).ceil();
        if !count.is_finite() || count < 1.0 || count > u32::MAX as f64 {
            return Err("movie must contain 1–4294967295 frames".into());
        }
        Ok(count as u64)
    }
}

pub(crate) struct FrameSink {
    pub tx: Option<mpsc::SyncSender<Vec<u8>>>,
    result: mpsc::Receiver<Result<(), String>>,
    child: Option<Arc<Mutex<Child>>>,
    temp: PathBuf,
    destination: PathBuf,
    png: bool,
    committed: bool,
    worker: Option<thread::JoinHandle<()>>,
}
impl FrameSink {
    pub fn start(destination: PathBuf, options: VideoOptions) -> Result<Self, String> {
        options.validate()?;
        if options.png_sequence && destination.exists() {
            return Err("frame directory already exists; choose a new directory".into());
        }
        if !options.png_sequence && destination.is_dir() {
            return Err("video output path is a directory".into());
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = parent.join(format!(
            ".molar-vis-{}-{serial}.{}",
            std::process::id(),
            if options.png_sequence {
                "frames"
            } else {
                "mp4"
            }
        ));
        let mut child = None;
        let mut stdin = None;
        let mut stderr = None;
        if options.png_sequence {
            std::fs::create_dir(&temp).map_err(|e| crate::errors::error_chain(&e))?;
        } else {
            let mut process = Command::new(&options.ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-n",
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    "rgba",
                    "-video_size",
                ])
                .arg(format!("{}x{}", options.width, options.height))
                .args([
                    "-framerate",
                    &options.fps.to_string(),
                    "-i",
                    "pipe:0",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "medium",
                    "-crf",
                    &options.crf.to_string(),
                    "-pix_fmt",
                    // Preserve odd viewport dimensions exactly rather than crop/pad/resize.
                    if options.width & 1 != 0 || options.height & 1 != 0 {
                        "yuv444p"
                    } else {
                        "yuv420p"
                    },
                    "-movflags",
                    "+faststart",
                    "-f",
                    "mp4",
                ])
                .arg(&temp)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("cannot start FFmpeg ({}): {e}", options.ffmpeg.display(), e = crate::errors::error_chain(&e)))?;
            stdin = process.stdin.take();
            stderr = process.stderr.take();
            child = Some(Arc::new(Mutex::new(process)));
        }
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let (done_tx, result) = mpsc::channel();
        let worker_child = child.clone();
        let work_dir = temp.clone();
        let png = options.png_sequence;
        let spawn = thread::Builder::new()
            .name("movie-encoder".into())
            .spawn(move || {
                // Drain stderr concurrently so the encoder cannot deadlock on a full pipe.
                let error_reader = stderr.map(|mut stream| {
                    thread::spawn(move || {
                        let mut tail = Vec::new();
                        let mut buf = [0u8; 4096];
                        loop {
                            let n = match stream.read(&mut buf) {
                                Ok(n) => n,
                                Err(error) => {
                                    log::error!("cannot read FFmpeg diagnostics: {}", crate::errors::error_chain(&error));
                                    break;
                                }
                            };
                            if n == 0 {
                                break;
                            }
                            tail.extend_from_slice(&buf[..n]);
                            if tail.len() > 16384 {
                                tail.drain(..tail.len() - 16384);
                            }
                        }
                        String::from_utf8_lossy(&tail).into_owned()
                    })
                });
                let outcome = (|| -> Result<(), String> {
                    for (index, rgba) in rx.iter().enumerate() {
                        if rgba.len() != options.width as usize * options.height as usize * 4 {
                            return Err("incorrect video frame size".into());
                        }
                        if let Some(pipe) = stdin.as_mut() {
                            pipe.write_all(&rgba)
                                .map_err(|e| format!("FFmpeg frame write failed: {e}", e = crate::errors::error_chain(&e)))?;
                        } else {
                            image::save_buffer(
                                work_dir.join(format!("frame_{index:06}.png")),
                                &rgba,
                                options.width,
                                options.height,
                                image::ColorType::Rgba8,
                            )
                            .map_err(|e| crate::errors::error_chain(&e))?;
                        }
                    }
                    drop(stdin.take()); // EOF flushes delayed packets and the MP4 trailer.
                    if let Some(process) = &worker_child {
                        loop {
                            if let Some(status) = process
                                .lock()
                                .unwrap()
                                .try_wait()
                                .map_err(|e| crate::errors::error_chain(&e))?
                            {
                                if !status.success() {
                                    return Err(format!("FFmpeg exited with {status}"));
                                }
                                break;
                            }
                            thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                    Ok(())
                })();
                drop(stdin);
                if outcome.is_err() {
                    if let Some(process) = &worker_child {
                        let _ = process.lock().unwrap().kill();
                    }
                }
                // Reap the child even when writing failed or the export was cancelled.
                if let Some(process) = &worker_child {
                    let _ = process.lock().unwrap().wait();
                }
                let detail = error_reader.and_then(|t| t.join().map_err(|payload| {
                    log::error!("FFmpeg diagnostics worker failed: {}", crate::errors::panic_message(payload.as_ref()));
                }).ok()).unwrap_or_default();
                let outcome = outcome.map_err(|e| {
                    if detail.is_empty() {
                        e
                    } else {
                        format!("{e}: {}", detail.trim())
                    }
                });
                let _ = done_tx.send(outcome);
            });
        let worker = match spawn {
            Ok(worker) => worker,
            Err(e) => {
                if let Some(process) = &child {
                    let _ = process.lock().unwrap().kill();
                    let _ = process.lock().unwrap().wait();
                }
                if png {
                    let _ = std::fs::remove_dir(&temp);
                }
                return Err(format!("cannot start encoder worker: {}", crate::errors::error_chain(&e)));
            }
        };
        Ok(Self {
            tx: Some(tx),
            result,
            child,
            temp,
            destination,
            png,
            committed: false,
            worker: Some(worker),
        })
    }
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        match self.result.try_recv() {
            Ok(Ok(())) => {
                // Publish only complete output. Rename replaces an existing movie;
                // cancellation and encoding errors leave the previous file intact.
                let result = if self.png {
                    if self.destination.exists() {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "output already exists",
                        ))
                    } else {
                        std::fs::rename(&self.temp, &self.destination)
                    }
                } else {
                    std::fs::rename(&self.temp, &self.destination)
                }
                .map_err(|e| format!("cannot publish video output: {e}", e = crate::errors::error_chain(&e)));
                self.committed = result.is_ok();
                Some(result)
            }
            Ok(Err(e)) => Some(Err(e)),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("encoder worker stopped".into())),
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
impl Drop for FrameSink {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(process) = &self.child {
                let _ = process.lock().unwrap().kill();
            }
            // The worker reaps FFmpeg. Unlinking an open temporary file is safe on Unix;
            // on Windows the worker may still hold it briefly, so clean up after exit too.
            self.tx.take();
            let temp = self.temp.clone();
            let png = self.png;
            let worker = self.worker.take();
            thread::spawn(move || {
                if let Some(worker) = worker {
                    let _ = worker.join();
                }
                if png {
                    let _ = std::fs::remove_dir_all(temp);
                } else {
                    let _ = std::fs::remove_file(temp);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn directory(label: &str) -> PathBuf {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("movie-test-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn finish(sink: &mut FrameSink) -> Result<(), String> {
        sink.tx.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(result) = sink.poll() {
                return result;
            }
            assert!(std::time::Instant::now() < deadline, "encoder timed out");
            thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[test]
    fn validates_parameters_and_missing_encoder() {
        let mut options = VideoOptions::default();
        options.width = 101;
        assert!(options.validate().is_ok());
        options.png_sequence = true;
        assert!(options.validate().is_ok());
        options.fps = 0;
        assert!(options.validate().is_err());
        let path = directory("missing").join("missing.mp4");
        let options = VideoOptions {
            ffmpeg: "/nonexistent/molar-vis-ffmpeg".into(),
            ..Default::default()
        };
        assert!(FrameSink::start(path, options)
            .err()
            .unwrap()
            .contains("cannot start FFmpeg"));
        assert_eq!(VideoOptions::default().frame_count(0.1).unwrap(), 3);
    }
    #[test]
    fn png_sequence_commits_only_on_completion() {
        let dir = directory("png");
        let output = dir.join("frames");
        let _ = std::fs::remove_dir_all(&output);
        let options = VideoOptions {
            width: 4,
            height: 3,
            png_sequence: true,
            ..Default::default()
        };
        let mut sink = FrameSink::start(output.clone(), options.clone()).unwrap();
        sink.tx
            .as_ref()
            .unwrap()
            .send(vec![255; 4 * 3 * 4])
            .unwrap();
        assert!(!output.exists());
        finish(&mut sink).unwrap();
        assert_eq!(
            image::open(output.join("frame_000000.png"))
                .unwrap()
                .width(),
            4
        );
        assert!(FrameSink::start(output, options).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    #[ignore = "requires ffmpeg with libx264 and ffprobe"]
    fn ffmpeg_preserves_odd_screen_dimensions() {
        let dir = directory("odd-screen");
        let output = dir.join("screen.mp4");
        let options = VideoOptions {
            width: 65,
            height: 49,
            fps: 12,
            ..Default::default()
        };
        let mut sink = FrameSink::start(output.clone(), options).unwrap();
        sink.tx
            .as_ref()
            .unwrap()
            .send(vec![255; 65 * 49 * 4])
            .unwrap();
        finish(&mut sink).unwrap();
        let result = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=width,height,pix_fmt",
                "-of",
                "json",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(result.status.success());
        let info: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(info["streams"][0]["width"], 65);
        assert_eq!(info["streams"][0]["height"], 49);
        assert_eq!(info["streams"][0]["pix_fmt"], "yuv444p");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires ffmpeg with libx264 and ffprobe"]
    fn ffmpeg_pipe_produces_expected_frames_and_reports_errors() {
        let dir = directory("ffmpeg");
        let output = dir.join("clip.mp4");
        let _ = std::fs::remove_file(&output);
        let options = VideoOptions {
            width: 64,
            height: 48,
            fps: 12,
            ..Default::default()
        };
        std::fs::write(&output, b"previous movie").unwrap();
        let mut sink = FrameSink::start(output.clone(), options.clone()).unwrap();
        for i in 0..6 {
            let mut pixels = vec![0u8; 64 * 48 * 4];
            for p in pixels.chunks_exact_mut(4) {
                p.copy_from_slice(&[i * 30, 80, 150, 255]);
            }
            sink.tx.as_ref().unwrap().send(pixels).unwrap();
        }
        assert_eq!(std::fs::read(&output).unwrap(), b"previous movie");
        finish(&mut sink).unwrap();
        let result = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,width,height,avg_frame_rate,nb_read_frames",
                "-of",
                "json",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(result.status.success());
        let info: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        let stream = &info["streams"][0];
        assert_eq!(stream["codec_name"], "h264");
        assert_eq!(stream["width"], 64);
        assert_eq!(stream["height"], 48);
        assert_eq!(stream["avg_frame_rate"], "12/1");
        assert_eq!(stream["nb_read_frames"], "6");
        std::fs::write(dir.join("failed.mp4"), b"keep old movie").unwrap();
        let mut failed = FrameSink::start(dir.join("failed.mp4"), options).unwrap();
        failed.tx.as_ref().unwrap().send(vec![0; 3]).unwrap();
        assert!(finish(&mut failed)
            .unwrap_err()
            .contains("incorrect video frame size"));
        assert_eq!(
            std::fs::read(dir.join("failed.mp4")).unwrap(),
            b"keep old movie"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
