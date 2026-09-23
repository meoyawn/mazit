use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

fn run(command: &mut Command) -> Result<()> {
    if !command.status()?.success() {
        bail!("Build command failed")
    }
    Ok(())
}
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    std::env::set_current_dir(root)?;
    let action = std::env::args().nth(1).unwrap_or("prepare".into());
    if action == "prepare" {
        let prefix = root.join(".runtime/ffmpeg");
        if !prefix.join("lib/libavformat.a").exists() {
            let cache = root.join(".cache");
            fs::create_dir_all(&cache)?;
            let archive = cache.join("ffmpeg-9.0.2.tar.xz");
            let data = ureq::get("https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz")
                .call()?
                .body_mut()
                .with_config()
                .limit(100 * 1024 * 1024)
                .read_to_vec()?;
            anyhow::ensure!(
                format!("{:x}", Sha256::digest(&data))
                    == "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e",
                "FFmpeg checksum mismatch"
            );
            fs::write(&archive, data)?;
            run(Command::new("tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(&cache))?;
            let source = cache.join("ffmpeg-9.0.2");
            run(Command::new("./configure")
                .current_dir(&source)
                .arg(format!("--prefix={}", prefix.display()))
                .args([
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
                ]))?;
            run(Command::new("make").current_dir(&source).args(["-j", "8"]))?;
            run(Command::new("make").current_dir(&source).arg("install"))?;
            fs::copy(
                source.join("COPYING.LGPLv2.1"),
                prefix.join("COPYING.LGPLv2.1"),
            )?;
        }
        run(Command::new("nub").arg("install"))?;
        run(Command::new("nub").args(["run", "bundle"]))?;
    } else if action == "bundle" {
        run(Command::new("cargo").args(["build", "--release", "-p", "mazit"]))?;
        let app = root.join("dist/Mazit.app/Contents");
        fs::create_dir_all(app.join("MacOS"))?;
        fs::create_dir_all(app.join("Resources"))?;
        fs::copy(root.join("target/release/mazit"), app.join("MacOS/Mazit"))?;
        fs::copy(root.join("assets/Info.plist"), app.join("Info.plist"))?;
        fs::copy(
            root.join(".runtime/ffmpeg/COPYING.LGPLv2.1"),
            app.join("Resources/FFmpeg-LICENSE.txt"),
        )?;
        println!("{}", app.parent().context("App path")?.display());
    } else {
        bail!("Use prepare or bundle")
    }
    Ok(())
}
