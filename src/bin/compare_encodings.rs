use std::path::{Path, PathBuf};
use std::{env, fs};

use image::DynamicImage;
use jp2lam::{EncodeOptions, Image, OutputFormat, Preset};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: compare_encodings <image_path> [output_dir]");
        eprintln!();
        eprintln!("Encodes the image at multiple JP2 and JPEG settings and writes");
        eprintln!("one file per variant. JP2 files open in IrfanView, Photoshop, etc.");
        std::process::exit(1);
    }

    let src_path = Path::new(&args[1]);
    let out_dir = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| src_path.parent().unwrap_or(Path::new(".")).to_path_buf());

    let stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");

    let raw = fs::read(src_path).expect("failed to read source image");
    let src_bytes = raw.len();

    let img = image::load_from_memory(&raw).expect("failed to decode source image");
    let (width, height) = (img.width(), img.height());
    let rgb_uncompressed = (width as usize) * (height as usize) * 3;

    println!(
        "Source:  {} ({}x{}, {} bytes on disk, {} bytes raw RGB)",
        src_path.display(),
        width,
        height,
        fmt(src_bytes),
        fmt(rgb_uncompressed),
    );
    println!("Output:  {}", out_dir.display());
    println!();
    println!("{:<32} {:>10} {:>10} {:>10}", "file", "bytes", "vs raw", "vs src");
    println!("{}", "-".repeat(65));

    let variants: &[(&str, Encoding)] = &[
        // -- JP2: all four presets --
        ("webhigh.jp2",      Encoding::Jp2(Preset::WebHigh)),
        ("weblow.jp2",       Encoding::Jp2(Preset::WebLow)),
        ("dochigh.jp2",      Encoding::Jp2(Preset::DocumentHigh)),
        ("doclow.jp2",       Encoding::Jp2(Preset::DocumentLow)),
    ];

    for (suffix, encoding) in variants {
        let filename = format!("{stem}_{suffix}");
        let out_path = out_dir.join(&filename);

        let bytes = encode(&img, encoding);
        let ratio_raw = bytes.len() as f64 / rgb_uncompressed as f64 * 100.0;
        let ratio_src = bytes.len() as f64 / src_bytes as f64 * 100.0;
        fs::write(&out_path, &bytes).expect("failed to write output");

        println!(
            "{:<32} {:>10} {:>9.1}% {:>9.1}%",
            filename,
            fmt(bytes.len()),
            ratio_raw,
            ratio_src,
        );
    }
}

enum Encoding {
    Jp2(Preset),
}

fn encode(img: &DynamicImage, encoding: &Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Jp2(preset) => {
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let jp2_img = Image::from_rgb_bytes(w, h, rgb.as_raw())
                .expect("failed to build jp2lam Image");
            jp2lam::encode(
                &jp2_img,
                &EncodeOptions { preset: *preset, format: OutputFormat::Jp2 },
            )
            .expect("jp2lam encode failed")
        }
    }
}

fn fmt(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}
