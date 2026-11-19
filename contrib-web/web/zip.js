// Reading the conversations out of a data export (.zip) in the browser, without a library:
// the central directory says where each file is, and DecompressionStream inflates it. Handles
// ZIP64, which exports with many images use. Only the files `want(name)` accepts are read.

const SIG_EOCD = 0x06054b50;
const SIG_EOCD64_LOCATOR = 0x07064b50;
const SIG_EOCD64 = 0x06064b50;
const SIG_CENTRAL = 0x02014b50;
const SIG_LOCAL = 0x04034b50;

async function bytes(blob, start, end) {
  return new DataView(await blob.slice(start, end).arrayBuffer());
}

function u64(view, at) {
  return view.getUint32(at, true) + view.getUint32(at + 4, true) * 2 ** 32;
}

export async function readZip(blob, want) {
  // The end of central directory record is in the last 22 bytes plus a comment of up to 64 KiB.
  const tailStart = Math.max(0, blob.size - 22 - 65535);
  const tail = await bytes(blob, tailStart, blob.size);
  let eocd = -1;
  for (let i = tail.byteLength - 22; i >= 0; i--) {
    if (tail.getUint32(i, true) === SIG_EOCD) { eocd = i; break; }
  }
  if (eocd < 0) throw new Error('This is not a zip file.');
  let cdSize = tail.getUint32(eocd + 12, true);
  let cdOffset = tail.getUint32(eocd + 16, true);
  if (cdOffset === 0xffffffff || cdSize === 0xffffffff) {
    const loc = eocd - 20;
    if (loc < 0 || tail.getUint32(loc, true) !== SIG_EOCD64_LOCATOR) throw new Error('This zip file is damaged.');
    const at = u64(tail, loc + 8);
    const rec = await bytes(blob, at, at + 56);
    if (rec.getUint32(0, true) !== SIG_EOCD64) throw new Error('This zip file is damaged.');
    cdSize = u64(rec, 40);
    cdOffset = u64(rec, 48);
  }
  const cd = await bytes(blob, cdOffset, cdOffset + cdSize);
  const names = new TextDecoder();
  const out = [];
  for (let p = 0; p + 46 <= cd.byteLength && cd.getUint32(p, true) === SIG_CENTRAL;) {
    const method = cd.getUint16(p + 10, true);
    let compSize = cd.getUint32(p + 20, true);
    let size = cd.getUint32(p + 24, true);
    const nameLen = cd.getUint16(p + 28, true);
    const extraLen = cd.getUint16(p + 30, true);
    const commentLen = cd.getUint16(p + 32, true);
    let local = cd.getUint32(p + 42, true);
    const name = names.decode(new Uint8Array(cd.buffer, cd.byteOffset + p + 46, nameLen));
    // ZIP64 sizes and offset live in an extra field, in this order, only where the 32-bit field is full.
    for (let e = p + 46 + nameLen; e + 4 <= p + 46 + nameLen + extraLen;) {
      const id = cd.getUint16(e, true);
      const len = cd.getUint16(e + 2, true);
      if (id === 0x0001) {
        let q = e + 4;
        if (size === 0xffffffff) { size = u64(cd, q); q += 8; }
        if (compSize === 0xffffffff) { compSize = u64(cd, q); q += 8; }
        if (local === 0xffffffff) { local = u64(cd, q); }
      }
      e += 4 + len;
    }
    p += 46 + nameLen + extraLen + commentLen;
    if (name.endsWith('/') || !want(name)) continue;
    const head = await bytes(blob, local, local + 30);
    if (head.getUint32(0, true) !== SIG_LOCAL) throw new Error(`The zip entry ${name} is damaged.`);
    const start = local + 30 + head.getUint16(26, true) + head.getUint16(28, true);
    const data = blob.slice(start, start + compSize);
    let text;
    if (method === 0) {
      text = await data.text();
    } else if (method === 8) {
      text = await new Response(data.stream().pipeThrough(new DecompressionStream('deflate-raw'))).text();
    } else {
      throw new Error(`${name} is compressed in a way this page can't read (method ${method}).`);
    }
    out.push({ name, text, size });
  }
  return out;
}
