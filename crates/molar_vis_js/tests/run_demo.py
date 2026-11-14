"""Check the actual Pages demo, including rendered pixels and startup failures.

Requires playwright, Pillow, and Chromium. Use --url to check a deployed demo,
or --site to serve an assembled Pages artifact locally.
"""

import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from io import BytesIO
from pathlib import Path
import threading

from PIL import Image
from playwright.sync_api import sync_playwright


def check_demo(browser, url, screenshot):
    page = browser.new_page(viewport={"width": 1200, "height": 800})
    errors = []
    page.route("**/favicon.ico", lambda route: route.fulfill(status=204))
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("console", lambda message: errors.append(message.text) if message.type == "error" else None)
    # Exercise the fallback even on machines that have a working WebGPU adapter.
    page.add_init_script("Object.defineProperty(Navigator.prototype, 'gpu', {get: () => undefined})")
    page.goto(url)
    page.wait_for_function("(window.vis && window.sys) || document.querySelector('#loading[role=alert]')", timeout=90000)
    assert not errors, errors
    page.locator("#loading").wait_for(state="detached")
    counts = page.evaluate("""async () => ({
        atoms: sys.numAtoms,
        molecules: (await vis.mols()).length,
        representations: (await mol.reps()).length,
        controls: JSON.parse(await vis.ui_visibility_json()),
    })""")
    assert counts == {
        "atoms": 1911, "molecules": 1, "representations": 2,
        "controls": {"selection_panel": True, "selection_toolbar": True},
    }, counts
    # Let the frame containing the queried scene reach the canvas.
    page.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
    pixels = Image.open(BytesIO(page.locator("canvas").screenshot(path=str(screenshot)))).convert("RGB")
    width, height = pixels.size
    viewport = pixels.crop((int(width * .4), int(height * .15), int(width * .9), int(height * .85)))
    colorful = sum(count for count, rgb in viewport.getcolors(viewport.width * viewport.height)
                   if max(rgb) - min(rgb) > 40 and max(rgb) > 80)
    assert colorful > 100, f"No colored molecule in the viewport ({colorful} pixels)"
    assert not errors, errors
    page.close()
    print("PASS: WebGL2 demo renders 1911 atoms, two representations, and visible controls")


def check_errors(browser, url):
    for failure in ("module", "structure", "graphics"):
        page = browser.new_page()
        if failure == "module":
            page.route("**/pkg/molar_vis.js", lambda route: route.abort())
        elif failure == "structure":
            page.route("**/2lao.pdb", lambda route: route.fulfill(status=503, body="Unavailable"))
        else:
            page.add_init_script("""
                Object.defineProperty(Navigator.prototype, 'gpu', {get: () => undefined});
                const getContext = HTMLCanvasElement.prototype.getContext;
                HTMLCanvasElement.prototype.getContext = function(kind, ...args) {
                    return kind.startsWith('webgl') ? null : getContext.call(this, kind, ...args);
                };
            """)
        page.goto(url)
        page.wait_for_function("document.querySelector('#loading[role=alert]')?.textContent.includes('failed to start')", timeout=90000)
        assert page.locator("#loading").is_visible()
        if failure == "structure":
            assert "HTTP 503" in page.locator("#loading").inner_text()
        page.close()
        print(f"PASS: {failure} failure stays visible in the page")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--site", type=Path)
    source.add_argument("--url")
    parser.add_argument("--browser", help="Chromium executable; defaults to Playwright's Chromium")
    parser.add_argument("--screenshot", type=Path, default=Path("/tmp/molar-demo.png"))
    parser.add_argument("--check-errors", action="store_true", help="Also simulate startup failures")
    args = parser.parse_args()
    server = None
    if args.site:
        handler = partial(SimpleHTTPRequestHandler, directory=str(args.site.resolve()))
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{server.server_port}/"
    else:
        url = args.url
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(executable_path=args.browser, args=[
                "--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
            ])
            try:
                check_demo(browser, url, args.screenshot)
                if args.check_errors:
                    check_errors(browser, url)
            finally:
                browser.close()
    finally:
        if server:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
