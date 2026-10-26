# Camera animation and video export

Camera motion works in Rust, Python and JavaScript. Video export is native-only.
It streams raw RGBA frames through stdin to an external FFmpeg process; the viewer
has no linked video codec dependency. Install FFmpeg with the `libx264` encoder.
The default output is an opaque MP4/H.264 movie. Even dimensions use YUV 4:2:0;
odd dimensions use YUV 4:4:4 so the exact viewport size can be preserved.

## Python

Camera calls return an `Animation` handle and inherit **Settings → Rendering →
Camera transition animation [ms]** when `duration` is omitted. The setting defaults
to **0**, which applies moves immediately. Set it to a positive number for smooth
programmed navigation. An explicit `duration=0` forces immediate motion; an explicit
positive duration overrides the setting. API duration is in seconds; angles are in degrees.
Zoom is a multiplicative factor: `> 1` moves closer.

```python
motion = vis.rotate(90, 0, duration=1.5)
motion.wait()  # releases the GIL; the viewer remains responsive
vis.zoom(1.5, duration=0.8).wait()
vis.unobstructed_view(rep, duration=1.2).wait()
```

`rotate`, `roll`, `pan`, `zoom`, `focus`, `reset_view`, `go_to_view`,
`unobstructed_view` and `unobstructed_view_multi` accept `duration` and `easing`.
The default easing is `"smooth"`; use `"linear"` for a constant-speed turntable.
`focus` retains its existing `(mol, sel)` arguments. Save a pose with
`vis.view_json()` and pass that JSON to `vis.go_to_view(view_json, duration=1)`.
Poses include framing, orientation and rotation center; lighting, background and
projection stay under the normal view controls.

A new standalone motion replaces the active motion. Mouse navigation cancels it
at its current pose. Call `motion.cancel()` or `vis.cancel_animation()` explicitly.
To run several standalone motions in order, wait between calls, or build a movie.

```python
movie = vis.movie()
movie.unobstructed_view(rep, duration=1.2)
movie.hold(0.5)
movie.rotate(360, 0, duration=8, easing="linear")
movie.move_view(duration=1.5, yaw=45, zoom=1.4)  # simultaneous rotation and zoom
movie.hold(0.5)

preview = movie.preview()
preview.wait()
movie.seek(2.0)  # uses the same prepared start pose, not the current camera

export = movie.render("view.mp4", width=1920, height=1080, fps=30)
export.wait()  # returns only after FFmpeg has flushed and output is published
```

Movie building methods append steps and return `None`. In addition to the methods
above, movies support `focus(mol, selection_text, duration=1)`,
`unobstructed_view_multi(reps, ...)`, `reset_view(duration=1)` and
`go_to_view(view_json, duration=1)`. `movie.duration` is the sum of step durations.
Preview and seek cache their prepared timeline. Appending a step invalidates that
cache. Export prepares a new timeline from the current camera, using the output
aspect ratio. To export from the preview's original start, call `movie.seek(0)`
before rendering. An unobstructed first step searches for its destination during
preparation, before playback starts; this search can take time. Focus and
unobstructed destinations use the coordinates present at preparation time; they
do not continuously track moving atoms.

For a complete protein/inhibitor example, run
`python examples/jak2_movie.py`. It loads the bundled JAK2 protein and the first ten
SD inhibitor records, makes a three-second orbit, approaches an unobstructed pocket
view, shows receptor–ligand interaction dashes, and returns to the whole protein.
One MolGroup owns the ligand and interaction representations. Each inhibitor gets
a one-second camera transition with no subsequent hold. The complete sequence is
one movie and one render call. Output defaults to `target/jak2_movie.mp4`
(1280 × 720, 30 fps).

To use a saved scene's materials, representation parameters, colors and display
settings, run `python examples/jak2_movie.py --session ~/Downloads/session.mvs --visible`.
The script replaces the saved camera pose with its programmed starting pose and
keeps the same camera sequence. It renders in the foreground (`background=False`).

Load a native molecular group and add shared contacts once:

```python
ligands = vis.load_group("tests/jak2_inhs.sd")
contacts = ligands.add_interactions(protein_mol)
contacts.width = 3
movie = vis.movie()
for member in range(10):
    movie.unobstructed_group_member(ligands, member, duration=1, zoom_out=2)
movie.render("ligands.mp4").wait()
```

`vis.groups` lists groups; `len(group)`, `group.members` and `group.reps` expose
their members and shared representations. Set `group.current` to switch members
without moving the camera. Shared representation handles continue to refer to
the active member after a switch. `unobstructed_group_member` switches at the start
of its transition and searches for the member's destination during preparation.
Forward and backward seeking set the corresponding member deterministically.
Preparation restores the original shown members, including on failure; export
restores them on success, failure or cancellation. Preview leaves the sampled
member selected, like trajectory preview leaves its sampled frame.

To animate loaded molecular trajectory frames as well as the camera:

```python
movie.trajectory(mol, start=0, end=99, fps=15)
```

Tracks run from movie time zero. They select discrete frames without interpolation,
and stop at their end frame. Reverse ranges are supported. A second track for the
same molecule replaces the first. Trajectory frames must already be loaded.
Ordinary trajectory playback pauses during preview and export. Preview leaves the
final pose/frame and restores playback flags. Export restores the original camera,
trajectory frames and playback flags on success, cancellation or failure.

Export options:

