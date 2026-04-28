# jp2lam

Lean JPEG 2000 encoding in Rust.

`jp2lam` writes JPEG 2000 Part 1 images as either JP2 files or raw J2K codestreams. It is built for callers that want a small, direct API: pass grayscale or sRGB bytes, choose JP2 or J2K, and set `quality` from `0` to `100`.

The library API exposes encoding, writer-based encoding, metrics-assisted encoding, timing output, error types, image models, options, formats, and presets from the crate root. :contentReference[oaicite:1]{index=1}

## Highlights

- Encode 8-bit grayscale or 8-bit sRGB input.
- Write JP2 wrapper files or raw J2K codestreams.
- Use one numeric quality value: `0..=100`.
- `quality < 100`: lossy irreversible 9/7 wavelet.
- `quality = 100`: lossless reversible 5/3 wavelet.
- Optional PSNR/SSIM helper through `encode_with_psnr`.
- Optional CLI behind the `cli` feature.
- Library mode avoids the CLI image-loading stack; `image` and `chrono` are only enabled by `cli`. :contentReference[oaicite:2]{index=2}

## Install

```toml
[dependencies]
jp2lam = "0.1"
````

Minimum Rust version from the crate manifest:

```text
Rust 1.85+
```

## Quick start

### Encode RGB to JP2

```rust
use jp2lam::{EncodeOptions, Image, OutputFormat};

fn main() -> jp2lam::Result<()> {
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

### Encode grayscale to raw J2K

```rust
use jp2lam::{EncodeOptions, Image, OutputFormat};

fn main() -> jp2lam::Result<()> {
    let width = 800;
    let height = 600;
    let gray = vec![240u8; width * height];

    let image = Image::from_gray_bytes(width as u32, height as u32, &gray)?;

    let options = EncodeOptions {
        quality: 85,
        format: OutputFormat::J2k,
    };

    let bytes = jp2lam::encode(&image, &options)?;
    std::fs::write("page.j2k", bytes)?;

    Ok(())
}
```

## Quality guide

`quality` controls the compression tradeoff. It is not a literal percentage of visual fidelity.

|  Quality | Use case                                                                    |
| -------: | --------------------------------------------------------------------------- |
|  `0..25` | Very small output, previews, stress testing                                 |
| `30..50` | Compact lossy output                                                        |
| `60..85` | Practical range for documents, screenshots, illustrations, and mixed images |
| `90..99` | High-fidelity lossy output                                                  |
|    `100` | Lossless output                                                             |

## Supported input

* 8-bit unsigned grayscale.
* 8-bit unsigned sRGB.
* Full-size, non-subsampled components.
* Images at least `2x2`.
* Interleaved RGB input through `Image::from_rgb_bytes`.
* Grayscale input through `Image::from_gray_bytes`.

## CLI

The CLI is optional:

```bash
cargo run --release --features cli --bin jp2lam -- input.png
cargo run --release --features cli --bin jp2lam -- input.png q50
cargo run --release --features cli --bin jp2lam -- input.png -q 75
cargo run --release --features cli --bin jp2lam -- input.png --quality=q95
```

The CLI reads image files through the optional `image` dependency and writes JP2 output under `output/`.

## Compare quality levels

The `compare_encodings` helper encodes one input at several quality levels and reports size and quality metrics:

```bash
cargo run --release --features cli --bin compare_encodings -- input.png
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

    let result = jp2lam::encode_with_psnr(&image, &options)?;

    println!("bytes: {}", result.bytes.len());
    println!("psnr: {:?}", result.psnr);
    println!("ssim: {:?}", result.ssim);

    Ok(())
}
```

Adjust field names above if your current `EncodeMetrics` return shape differs.

## What it does internally

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
```

## License

Dual-licensed under either:

* MIT
* Apache-2.0

```
```
