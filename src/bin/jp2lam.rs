use chrono::Local;
use image::{DynamicImage, GenericImageView};
use jp2lam::{encode, ColorSpace, Component, EncodeOptions, Image, OutputFormat};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    input_path: PathBuf,
    quality: Option<u8>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = parse_args(env::args().skip(1).collect())?;
    if !cli.input_path.exists() {
        return Err(format!("input file not found: {}", cli.input_path.display()));
    }

    let decoded = image::open(&cli.input_path)
        .map_err(|err| format!("failed to open input image: {err}"))?;
    let image = to_jp2lam_image(decoded)?;
    let quality = cli.quality.unwrap_or_else(|| default_quality(image.colorspace));

    let options = EncodeOptions {
        quality,
        format: OutputFormat::Jp2,
    };

    let encoded = encode(&image, &options).map_err(|err| format!("encode failed: {err}"))?;

    let output_dir = PathBuf::from("output");
    fs::create_dir_all(&output_dir)
        .map_err(|err| format!("failed to create output directory: {err}"))?;

    let stem = cli
        .input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image");
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let output_path = output_dir.join(format!("{stem}_q{quality:02}_{timestamp}.jp2"));

    fs::write(&output_path, encoded)
        .map_err(|err| format!("failed to write output file: {err}"))?;

    println!("quality={quality} output={}", output_path.display());
    Ok(())
}

fn default_quality(colorspace: ColorSpace) -> u8 {
    match colorspace {
        ColorSpace::Gray => 85,
        _ => 62,
    }
}

fn parse_args(args: Vec<String>) -> Result<CliArgs, String> {
    let usage = "usage: cargo run --features cli --bin jp2lam -- <input> [q0..q100]";
    let mut args = args.as_slice();
    if let [bin_name, rest @ ..] = args {
        if bin_name.eq_ignore_ascii_case("jp2lam") {
            args = rest;
        }
    }

    let mut input_path: Option<PathBuf> = None;
    let mut quality: Option<u8> = None;
    let mut i = 0usize;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--quality" || arg == "-q" {
            let Some(next) = args.get(i + 1) else {
                return Err(format!("{usage}\nmissing value after {arg}"));
            };
            quality = Some(parse_quality(next)?);
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--quality=") {
            quality = Some(parse_quality(value)?);
            i += 1;
            continue;
        }

        if is_quality_token(arg) {
            quality = Some(parse_quality(arg)?);
        } else if input_path.is_none() {
            input_path = Some(PathBuf::from(arg));
        } else {
            return Err(format!("{usage}\nunrecognized extra argument: {arg}"));
        }
        i += 1;
    }

    let Some(input_path) = input_path else {
        return Err(usage.to_string());
    };

    Ok(CliArgs {
        input_path,
        quality,
    })
}

fn is_quality_token(value: &str) -> bool {
    let trimmed = value.trim();
    let number = trimmed
        .strip_prefix('q')
        .or_else(|| trimmed.strip_prefix('Q'))
        .unwrap_or(trimmed);
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_quality(value: &str) -> Result<u8, String> {
    let trimmed = value.trim();
    let number = trimmed
        .strip_prefix('q')
        .or_else(|| trimmed.strip_prefix('Q'))
        .unwrap_or(trimmed);
    let quality = number
        .parse::<u8>()
        .map_err(|_| format!("invalid quality `{value}`; expected q0..q100"))?;
    if quality > 100 {
        return Err(format!("invalid quality `{value}`; expected q0..q100"));
    }
    Ok(quality)
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

#[cfg(test)]
mod tests {
    use super::{parse_args, parse_quality};
    use std::path::PathBuf;

    #[test]
    fn parses_plain_input_with_default_quality() {
        let args = parse_args(vec!["file.png".to_string()]).expect("args");
        assert_eq!(args.input_path, PathBuf::from("file.png"));
        assert_eq!(args.quality, None);
    }

    #[test]
    fn parses_one_shot_quality_forms() {
        for token in ["q50", "Q50", "50", "--quality=q50"] {
            let args = parse_args(vec!["file.png".to_string(), token.to_string()]).expect(token);
            assert_eq!(args.input_path, PathBuf::from("file.png"));
            assert_eq!(args.quality, Some(50));
        }

        let args = parse_args(vec![
            "jp2lam".to_string(),
            "file.png".to_string(),
            "--quality".to_string(),
            "75".to_string(),
        ])
        .expect("explicit quality");
        assert_eq!(args.input_path, PathBuf::from("file.png"));
        assert_eq!(args.quality, Some(75));
    }

    #[test]
    fn rejects_quality_outside_range() {
        assert!(parse_quality("q101").is_err());
        assert!(parse_quality("qabc").is_err());
    }
}