| Option | Default | Meaning |
| --- | --- | --- |
| `width`, `height` | 1920, 1080 | Exact output pixels |
| `fps` | 30 | Integer frame rate, 1–240 |
| `raytrace` | `False` | Use the ray tracer instead of raster capture |
| `samples` | 128 | Ray-tracing samples per frame |
| `crf` | 18 | H.264 quality, 0–51; lower values give larger files |
| `ffmpeg` | `"ffmpeg"` | Executable name or path |
| `png_sequence` | `False` | Save numbered PNGs to a new directory instead |
| `background` | `False` | Render an independent scene snapshot on a worker thread |

For example, `movie.render("frames", png_sequence=True)` needs no FFmpeg.
An existing movie file is replaced after successful encoding. Until then the
previous file remains intact, including on cancellation or failure. PNG sequences
use a new destination directory. The viewer stages output next to its destination
and removes staged output on cancellation or failure. `Animation` exposes `done`, `state`, `completed`,
`total`, `error`, `cancel()` and `wait()`. Frame progress counts submitted frames;
encoding can continue after all frames have been submitted.

Export uses `ceil(duration * fps)` frames sampled at `i / fps`, starting at zero.
The terminal endpoint is excluded when the duration is exactly a whole number of
frames. Add a hold to show the final view for a specified time. Rendering and
encoding speed do not change movie timing. Full rotations preserve all requested
turns. Zoom interpolates multiplicatively, and saved-view rotation uses the short
quaternion path.

During foreground export, the viewer shows the rendered movie frames in the
viewport with a compact progress overlay and Cancel/Esc controls. Scene controls
stay visible and disabled until export finishes; queued host commands run afterward.
Keep external shared molecular data unchanged during foreground export.

With **Background render** checked, or `movie.render(..., background=True)`, the
viewer remains editable. A separate worker owns a frozen copy of the scene, camera,
settings and required trajectory frames. Later edits, new sessions, deletions and
camera moves do not change the movie. Completion never restores or changes the
live view. A small progress/Cancel panel has an optional Preview. One movie job can
run at a time. The worker uses small ray-tracing batches and slows down while the
viewer is handling input; both renderers still share GPU capacity.

The label under **Background render** estimates additional system RAM for the
snapshot, trajectory/smoothing frames, geometry caches, readback/frame buffers and
encoding. It is approximate and excludes dedicated GPU memory. Only the selected
trajectory ranges and their smoothing neighbors are copied, plus the current
frames of other molecules. Capture happens before dispatch and can briefly pause
large scenes. Python-backed data is copied under the GIL; the worker uses owned
data and needs no Python access.

Raster captures use the current supersampling setting, subject to device texture
limits. Ray tracing uses a fresh accumulation for each frame. PNG and movie
captures contain the scene without app panels.

## Native GUI

Use **Render → Movie…** (film-strip icon) and select the movie type from the dropdown:

- **Trajectory** plays a visible molecule's loaded trajectory with the current camera.
  Select the molecule, inclusive source-frame range and playback rate. The dialog
  shows the resulting duration. Reverse ranges play backwards.
- **Fly around** orbits the current view while molecular coordinates stay fixed.
  Select duration, rotation angle and an optional zoom factor.

**Size → Actual screen** uses the molecular viewport’s current pixel resolution
and aspect ratio. **Custom** enables manual dimensions. The Zoom factor tooltip
explains how the final scale relates to the starting view.

The dialog defaults to Trajectory when a visible molecule has a loaded trajectory;
otherwise it defaults to Fly around. Both modes offer output resolution, video
frame rate and raster/ray-traced rendering. Use the Python or Rust API for
multi-step movies or simultaneous camera motion and trajectory playback. The rep magnifier applies the configured camera transition duration after its
background search completes. Mouse navigation stays direct. Movie step durations
and export frame timing are independent of the navigation setting.

## JavaScript

Camera methods are asynchronous and resolve to an `Animation` when scheduled.
Await its `wait()` method to wait for completion:

```javascript
const motion = await vis.rotate(90, 0, 1.5, "smooth");
await motion.wait();

const movie = vis.movie();
movie.unobstructed_view(rep, 1.2, 1.0, 256);
movie.rotate(360, 0, 8, "linear");
movie.hold(0.5);
const preview = await movie.preview();
await preview.wait();
await movie.seek(2.0);
```

Omitting duration inherits the camera transition setting (0 ms by default). `Animation` has `done`, `cancel()`,
`wait()` and `progress_json()`. Use `vis.view_json()` / `vis.go_to_view(...)` for
saved poses, or `vis.animate_json(step_json)` for a full `ViewStep`.
`movie.add_step_json(step_json)` supports all shared step types, including combined
motion. `movie.trajectory(mol, start, end, fps)` adds a discrete trajectory track. Browser video encoding is not provided.

```javascript
movie.add_step_json(JSON.stringify({
  duration: 2,
  easing: "smooth",
  action: {kind: "move", yaw: 45, zoom: 1.4}
}));
```

## Rust

Public core types are `ViewAction`, `ViewStep`, `ViewPose`, `Easing`, `Movie`,
`TrajectoryTrack`, `PreparedMovie`, `AnimationHandle`, and native `VideoOptions`.
App methods are `view_pose`, `animate_view`, `cancel_animation`, `prepare_movie`,
`prepare_movie_viewport`, `preview_movie`, `seek_movie` and native `render_movie`.
Prepare once and reuse `PreparedMovie` for repeatable seeking. Movie target IDs
are stable molecule/representation IDs, not list indices. `Movie::from_json`
accepts a recipe with `steps` and optional `trajectories`.

Call App methods on the viewer thread through `AppJob`. Wait on handles from a
host thread; waiting on the viewer thread would stop its event loop.
