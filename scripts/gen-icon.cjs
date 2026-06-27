// Generate a minimal 32x32 blue PNG icon for Tauri build
const fs = require("fs");
const path = require("path");

function createPNG(width, height, r, g, b) {
  // Minimal PNG with uncompressed filter-byte-per-row data for small size
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

  // IHDR
  const ihdrData = Buffer.alloc(13);
  ihdrData.writeUInt32BE(width, 0);
  ihdrData.writeUInt32BE(height, 4);
  ihdrData[8] = 8; // bit depth
  ihdrData[9] = 2; // color type: RGB
  ihdrData[10] = 0; // compression
  ihdrData[11] = 0; // filter
  ihdrData[12] = 0; // interlace
  const ihdr = chunk("IHDR", ihdrData);

  // IDAT - raw pixel data with filter byte (0 = None)
  const raw = Buffer.alloc(height * (1 + width * 3));
  for (let y = 0; y < height; y++) {
    const off = y * (1 + width * 3);
    raw[off] = 0; // filter byte: None
    for (let x = 0; x < width; x++) {
      const px = off + 1 + x * 3;
      raw[px] = r;
      raw[px + 1] = g;
      raw[px + 2] = b;
    }
  }

  const zlib = require("zlib");
  const compressed = zlib.deflateSync(raw);
  const idat = chunk("IDAT", compressed);

  // IEND
  const iend = chunk("IEND", Buffer.alloc(0));

  return Buffer.concat([signature, ihdr, idat, iend]);
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeB = Buffer.from(type, "ascii");
  const crcData = Buffer.concat([typeB, data]);
  const crc = crc32(crcData);
  const crcB = Buffer.alloc(4);
  crcB.writeUInt32BE(crc, 0);
  return Buffer.concat([len, typeB, data, crcB]);
}

function crc32(buf) {
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    crc ^= buf[i];
    for (let j = 0; j < 8; j++) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

const outDir = path.join(__dirname, "..", "src-tauri", "icons");
fs.mkdirSync(outDir, { recursive: true });

// Create 1024x1024 source PNG (Tauri needs at least 1024px for icon generation)
const png = createPNG(1024, 1024, 59, 130, 246);
const sourcePath = path.join(__dirname, "..", "app-icon.png");
fs.writeFileSync(sourcePath, png);
console.log("Created app-icon.png");
