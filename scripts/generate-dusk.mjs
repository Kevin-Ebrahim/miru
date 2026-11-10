// Build-time only: the application plays these cells natively, without Node.
// Source: bas3line/ascii, commit b9e5af079c99ea83e6154b4d7fa3cb6917b66731.
// The unmodified renderer and its MIT licence are in assets/kyoto-dusk/.
// Regenerate with Node 22.18+: node scripts/generate-dusk.mjs
import { writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';
import create, { meta } from '../assets/kyoto-dusk/source.ts';

const count = 240;
const interval = 120;
const cells = meta.cols * meta.rows;
const palette = meta.palette.flatMap(hex => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16)));
const header = Buffer.alloc(13);
header.write('AWKD');
header.writeUInt16LE(meta.cols, 4);
header.writeUInt16LE(meta.rows, 6);
header.writeUInt16LE(count, 8);
header.writeUInt16LE(interval, 10);
header[12] = meta.palette.length;
const color = new Uint8Array(cells);
const render = create();
const frames = [];
for (let i = 0; i < count; i++) {
  const glyphs = [...render(i * interval / 1000, { color }).replaceAll('\n', '')];
  if (glyphs.length !== cells) throw new Error('Unexpected scene dimensions');
  const frame = Buffer.alloc(cells);
  for (let k = 0; k < cells; k++) {
    const dot = ' ·•●'.indexOf(glyphs[k]);
    if (dot < 0 || color[k] >= palette.length / 3) throw new Error('Unexpected scene cell');
    frame[k] = (dot << 6) | color[k];
  }
  frames.push(frame);
}
// Dissolve moving details into the first frame at the loop boundary. Stationary
// cells stay identical; petals and reflections fade instead of jumping back.
const dissolve = 16;
for (let i = count - dissolve; i < count; i++) {
  const progress = (i - (count - dissolve) + 1) / dissolve;
  for (let k = 0; k < cells; k++) {
    const threshold = ((Math.imul(k + 1, 2654435761) >>> 0) % 1024) / 1024;
    if (threshold < progress) frames[i][k] = frames[0][k];
  }
}
const data = Buffer.concat([header, Buffer.from(palette), ...frames]);
const packed = deflateSync(data, { level: 9 });
writeFileSync(new URL('../assets/kyoto-dusk/frames.zlib', import.meta.url), packed);
console.log(`${count} frames, ${meta.cols} x ${meta.rows}, ${interval} ms, ${packed.length} bytes`);
