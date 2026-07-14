import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { inflateSync } from "node:zlib";

const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const REQUIRED_ICO_SIZES = [16, 24, 32, 48, 64, 256];

function fail(message) {
  throw new Error(message);
}

function read(path) {
  try {
    return readFileSync(resolve(path));
  } catch (error) {
    fail(`Unable to read ${path}: ${error.message}`);
  }
}

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

function paeth(left, above, upperLeft) {
  const estimate = left + above - upperLeft;
  const leftDistance = Math.abs(estimate - left);
  const aboveDistance = Math.abs(estimate - above);
  const upperLeftDistance = Math.abs(estimate - upperLeft);
  if (leftDistance <= aboveDistance && leftDistance <= upperLeftDistance) return left;
  if (aboveDistance <= upperLeftDistance) return above;
  return upperLeft;
}

function parsePng(buffer, label) {
  if (!buffer.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) {
    fail(`${label} is not a PNG image.`);
  }

  let offset = PNG_SIGNATURE.length;
  let header;
  const compressed = [];
  while (offset + 12 <= buffer.length) {
    const length = buffer.readUInt32BE(offset);
    const type = buffer.toString("ascii", offset + 4, offset + 8);
    const dataStart = offset + 8;
    const dataEnd = dataStart + length;
    if (dataEnd + 4 > buffer.length) fail(`${label} has a truncated ${type} chunk.`);
    const data = buffer.subarray(dataStart, dataEnd);
    if (type === "IHDR") {
      header = {
        width: data.readUInt32BE(0),
        height: data.readUInt32BE(4),
        bitDepth: data[8],
        colorType: data[9],
        compression: data[10],
        filter: data[11],
        interlace: data[12],
      };
    } else if (type === "IDAT") {
      compressed.push(data);
    } else if (type === "IEND") {
      break;
    }
    offset = dataEnd + 4;
  }

  if (!header) fail(`${label} is missing an IHDR chunk.`);
  if (header.bitDepth !== 8 || ![2, 6].includes(header.colorType)) {
    fail(`${label} must be an 8-bit RGB or RGBA PNG.`);
  }
  if (header.compression !== 0 || header.filter !== 0 || header.interlace !== 0) {
    fail(`${label} uses unsupported PNG encoding settings.`);
  }
  if (!compressed.length) fail(`${label} is missing pixel data.`);

  const channels = header.colorType === 6 ? 4 : 3;
  const stride = header.width * channels;
  const inflated = inflateSync(Buffer.concat(compressed));
  const expectedLength = header.height * (stride + 1);
  if (inflated.length !== expectedLength) {
    fail(`${label} has an unexpected decompressed pixel length.`);
  }

  const pixels = Buffer.alloc(header.height * stride);
  for (let row = 0; row < header.height; row += 1) {
    const sourceOffset = row * (stride + 1);
    const filterType = inflated[sourceOffset];
    const rowOffset = row * stride;
    for (let column = 0; column < stride; column += 1) {
      const raw = inflated[sourceOffset + 1 + column];
      const left = column >= channels ? pixels[rowOffset + column - channels] : 0;
      const above = row > 0 ? pixels[rowOffset + column - stride] : 0;
      const upperLeft = row > 0 && column >= channels
        ? pixels[rowOffset + column - stride - channels]
        : 0;
      let value;
      switch (filterType) {
        case 0:
          value = raw;
          break;
        case 1:
          value = raw + left;
          break;
        case 2:
          value = raw + above;
          break;
        case 3:
          value = raw + Math.floor((left + above) / 2);
          break;
        case 4:
          value = raw + paeth(left, above, upperLeft);
          break;
        default:
          fail(`${label} uses unknown PNG filter ${filterType}.`);
      }
      pixels[rowOffset + column] = value & 0xff;
    }
  }

  return { ...header, channels, pixels };
}

function pixelAt(png, x, y) {
  const offset = (y * png.width + x) * png.channels;
  return [png.pixels[offset], png.pixels[offset + 1], png.pixels[offset + 2]];
}

