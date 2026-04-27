use chrono::Local;
use image::{DynamicImage, GenericImageView};
use jp2lam::{encode, ColorSpace, Component, EncodeOptions, Image, OutputFormat};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let input_path = parse_args(env::args().skip(1).collect())?;
    if !input_path.exists() {
        return Err(format!("input file not found: {}", input_path.display()));
    }

    let decoded =
        image::open(&input_path).map_err(|err| format!("failed to open PNG image: {err}"))?;
    let image = to_jp2lam_image(decoded)?;

    let options = EncodeOptions {
        quality: match image.colorspace {
            ColorSpace::Gray => 85,
            _ => 62,
        },
        format: OutputFormat::Jp2,
    };

    let encoded = encode(&image, &options).map_err(|err| format!("encode failed: {err}"))?;

    let output_dir = PathBuf::from("output");
    fs::create_dir_all(&output_dir)
        .map_err(|err| format!("failed to create output directory: {err}"))?;

    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image");
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let output_path = output_dir.join(format!("{stem}_{timestamp}.jp2"));

    fs::write(&output_path, encoded)
        .map_err(|err| format!("failed to write output file: {err}"))?;

    println!("{}", output_path.display());
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<PathBuf, String> {
    let usage = "usage: cargo run --features cli --bin jp2lam -- <input.png>";
    match args.as_slice() {
        [bin_name, input] if bin_name.eq_ignore_ascii_case("jp2lam") => {
            Ok(PathBuf::from(input))
        }
        [input] => Ok(PathBuf::from(input)),
        _ => Err(usage.to_string()),
    }
}

fn to_jp2lam_image(decoded: DynamicImage) -> Result<Image, String> {
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 {
        return Err("input image has zero width or height".to_string());
    }

    let channels = decoded.color().channel_count();
    if channels <= 2 {
        let gray = decoded.to_luma8();
        let data = gray.as_raw().iter().map(|&v| i32::from(v)).collect();
        Ok(Image {
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
    } else {
        let rgb = decoded.to_rgb8();
        let mut r = Vec::with_capacity((width * height) as usize);
        let mut g = Vec::with_capacity((width * height) as usize);
        let mut b = Vec::with_capacity((width * height) as usize);
        for px in rgb.as_raw().chunks_exact(3) {
            r.push(i32::from(px[0]));
            g.push(i32::from(px[1]));
            b.push(i32::from(px[2]));
        }
        Ok(Image {
            width,
            height,
            components: vec![
                component_from_u8_plane(r, width, height),
                component_from_u8_plane(g, width, height),
                component_from_u8_plane(b, width, height),
            ],
            colorspace: ColorSpace::Srgb,
        })
    }
}

fn component_from_u8_plane(data: Vec<i32>, width: u32, height: u32) -> Component {
    Component {
        data,
        width,
        height,
        precision: 8,
        signed: false,
        dx: 1,
        dy: 1,
    }
}
