use jp2lam::{encode, ColorSpace, Component, EncodeOptions, Image, OutputFormat, Preset};
use std::fs;
use std::path::PathBuf;

fn lear_png_path() -> PathBuf {
    if let Ok(p) = std::env::var("OPENJP2_LEAR_PNG") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lear.png")
}

fn load_lear_rgb() -> Option<Image> {
    let path = lear_png_path();
    if !path.exists() {
        eprintln!("lear.png not found at {path:?}, skipping test");
        return None;
    }
    let dyn_img = image::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {path:?}: {e}"))
        .into_rgb8();
    let width = dyn_img.width();
    let height = dyn_img.height();
    let n = (width * height) as usize;
    let mut r = Vec::with_capacity(n);
    let mut g = Vec::with_capacity(n);
    let mut b = Vec::with_capacity(n);
    for pixel in dyn_img.pixels() {
        r.push(pixel[0] as i32);
        g.push(pixel[1] as i32);
        b.push(pixel[2] as i32);
    }
    Some(Image {
        width,
        height,
        components: vec![
            Component { data: r, width, height, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: g, width, height, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: b, width, height, precision: 8, signed: false, dx: 1, dy: 1 },
        ],
        colorspace: ColorSpace::Srgb,
    })
}

fn load_lear_gray() -> Option<Image> {
    let path = lear_png_path();
    if !path.exists() {
        eprintln!("lear.png not found at {path:?}, skipping test");
        return None;
    }
    let dyn_img = image::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {path:?}: {e}"))
        .into_luma8();
    let width = dyn_img.width();
    let height = dyn_img.height();
    let n = (width * height) as usize;
    let mut data = Vec::with_capacity(n);
    for pixel in dyn_img.pixels() {
        data.push(pixel[0] as i32);
    }
    Some(Image {
        width,
        height,
        components: vec![Component {
            data,
            width,
            height,
            precision: 8,
            signed: false,
            dx: 1,
            dy: 1,
        }],
        colorspace: ColorSpace::Gray,
    })
}

fn visual_output_dir() -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("visual-output").join("quality-sweep");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn write_jp2(name: &str, bytes: &[u8]) {
    let path = visual_output_dir().join(format!("{name}.jp2"));
    let _ = fs::write(path, bytes);
}

fn is_valid_jp2(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"jP  "
}

/// Verify the lossy quality ladder at 5 representative quality points.
///
/// Points are chosen to cover the full range from the absolute floor (q=1)
/// through heavy lossy (q=10 ≈ JPEG q30), moderate (q=25), balanced (q=50),
/// and near-lossless (q=75), plus lossless (q=100).
///
/// Output JP2 files are written to visual-output/quality-sweep/ for manual
/// inspection. File sizes must be strictly ordered: q=1 < q=10 < q=25 < q=50 < q=75.
#[test]
fn lossy_rgb_quality_ladder_is_valid_and_ordered() {
    let image = match load_lear_rgb() {
        Some(img) => img,
        None => return,
    };
    let qualities: &[u8] = &[1, 10, 25, 50, 75];

    println!("\nlear.png RGB quality sweep ({}×{}):", image.width, image.height);
    let mut sizes = Vec::new();

    for &quality in qualities {
        let options = EncodeOptions { preset: Preset::Image, quality, format: OutputFormat::Jp2 };
        let bytes = encode(&image, &options)
            .unwrap_or_else(|e| panic!("encode failed at quality {quality}: {e}"));

        write_jp2(&format!("lear_rgb_q{quality:03}"), &bytes);

        assert!(
            is_valid_jp2(&bytes),
            "quality {quality}: not a valid JP2 container (len={})",
            bytes.len()
        );
        assert!(
            bytes.len() >= 64,
            "quality {quality}: suspiciously small ({} bytes)",
            bytes.len()
        );

        println!("  quality={quality:3}  size={:8} bytes", bytes.len());
        sizes.push((quality, bytes.len()));
    }

    // Sizes must be strictly increasing across the 5 test points.
    for w in sizes.windows(2) {
        let (q0, s0) = w[0];
        let (q1, s1) = w[1];
        assert!(
            s1 > s0,
            "quality {q0} ({s0} bytes) should be smaller than quality {q1} ({s1} bytes)"
        );
    }

    // Lossless pass.
    let lossless = encode(
        &image,
        &EncodeOptions { preset: Preset::Image, quality: 100, format: OutputFormat::Jp2 },
    )
    .expect("lossless encode failed");
    write_jp2("lear_rgb_q100", &lossless);
    assert!(is_valid_jp2(&lossless), "lossless output is not a valid JP2");
    println!("  quality=100  size={:8} bytes  (lossless 5/3 + RCT)", lossless.len());
}

/// Grayscale quality ladder: same 5-point coverage and ordering checks.
#[test]
fn lossy_gray_quality_ladder_is_valid_and_ordered() {
    let image = match load_lear_gray() {
        Some(img) => img,
        None => return,
    };
    let qualities: &[u8] = &[1, 10, 25, 50, 75];

    println!("\nlear.png Gray quality sweep ({}×{}):", image.width, image.height);
    let mut sizes = Vec::new();

    for &quality in qualities {
        let options =
            EncodeOptions { preset: Preset::Mixed, quality, format: OutputFormat::Jp2 };
        let bytes = encode(&image, &options)
            .unwrap_or_else(|e| panic!("encode failed at quality {quality}: {e}"));

        write_jp2(&format!("lear_gray_q{quality:03}"), &bytes);

        assert!(
            is_valid_jp2(&bytes),
            "gray quality {quality}: not a valid JP2 (len={})",
            bytes.len()
        );
        assert!(
            bytes.len() >= 32,
            "gray quality {quality}: suspiciously small ({} bytes)",
            bytes.len()
        );

        println!("  quality={quality:3}  size={:8} bytes", bytes.len());
        sizes.push((quality, bytes.len()));
    }

    for w in sizes.windows(2) {
        let (q0, s0) = w[0];
        let (q1, s1) = w[1];
        assert!(
            s1 > s0,
            "gray quality {q0} ({s0} bytes) should be smaller than quality {q1} ({s1} bytes)"
        );
    }
}

/// Regression guard: lossless encoding must also produce a valid JP2.
#[test]
fn lossless_rgb_produces_valid_jp2() {
    let image = match load_lear_rgb() {
        Some(img) => img,
        None => return,
    };
    let options = EncodeOptions { preset: Preset::Image, quality: 100, format: OutputFormat::Jp2 };
    let bytes = encode(&image, &options).expect("lossless encode failed");
    write_jp2("lear_rgb_lossless", &bytes);
    assert!(is_valid_jp2(&bytes), "lossless output is not a valid JP2");
    println!(
        "lear.png lossless ({}×{}): {} bytes",
        image.width,
        image.height,
        bytes.len()
    );
}
