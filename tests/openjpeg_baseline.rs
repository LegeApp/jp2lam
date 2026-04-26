/// OpenJPEG size-baseline comparison.
///
/// Compares jp2lam output file sizes against OpenJPEG `opj_compress`.
/// Skipped unless `OPENJP2_OPJ_COMPRESS` env var points to the executable
/// and `OPENJP2_LEAR_PNG` (or `lear.png` at crate root) is present.
///
/// Run manually:
///   OPENJP2_OPJ_COMPRESS=/path/to/opj_compress cargo test --test openjpeg_baseline -- --nocapture
///
/// Expected results:
///   - Lossless: ratio ≤ 1.5× (jp2lam ≤ 1.5× opj_compress size)
///   - Ratios > 2.0× indicate a potential inefficiency worth investigating.
use jp2lam::{encode, ColorSpace, Component, EncodeOptions, Image, OutputFormat, Preset};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn opj_compress_path() -> Option<PathBuf> {
    let path = std::env::var("OPENJP2_OPJ_COMPRESS").ok()?;
    let p = PathBuf::from(&path);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn lear_png_path() -> PathBuf {
    if let Ok(p) = std::env::var("OPENJP2_LEAR_PNG") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lear.png")
}

fn load_lear_rgb() -> Option<(image::RgbImage, u32, u32)> {
    let path = lear_png_path();
    if !path.exists() {
        eprintln!("lear.png not found at {path:?}, skipping");
        return None;
    }
    let img = image::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {path:?}: {e}"))
        .into_rgb8();
    let w = img.width();
    let h = img.height();
    Some((img, w, h))
}

/// Write an 8-bit RGB image as binary PPM (P6) to `path`.
fn write_ppm(path: &PathBuf, img: &image::RgbImage) {
    use std::io::Write;
    let mut f = fs::File::create(path).expect("create ppm");
    writeln!(f, "P6\n{} {}\n255", img.width(), img.height()).expect("ppm header");
    f.write_all(img.as_raw()).expect("ppm body");
}

fn jp2lam_encode_lossless(rgb: &image::RgbImage, w: u32, h: u32) -> Vec<u8> {
    let n = (w * h) as usize;
    let raw = rgb.as_raw();
    let mut r = Vec::with_capacity(n);
    let mut g = Vec::with_capacity(n);
    let mut b = Vec::with_capacity(n);
    for i in 0..n {
        r.push(raw[3 * i] as i32);
        g.push(raw[3 * i + 1] as i32);
        b.push(raw[3 * i + 2] as i32);
    }
    let image = Image {
        width: w,
        height: h,
        components: vec![
            Component { data: r, width: w, height: h, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: g, width: w, height: h, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: b, width: w, height: h, precision: 8, signed: false, dx: 1, dy: 1 },
        ],
        colorspace: ColorSpace::Srgb,
    };
    encode(
        &image,
        &EncodeOptions { preset: Preset::Image, quality: 100, format: OutputFormat::Jp2 },
    )
    .expect("jp2lam lossless encode")
}

fn opj_compress_lossless(opj: &PathBuf, ppm: &PathBuf, out: &PathBuf) -> usize {
    let status = Command::new(opj)
        .args(["-i", ppm.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .expect("run opj_compress");
    assert!(status.success(), "opj_compress failed with status {status}");
    fs::metadata(out).expect("opj output").len() as usize
}

#[test]
fn lossless_rgb_size_vs_openjpeg() {
    let opj = match opj_compress_path() {
        Some(p) => p,
        None => {
            eprintln!("OPENJP2_OPJ_COMPRESS not set — skipping OpenJPEG baseline test");
            return;
        }
    };
    let (rgb, w, h) = match load_lear_rgb() {
        Some(t) => t,
        None => return,
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let ppm_path = tmp.path().join("lear.ppm");
    let opj_out = tmp.path().join("lear_opj.jp2");
    let jp2lam_out = tmp.path().join("lear_jp2lam.jp2");

    write_ppm(&ppm_path, &rgb);
    let opj_size = opj_compress_lossless(&opj, &ppm_path, &opj_out);
    let jp2lam_bytes = jp2lam_encode_lossless(&rgb, w, h);
    fs::write(&jp2lam_out, &jp2lam_bytes).expect("write jp2lam output");

    let jp2lam_size = jp2lam_bytes.len();
    let ratio = jp2lam_size as f64 / opj_size as f64;

    println!("\n=== OpenJPEG Lossless Baseline ({}×{}) ===", w, h);
    println!("  opj_compress  : {:>10} bytes", opj_size);
    println!("  jp2lam        : {:>10} bytes", jp2lam_size);
    println!("  ratio (jp2lam/opj): {:.3}×", ratio);

    if ratio <= 1.2 {
        println!("  Result: EXCELLENT (within 20% of OpenJPEG)");
    } else if ratio <= 1.5 {
        println!("  Result: GOOD (within 50% of OpenJPEG)");
    } else if ratio <= 2.0 {
        println!("  Result: FAIR — investigate further");
    } else {
        println!("  Result: POOR — ratio > 2× suggests a compression bug");
    }

    assert!(
        ratio <= 3.0,
        "jp2lam lossless is {ratio:.2}× OpenJPEG — exceeds 3× threshold, likely a bug"
    );
}
