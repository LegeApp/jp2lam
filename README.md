# jp2lam

JPEG 2000 Part 1 encoding in Rust, with focused JP2 decoding for document image workflows.

`jp2lam` writes 8-bit grayscale or sRGB images as JP2 files or raw J2K codestreams. It also includes a narrow decoder aimed at the JP2 page images commonly found in Internet Archive book listings, including `_jp2.zip` archives and extracted `*_jp2/` folders.

The current goal is practical document-image interoperability first: a small Rust API, explicit unsupported-feature errors, and code organized around the JPEG 2000 standard rather than OpenJPEG internals.

## Highlights

- Encode 8-bit grayscale or 8-bit sRGB input.
- Write JP2 wrapper files or raw J2K codestreams.
- Decode Internet Archive-style JP2 page images into the crate's native `Image` model.
- Batch encode or decode folders whose images share dimensions, color model, and precision; encoded batches also share quality and format.
- Use one numeric encoder quality value: `0..=100`.
- `quality < 100`: lossy irreversible 9/7 wavelet.
- `quality = 100`: lossless reversible 5/3 wavelet.
- Optional PSNR/SSIM helper through `encode_with_psnr`.
- Optional CLI behind the `cli` feature.

## Install

```toml
[dependencies]
jp2lam = "0.1"
```

Minimum Rust version from the crate manifest:

```text
Rust 1.85+
```

## Quick Start

### Encode RGB To JP2

```rust
use jp2lam::{EncodeOptions, Image, OutputFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let width = 800;
    let height = 600;
    let rgb = vec![128u8; width * height * 3];

    let image = Image::from_rgb_bytes(width as u32, height as u32, &rgb)?;
    let options = EncodeOptions {
        quality: 75,
        format: OutputFormat::Jp2,
    };

    let bytes = jp2lam::encode(&image, &options)?;
    std::fs::write("output.jp2", bytes)?;

    Ok(())
}
```

### Decode JP2

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read("page.jp2")?;
    let image = jp2lam::decode_jp2(&bytes)?;

    println!(
        "{}x{} {:?} components={}",
        image.width,
        image.height,
        image.colorspace,
        image.components.len()
    );

    Ok(())
}
```

### Batch Encode Matching Pages

Use `BatchEncoder` when a folder or page stream comes from the same source and should keep one consistent image profile and encode configuration.

```rust
use jp2lam::{BatchEncoder, EncodeOptions, Image, OutputFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let width = 2550;
    let height = 3300;
    let pages: Vec<Vec<u8>> = load_grayscale_pages();

    let options = EncodeOptions {
        quality: 85,
        format: OutputFormat::Jp2,
    };
    let mut encoder = BatchEncoder::new(options);

    for (idx, page) in pages.iter().enumerate() {
        let image = Image::from_gray_bytes(width, height, page)?;
        let bytes = encoder.encode_one(&image)?;
        std::fs::write(format!("page_{idx:04}.jp2"), bytes)?;
    }

    Ok(())
}

