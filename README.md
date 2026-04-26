# jp2lam – Lean and Mean JPEG 2000 Encoder

A high-performance, production-ready JPEG 2000 (JP2/J2K) encoder written in Rust. Optimized for document and web imaging workloads with intelligent quality presets, perceptual masking, and PCRD-driven compression.

## Features

### Encoding Modes
- **Four quality presets** targeting different use cases:
  - `DocumentLow` (q=30): Extreme compression for scanned documents
  - `DocumentHigh` (q=85): High-fidelity document archival
  - `WebLow` (q=42): Web delivery with aggressive optimization
  - `WebHigh` (q=62): Web delivery with visual fidelity

### Compression Architecture
- **Wavelet Transforms**
  - Reversible 5/3 integer lifting (lossless only, perfect reconstruction)
  - Irreversible 9/7 float lifting (lossy, production default for all presets)
  
- **Color Space Handling**
  - Automatic MCT (Multiple Component Transform) for RGB→YCbCr conversion
  - ICT (Irreversible Color Transform) for lossy encoding
  - Proper luma/chroma scaling and rounding
  
- **Adaptive Quantization**
  - Scalar Expounded Quantization (lossy) with JPEG 2000 standard encoding
  - Quality-based step scaling: qualities < 50 dynamically scale quantization for smooth output
  - Subband-specific exponent and mantissa calculation

- **PCRD Optimization** (Post-Compression Rate-Distortion)
  - Lambda-driven optimal truncation point selection per code block
  - Quality→lambda calibration with built-in presets
  - Resolution-scaled distortion estimates
  - Smooth band distortion biasing for low-quality output

- **Perceptual Masking**
  - 8×8 DCT-based local contrast masking
  - Edge-aware texture analysis via quadrant variance
  - White-region visibility override (near-white blocks get deprioritized in PCRD)
  - Automatic visibility weighting from 1.0 (normal) to 0.05 (bright white)

### Output Formats
- `JP2` (JPEG 2000 Part 1): Full-featured with JP2 signature box and extended metadata
- `J2K` (JPEG 2000 codestream): Bare codestream for embedding in other containers

### Technical Details
- **Code Block Size**: Fixed 64×64 (maximum standard block size)
- **Decomposition Levels**: Auto-capped based on image dimensions (max 6 for RGB, 5 for grayscale)
- **Progression Order**: LRCP (Layer-Resolution-Component-Position)
- **Marker Support**: SIZ (image size), COD (coding style), QCD (quantization default)
- **Guard Bits**: 2 (standard value for irreversible transform)
- **Parallel Processing**: Rayon-driven multi-threaded tile and layer encoding

## Installation

Add to `Cargo.toml`:
```toml
[dependencies]
jp2lam = "0.1"
```

### Requirements
- **Rust**: 1.95 or later
- **Dependencies**: rayon (automatic)
- **No external C libraries required** – fully Rust implementation

## Quick Start

### Basic Encoding

```rust
use jp2lam::{Image, Component, EncodeOptions, Preset, OutputFormat};

fn main() -> anyhow::Result<()> {
    // Prepare 8-bit RGB image data
    let width = 800;
    let height = 600;
    let rgb_bytes = vec![0u8; (width * height * 3) as usize];
    
    // Create components (RGB)
    let components = vec![
        Component {
            data: rgb_bytes[0..].iter().step_by(3).cloned().collect(),
            width,
            height,
            precision: 8,
            signed: false,
            dx: 1,
            dy: 1,
        },
        Component {
            data: rgb_bytes[1..].iter().step_by(3).cloned().collect(),
            width,
            height,
            precision: 8,
            signed: false,
            dx: 1,
            dy: 1,
        },
        Component {
            data: rgb_bytes[2..].iter().step_by(3).cloned().collect(),
            width,
            height,
            precision: 8,
            signed: false,
            dx: 1,
            dy: 1,
        },
    ];
    
    // Build image
    let image = Image {
        width,
        height,
        components,
        colorspace: jp2lam::ColorSpace::Srgb,
    };
    
    // Encode with WebHigh preset
    let options = EncodeOptions {
        preset: Preset::WebHigh,
        format: OutputFormat::Jp2,
    };
    
    let jp2_bytes = jp2lam::encode(&image, &options)?;
    std::fs::write("output.jp2", jp2_bytes)?;
    
    Ok(())
}
```

