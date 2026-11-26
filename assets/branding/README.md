# Quby artwork

The approved Quby character has a continuous pastel cube surface, a black rounded face panel, and two ivory pill eyes. `quby.svg` is an editable vector reconstruction of the approved imagegen concept: named paths, a clipping path, and gradients, with no embedded bitmap or font dependency. The gradients approximate the original raster artwork.

| Asset | Purpose |
| --- | --- |
| `quby.svg` | Transparent vector master, retaining the original composition and margins |
| `quby-1024.png` | 1024 × 1024 transparent PNG, with tighter framing for reuse |
| `quby-app-icon.svg` | Editable macOS icon composition, black tile with transparent outer corners and padding |
| `quby-app-icon-1024.png` | 1024 × 1024 desktop icon export |
| `quby-app-icon-transparent.svg` | macOS icon without the tile: the cube alone on transparency, filling the canvas less a 12 pixel margin |
| `quby-app-icon-transparent-1024.png` | 1024 × 1024 export of the transparent icon |
| `../../apps/desktop/assets/QubyTransparent.icns` | The app's icon now: the transparent cube, 16–1024 pixel representations |
| `../../apps/desktop/assets/Quby.icns` | The earlier icon, the cube on the black tile, kept to switch back |
| `../../apps/desktop/assets/Cubie.ico` | The Windows app's icon: the same transparent cube, 16–256 pixel images, embedded in `Cubie.exe` by `apps/desktop/build.rs` |
| `../../apps/web/public/favicon.ico` | Browser ICO with 16, 24, 32, 48, 64, 128, and 256 pixel images |
| `../../apps/web/public/favicon.svg` | Vector favicon, tightly framed so the face remains readable |
| `../../apps/web/public/favicon-32.png` | 32 pixel favicon PNG |

The black face panel stays opaque in the transparent artwork. The desktop tile has a 52 pixel outer inset in a 1024 pixel canvas. The favicon uses the character without the tile and has been visually checked at its exported sizes on light and dark backgrounds.

## Regenerate

From the repository root, using Node and macOS `iconutil`:

```sh
npm install --prefix target/branding-tools --no-save --no-package-lock @resvg/resvg-js@2.6.2
node dev/generate-branding.mjs
```

The renderer is a development-only tool in the ignored `target` directory. No renderer or new library ships with either app. On other platforms, the script regenerates SVG/PNG/ICO files and leaves the checked-in ICNS unchanged. Edit `quby.svg` for character changes; the exporter creates the other artwork from that master.

`ICON` in `deploy/mac/package.sh` picks the icon (`QubyTransparent`, or `Quby` for the black tile): the script copies that ICNS into the app's `Contents/Resources` before signing and writes its name into `Info.plist`. `apps/web/index.html` references both the ICO and SVG favicon, which Vite copies from `public` into `dist`.