fn load_grayscale_pages() -> Vec<Vec<u8>> {
    todo!("load pages from your own folder or pipeline")
}
```

`BatchDecoder` provides the matching decode path and rejects later items whose decoded image profile does not match the first page.

## Decoder Scope

The decoder is intentionally focused. It currently targets JP2 wrapper files with JPEG 2000 Part 1 codestreams that match the Internet Archive page-image outputs tested during development:

- 8-bit unsigned grayscale or enumerated sRGB JP2 images.
- Single full-image tile.
- LRCP progression.
- Default MQ code-block style.
- No precinct, SOP, or EPH marker syntax.
- Common Archive.org `_jp2.zip` and extracted `*_jp2/` page-image layouts through the CLI.

This is not a universal JPEG 2000 or JPX decoder yet. Palettes, component mapping boxes, per-component bit-depth boxes, ICC color profiles, multiple codestream boxes, unsupported progression orders, and other out-of-scope features fail explicitly.

## Quality Guide

`quality` controls the compression tradeoff. It is not a literal percentage of visual fidelity.

| Quality  | Use case                                                                    |
| -------: | --------------------------------------------------------------------------- |
|  `0..25` | Very small output, previews, stress testing                                 |
| `30..50` | Compact lossy output                                                        |
| `60..85` | Practical range for documents, screenshots, illustrations, and mixed images |
| `90..99` | High-fidelity lossy output                                                  |
|    `100` | Lossless output                                                             |

## Supported Input

Encoder input:

- 8-bit unsigned grayscale.
- 8-bit unsigned sRGB.
- Full-size, non-subsampled components.
- Images at least `2x2`.
- Interleaved RGB input through `Image::from_rgb_bytes`.
- Grayscale input through `Image::from_gray_bytes`.

Decoder input:

- JP2 files accepted by the focused decoder scope above.
- Complete in-memory byte slices through `decode_jp2`.
- Reader-backed input through `decode_from_reader`.

## CLI

The CLI is optional:

```bash
cargo run --release --features cli --bin jp2lam -- input.png
cargo run --release --features cli --bin jp2lam -- encode input.png q75
cargo run --release --features cli --bin jp2lam -- encode-dir pages_png/ pages_jp2/ q85
cargo run --release --features cli --bin jp2lam -- decode page.jp2 page.png
cargo run --release --features cli --bin jp2lam -- decode-dir book_jp2/ book_png/
cargo run --release --features cli --bin jp2lam -- decode-zip book_jp2.zip book_png/
```

The CLI reads normal image files through the optional `image` dependency, writes encoded JP2 output, and writes decoded pages as PNG files. `decode-zip` also walks nested ZIP entries and preserves the archive-relative output layout.

## Batch API

The batch API is for callers handling a sequence of images from one source. It does not require a folder abstraction in the library itself; external programs can walk their own directories and feed each page to `BatchEncoder` or `BatchDecoder`.

`BatchEncoder` checks dimensions, color model, component precision, component sampling, quality, and output format against the first image. `BatchDecoder` checks the decoded image profile against the first decoded page. The current implementation mainly centralizes validation and call shape; it is also the place to add future buffer reuse or shared setup without changing external callers.

Convenience helpers are available when all inputs are already in memory:

```rust
let encoded_pages = jp2lam::encode_batch(images.iter(), &options)?;
let decoded_pages = jp2lam::decode_batch(jp2_streams.iter().map(Vec::as_slice))?;
```

## Metrics

Use `encode_with_psnr` when you want encoded bytes plus internal PSNR/SSIM estimates:

```rust
use jp2lam::{EncodeOptions, Image, OutputFormat};

fn main() -> jp2lam::Result<()> {
    let width = 800;
    let height = 600;
    let gray = vec![240u8; width * height];

    let image = Image::from_gray_bytes(width as u32, height as u32, &gray)?;
    let options = EncodeOptions {
        quality: 75,
        format: OutputFormat::Jp2,
    };

    let (bytes, metrics) = jp2lam::encode_with_psnr(&image, &options)?;

    println!("bytes: {}", bytes.len());
    println!("psnr: {:?}", metrics.psnr_db);
    println!("ssim: {:?}", metrics.ssim);

    Ok(())
}
```

## What It Does Internally

The encoder pipeline is organized around the standard JPEG 2000 stages:

1. Validate image and component metadata.
2. Prepare image samples.
3. Apply color transform where needed.
4. Run reversible 5/3 or irreversible 9/7 DWT.
5. Quantize lossy coefficients.
6. Encode code-block bit-planes with Tier-1 coding.
7. Select truncation points with PCRD.
8. Build packets and packet headers.
9. Write codestream markers.
10. Wrap the codestream in JP2 boxes when requested.

The crate also has an explicit geometry layer for tiles, tile-components, subbands, precincts, and code-blocks. The current encoder uses a single full-image tile, with the geometry kept centralized for the encoder stages that need it.

## Features

Default library build:

```toml
jp2lam = "0.1"
```

Optional CLI:

```toml
jp2lam = { version = "0.1", features = ["cli"] }
```

Available feature flags:

| Feature    | Purpose                                             |
| ---------- | --------------------------------------------------- |
| `cli`      | Enables the command-line tools and image-file input |
| `profile`  | Enables profiling hooks                             |
| `counters` | Exposes internal encoding counters                  |

## Testing

```bash
cargo test
cargo test --features cli
```

## License

Dual-licensed under either:

- MIT
- Apache-2.0