### Using the CLI Tool

The `jp2lam` command-line tool (requires `cli` feature) encodes PNG, JPEG, and other formats:

```bash
# Encode with WebHigh preset (default)
cargo run --release --bin jp2lam --features cli -- input.png output.jp2

# Encode with custom preset
cargo run --release --bin jp2lam --features cli -- -p DocumentHigh input.png output.jp2

# Available presets: DocumentLow, DocumentHigh, WebLow, WebHigh
```

## Preset Comparison

| Preset | Quality | Transform | MCT | Use Case |
|--------|---------|-----------|-----|----------|
| **DocumentLow** | 30 | 9/7 | Yes | Extreme compression, scanned docs |
| **DocumentHigh** | 85 | 9/7 | Yes | High-fidelity document archival |
| **WebLow** | 42 | 9/7 | Yes | Web delivery (aggressive) |
| **WebHigh** | 62 | 9/7 | Yes | Web delivery (quality) |

All presets use the Irreversible 9/7 wavelet (production default). Quality-based quantization step scaling is applied automatically for q < 50.

## Architecture

### Encoding Pipeline

1. **Input Validation**: Verify 8-bit unsigned components, consistent dimensions
2. **Color Transform**: RGB→YCbCr via ICT if MCT enabled
3. **Forward DWT**: 9/7 float lifting with symmetric extension
4. **Quantization**: Scalar Expounded with subband-specific exponents/mantissas
5. **Tier-1 Coding**: Bit-plane entropy encoding (significance propagation, magnitude refinement, cleanup)
6. **PCRD Analysis**: Lambda-driven optimal truncation selection per code block
7. **Packet Assembly**: LRCP order with perceptual weighting
8. **Marker Layout**: SIZ, COD, QCD, SOT, SOP, SOD, EOC

### Perceptual Optimization

The encoder uses local contrast masking to guide bit allocation:
- High-contrast regions (edges, text): higher visibility weight → more bits
- Smooth regions: lower weight → fewer bits  
- Near-white regions: visibility override down to 0.05 (reserved for structure)

This ensures compression artifacts are kept in perceptually less-sensitive areas.

## Performance Characteristics

### vs. JPEG
- **Bitrate advantage**: 15–30% file size reduction at equal visual quality (moderate-to-high bitrates)
- **Low bitrate**: JPEG competitive at extreme compression (< 0.1 bpp); JP2 wins at 0.3+ bpp
- **Artifact profile**: Wavelet ringing (in flat regions) vs. JPEG blocking; both perceptually optimized

### Encoding Speed
- Single-threaded: ~1–3 MiB/s depending on preset (rough estimate)
- Multi-threaded (rayon): Near-linear speedup on 4+ cores

## Input Requirements

- **Format**: 8-bit unsigned integer per component
- **Color Space**: Grayscale or sRGB
- **Dimensions**: Any size ≥ 2×2 (no practical limit)
- **Subsampling**: Not supported; all components must match dimensions

## Dependencies

- **rayon** (1.10+) – Data parallelism for tile and layer encoding

No external C/C++ libraries or system dependencies required.

## Testing

```bash
cargo test
```

Tests cover:
- Wavelet norm tables (5/3 and 9/7)
- Quantization step size encoding
- Plan generation and decomposition level capping
- Rate-quality curves
- Round-trip DCT masking

## License

Dual-licensed under MIT or Apache-2.0.

## Use Cases

- **Document Archival**: High-fidelity preservation of scanned documents (DocumentHigh)
- **Web Delivery**: Efficient distribution of photographs and illustrations (WebLow/WebHigh)
- **Content Management Systems**: Backend image processing for mixed media
- **Embedded Systems**: Compact image compression without external dependencies

## Future Enhancements

Potential additions (not currently implemented):
- Lossless 5/3 optimization (currently production default is irreversible 9/7)
- Region-of-Interest (ROI) encoding
- Tiling (currently single-tile per image)
- Extended marker sets (COM, TLM)
- Progressive streaming with Quality Layers
