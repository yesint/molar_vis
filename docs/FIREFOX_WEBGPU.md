# Enable WebGPU in Firefox

The [molar_vis browser viewer](https://yesint.github.io/molar_vis/) uses WebGPU
when Firefox provides a compatible GPU adapter. Otherwise, it uses WebGL2, which
supports basic molecular rendering but fewer lighting features.

Firefox may leave WebGPU disabled by default, depending on the operating system
and release channel. Enabling it can make these additional features available:

| Feature | WebGPU | WebGL2 fallback |
| --- | --- | --- |
| Ambient occlusion in the interactive view (SSAO) | Available | Unavailable |
| Cast shadows in the interactive view | Available | Unavailable |
| Viewport ray tracing, including ray-traced AO, soft shadows and optional global illumination | Available on supported devices | Unavailable |

## Enable WebGPU

1. Enter `about:config` in the Firefox **address bar** and press Enter.
2. If Firefox displays a configuration warning, select **Accept the Risk and Continue**.
3. Search for `dom.webgpu.enabled`.
4. If the value is `false`, use the toggle button to set it to **`true`**. If it
   is already `true`, leave it unchanged.
5. Restart Firefox, then reopen the [online viewer](https://yesint.github.io/molar_vis/).

This preference enables the browser API; Firefox must also provide a compatible
GPU adapter. Availability still depends on the Firefox version, operating system,
GPU and graphics driver. Mozilla documents the preference and current platform
support in its [WebGPU feature notes](https://developer.mozilla.org/en-US/docs/Mozilla/Firefox/Experimental_features#webgpu_api).

## Use the additional features

In the viewer settings, open **View → Lighting** and enable **Ambient occlusion**
or **Cast shadows**. To ray-trace a loaded molecule, click the viewport to focus it
and press **R** while no text field is active. The image refines progressively;
changing the camera or scene returns to the interactive view.

Browser **Render → Image…** export currently uses raster rendering even with
WebGPU enabled. The ray-traced viewport does not enable ray-traced PNG export.

## If WebGPU is still unavailable

Open `about:support` in the Firefox address bar and find the **WebGPU Default
Adapter** and **WebGPU Fallback Adapter** entries in the Graphics section. If both
report `"navigator.gpu": null`, Firefox is not exposing the WebGPU API. Check the
preference above and confirm that Firefox restarted.

If the API is enabled but no adapter is available, inspect the **Decision Log**
for WebGPU restrictions and check for Firefox and graphics-driver updates. The
[WebGPU implementation status](https://github.com/gpuweb/gpuweb/wiki/Implementation-Status#firefox)
lists support by operating system and release channel. Firefox Nightly can provide
support on platforms where the regular release still disables WebGPU by default.

The adapter entries report browser availability, not which backend a particular
page selected. After reopening molar_vis, starting a ray-traced view with **R**
confirms that its WebGPU rendering path is active. If WebGPU remains unavailable,
the viewer continues to use WebGL2.
