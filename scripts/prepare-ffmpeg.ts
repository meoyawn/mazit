import { $ } from "bun";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const prefix = resolve(process.env.FFMPEG_DIR || join(root, ".runtime/ffmpeg"));
const cache = join(root, ".cache");
const version = "9.0.2";
const checksum =
  "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e";
const archive = join(cache, `ffmpeg-${version}.tar.xz`);
const source = join(cache, `ffmpeg-${version}`);

await $`mkdir -p ${cache}`;
await $`curl --fail --location --max-filesize 104857600 --output ${archive} https://ffmpeg.org/releases/ffmpeg-${version}.tar.xz`;
const actual = new Bun.CryptoHasher("sha256")
  .update(await Bun.file(archive).arrayBuffer())
  .digest("hex");
if (actual !== checksum) throw new Error("FFmpeg checksum mismatch");
await $`tar -xf ${archive} -C ${cache}`;
const flags = [
  "--disable-everything",
  "--disable-autodetect",
  "--disable-programs",
  "--disable-doc",
  "--disable-debug",
  "--disable-network",
  "--disable-shared",
  "--enable-static",
  "--enable-pic",
  "--disable-avdevice",
  "--disable-avfilter",
  "--disable-swscale",
  "--enable-swresample",
  "--enable-protocol=file",
  "--enable-demuxer=mov,matroska,ogg,aac",
  "--enable-muxer=ipod",
  "--enable-decoder=aac,opus,vorbis",
  "--enable-encoder=aac",
  "--enable-parser=aac,opus,vorbis",
  "--enable-bsf=aac_adtstoasc",
  "--disable-x86asm",
];
await $`./configure --prefix=${prefix} ${flags}`.cwd(source);
await $`make -j8`.cwd(source);
await $`make install`.cwd(source);
await $`cp ${join(source, "COPYING.LGPLv2.1")} ${join(prefix, "COPYING.LGPLv2.1")}`;