function inspectMark(png, label) {
  let minLuminance = 255;
  let maxLuminance = 0;
  let brightMagentaPixels = 0;
  for (let offset = 0; offset < png.pixels.length; offset += png.channels) {
    const red = png.pixels[offset];
    const green = png.pixels[offset + 1];
    const blue = png.pixels[offset + 2];
    const luminance = Math.round(0.2126 * red + 0.7152 * green + 0.0722 * blue);
    minLuminance = Math.min(minLuminance, luminance);
    maxLuminance = Math.max(maxLuminance, luminance);
    if (red > 200 && blue > 200 && green < 80) brightMagentaPixels += 1;
  }
  if (brightMagentaPixels > 0) fail(`${label} contains ${brightMagentaPixels} bright-magenta pixels.`);
  if (maxLuminance - minLuminance < 120) fail(`${label} lacks enough contrast for the T monogram.`);

  const corners = [
    pixelAt(png, 0, 0),
    pixelAt(png, png.width - 1, 0),
    pixelAt(png, 0, png.height - 1),
    pixelAt(png, png.width - 1, png.height - 1),
  ];
  if (corners.some(([red, green, blue]) => Math.max(red, green, blue) > 90)) {
    fail(`${label} must keep a charcoal field in every canvas corner.`);
  }
}

function verifyPng(path, width, height, requireAlpha) {
  const png = parsePng(read(path), path);
  if (png.width !== width || png.height !== height) {
    fail(`${path} must be ${width}x${height}; got ${png.width}x${png.height}.`);
  }
  if (requireAlpha && png.colorType !== 6) fail(`${path} must include an RGBA channel.`);
  inspectMark(png, path);
  return png;
}

function verifyIco(path) {
  const buffer = read(path);
  if (buffer.length < 6) fail(`${path} is too short to be an ICO file.`);
  if (buffer.readUInt16LE(0) !== 0 || buffer.readUInt16LE(2) !== 1) {
    fail(`${path} has an invalid ICO header.`);
  }
  const count = buffer.readUInt16LE(4);
  if (buffer.length < 6 + count * 16) fail(`${path} has a truncated directory.`);

  const entries = [];
  for (let index = 0; index < count; index += 1) {
    const offset = 6 + index * 16;
    const width = buffer[offset] || 256;
    const height = buffer[offset + 1] || 256;
    const bitCount = buffer.readUInt16LE(offset + 6);
    const byteLength = buffer.readUInt32LE(offset + 8);
    const imageOffset = buffer.readUInt32LE(offset + 12);
    if (imageOffset + byteLength > buffer.length) fail(`${path} has an out-of-bounds ${width}px entry.`);
    if (width !== height) fail(`${path} contains a non-square ${width}x${height} entry.`);
    if (bitCount !== 32) fail(`${path} ${width}px entry must be 32-bit RGBA.`);
    const image = buffer.subarray(imageOffset, imageOffset + byteLength);
    const png = parsePng(image, `${path} ${width}px entry`);
    if (png.width !== width || png.height !== height || png.colorType !== 6) {
      fail(`${path} ${width}px entry must be a matching RGBA PNG.`);
    }
    if (width === 16 || width === 32) inspectMark(png, `${path} ${width}px entry`);
    entries.push(width);
  }

  for (const size of REQUIRED_ICO_SIZES) {
    if (!entries.includes(size)) fail(`${path} is missing the ${size}px entry.`);
  }
  return entries.sort((left, right) => left - right);
}

const source = read("assets/brand/wuwa-ini-tool-mark.png");
const publicAsset = read("public/branding/wuwa-ini-tool-mark.png");
if (sha256(source) !== sha256(publicAsset)) {
  fail("The public brand asset must be byte-identical to the production source.");
}

verifyPng("assets/brand/wuwa-ini-tool-mark.png", 1024, 1024, false);
verifyPng("public/branding/wuwa-ini-tool-mark.png", 1024, 1024, false);
verifyPng("src-tauri/icons/32x32.png", 32, 32, true);
verifyPng("src-tauri/icons/64x64.png", 64, 64, true);
verifyPng("src-tauri/icons/128x128.png", 128, 128, true);
verifyPng("src-tauri/icons/128x128@2x.png", 256, 256, true);
verifyPng("src-tauri/icons/icon.png", 512, 512, true);
const icoSizes = verifyIco("src-tauri/icons/icon.ico");

console.log(`Icon verification passed: PNG exports valid; ICO sizes ${icoSizes.join(", ")}px.`);
