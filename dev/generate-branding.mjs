#!/usr/bin/env node
// Development-only exporter; no additional app/runtime dependencies.
// npm install --prefix target/branding-tools --no-save --no-package-lock @resvg/resvg-js@2.6.2
// node dev/generate-branding.mjs
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(join(root, 'target/branding-tools/package.json'));
const { Resvg } = require('@resvg/resvg-js');
const branding = join(root, 'assets/branding');
const web = join(root, 'apps/web/public');
const desktop = join(root, 'apps/desktop/assets');
for (const dir of [branding, web, desktop]) mkdirSync(dir, { recursive: true });
const source = readFileSync(join(branding, 'quby.svg'), 'utf8');

function render(svg, size) {
  return new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render().asPng();
}

// An SVG export remains completely editable: paths and gradients, no bitmap.
// Export a comfortably framed standalone character on real transparent alpha.
const logo = source.replace('viewBox="0 0 1254 1254"', 'viewBox="165 165 924 924"');
writeFileSync(join(branding, 'quby-1024.png'), render(logo, 1024));

// The native macOS tile has transparent corners AND an outer safety margin.
// Reuse the same character; the cube occupies the same fraction of the tile
// as in the approved artwork. No sharp edges between its pastel faces.
const definitions = source.match(/<defs>([\s\S]*?)<\/defs>/)[1].trim();
const character = source.match(/  <g id="quby-character">([\s\S]*)<\/svg>/)[0].replace(/<\/svg>\s*$/, '').trim();
const icon = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" role="img" aria-label="Quby app icon">
  <defs>${definitions}
    <linearGradient id="tile-fill" x1="0" y1="0" x2="0.65" y2="1">
      <stop stop-color="#1b1c1d"/><stop offset="1" stop-color="#030303"/>
    </linearGradient>
    <linearGradient id="tile-rim" x1="0" y1="0" x2="0" y2="1">
      <stop stop-color="#525354"/><stop offset="1" stop-color="#272829"/>
    </linearGradient>
  </defs>
  <path d="M282 52H742C908 52 972 116 972 282V742C972 908 908 972 742 972H282C116 972 52 908 52 742V282C52 116 116 52 282 52Z" fill="url(#tile-fill)" stroke="url(#tile-rim)" stroke-width="3"/>
  <g transform="translate(52 52) scale(${920 / 1254})">${character}</g>
</svg>\n`;
writeFileSync(join(branding, 'quby-app-icon.svg'), icon);
writeFileSync(join(branding, 'quby-app-icon-1024.png'), render(icon, 1024));

// The macOS icon without the tile: only the cube, on transparency, scaled
// until its taller side fills the canvas less a thin margin for antialiasing.
// The square viewBox is centred on the character's measured bounds.
const MARGIN = 12;
const bounds = new Resvg(source).getBBox();
const side = (Math.max(bounds.width, bounds.height) * 1024) / (1024 - 2 * MARGIN);
const round = n => Number(n.toFixed(2));
const cube = source.replace(
  'viewBox="0 0 1254 1254"',
  `viewBox="${round(bounds.x + bounds.width / 2 - side / 2)} ${round(bounds.y + bounds.height / 2 - side / 2)} ${round(side)} ${round(side)}"`,
);
writeFileSync(join(branding, 'quby-app-icon-transparent.svg'), cube);
writeFileSync(join(branding, 'quby-app-icon-transparent-1024.png'), render(cube, 1024));

// Favicons use the full character without the black tile; tighter framing
// keeps its eyes visible at 16/32 pixels on both light and dark browser tabs.
const favicon = source
  .replace('width="1024" height="1024"', 'width="64" height="64"')
  .replace('viewBox="0 0 1254 1254"', 'viewBox="207 207 840 840"');
writeFileSync(join(web, 'favicon.svg'), favicon);
writeFileSync(join(web, 'favicon-32.png'), render(favicon, 32));

// ICO directory plus PNG-encoded images: sizes used by browsers and Windows.
const sizes = [16, 24, 32, 48, 64, 128, 256];
const images = sizes.map(size => render(favicon, size));
const header = Buffer.alloc(6 + sizes.length * 16);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(sizes.length, 4);
let offset = header.length;
for (let i = 0; i < sizes.length; i++) {
  const at = 6 + i * 16;
  header[at] = header[at + 1] = sizes[i] === 256 ? 0 : sizes[i];
  header.writeUInt16LE(1, at + 4);
  header.writeUInt16LE(32, at + 6);
  header.writeUInt32LE(images[i].length, at + 8);
  header.writeUInt32LE(offset, at + 12);
  offset += images[i].length;
}
writeFileSync(join(web, 'favicon.ico'), Buffer.concat([header, ...images]));

function icns(svg, name) {
  const temp = mkdtempSync(join(tmpdir(), 'quby-icons-'));
  try {
    const iconset = join(temp, `${name}.iconset`);
    mkdirSync(iconset);
    for (const size of [16, 32, 128, 256, 512]) {
      for (const scale of [1, 2]) {
        const suffix = scale === 2 ? '@2x' : '';
        writeFileSync(join(iconset, `icon_${size}x${size}${suffix}.png`), render(svg, size * scale));
      }
    }
    execFileSync('iconutil', ['--convert', 'icns', '--output', join(desktop, `${name}.icns`), iconset]);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

if (process.platform === 'darwin') {
  icns(icon, 'Quby');
  icns(cube, 'QubyTransparent');
} else {
  console.warn('macOS is required to regenerate Quby.icns and QubyTransparent.icns; other exports are ready.');
}
console.log('Exported Quby PNG/SVG artwork, web favicons, and macOS icon (on macOS).');
