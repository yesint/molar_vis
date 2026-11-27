# Embed the viewer in a Rust web application

Add this workspace's `molar_vis_web` crate as a dependency of your browser
application and build for `wasm32-unknown-unknown`. In an external workspace,
include the wasm target flags from the [viewer Cargo configuration](../../.cargo/config.toml).
Cargo reads this configuration from the host workspace. The host and viewer link
into the same WebAssembly module.

Mount a canvas with a nonzero CSS width and height. Pass its
`web_sys::HtmlCanvasElement` to `ViewerHandle::start` after the element enters the
DOM. Keep the handle alive across wizard steps. Dropping it releases the renderer
and allows a new viewer to start.

```rust
use molar_vis_web::{MoleculeHandle, ViewerHandle};
use web_sys::HtmlCanvasElement;

async fn open_ligand(
    canvas: HtmlCanvasElement,
    sdf: Vec<u8>,
) -> Result<(ViewerHandle, MoleculeHandle), String> {
    let viewer = ViewerHandle::start(canvas).await?;
    let ligand = viewer.add_editable("ligand.sdf".into(), sdf).await?;
    ligand.set_editing(true).await?;
    Ok((viewer, ligand))
}
```

Use `add_system` for a parsed, read-only `Rc<molar::prelude::System>`.
Call `representations` to access the initial representation or `add_rep` to
create a styled selection. A representation handle can change its selection,
toggle its visibility, or remove it. Use `viewer.with_app` to queue other
public `molar_vis_core::App` operations, including camera controls.

Subscribe with `ligand.events().await?` and consume the stream with
`futures_util::StreamExt::next`. The first event reports the current editor mode.
Subsequent events identify the molecule whose structure or editor mode changed.
The viewer coalesces changes within one host command or UI frame. Removing a
watched molecule emits `Removed` and closes its stream.

Before replacing an edited ligand, save the result of `ligand.export_cif().await?`.
Then call `viewer.clear().await?` and load the next structure on the same canvas.
Clearing invalidates the old molecule and representation handles.
