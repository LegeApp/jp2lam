use jp2lam::{encode, print_timing_data, ColorSpace, Component, EncodeOptions, Image, OutputFormat};
#[cfg(feature = "counters")]
use jp2lam::print;
use std::path::PathBuf;
use std::time::Instant;

fn lear_png_path() -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()
        .map(|p| p.join("lear.png"))
        .unwrap_or_else(|| PathBuf::new());
    if workspace.exists() {
        return workspace;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lear.png")
}

fn load_lear_rgb() -> Image {
    let path = lear_png_path();
    let dyn_img = image::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {}: {}", path.display(), e))
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
    Image {
        width,
        height,
        components: vec![
            Component { data: r, width, height, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: g, width, height, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: b, width, height, precision: 8, signed: false, dx: 1, dy: 1 },
        ],
        colorspace: ColorSpace::Srgb,
    }
}

fn load_lear_gray() -> Image {
    let path = lear_png_path();
    let dyn_img = image::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {}: {}", path.display(), e))
        .into_luma8();
    let width = dyn_img.width();
    let height = dyn_img.height();
    let n = (width * height) as usize;
    let mut data = Vec::with_capacity(n);
    for pixel in dyn_img.pixels() {
        data.push(pixel[0] as i32);
    }
    Image {
        width,
        height,
        components: vec![Component { data, width, height, precision: 8, signed: false, dx: 1, dy: 1 }],
        colorspace: ColorSpace::Gray,
    }
}

fn run_benchmark(name: &str, image: &Image, quality: u8) {
    let iterations = 3;
    let mut total_time = std::time::Duration::ZERO;
    
    println!("\n=== {} ({}x{}, {} components) ===", 
        name, image.width, image.height, image.components.len());
    
    for i in 0..iterations {
        let start = Instant::now();
        let bytes = encode(&image, &EncodeOptions {
            quality,
            format: OutputFormat::Jp2,
        }).expect("encode failed");
        let elapsed = start.elapsed();
        total_time += elapsed;
        println!("Iter {}: {} bytes in {:?}", i+1, bytes.len(), elapsed);
    }
    
    let avg = total_time / iterations;
    println!("Avg: {:?} ({:.2} MP/s)", avg, 
        (image.width as f64 * image.height as f64 / 1_000_000.0) / (avg.as_secs_f64()));
    
    #[cfg(feature = "counters")]
    print();
}

fn main() {
    let rgb = load_lear_rgb();
    let gray = load_lear_gray();
    
    println!("Loaded lear.png: {}x{}", rgb.width, rgb.height);
    
    // Test grayscale first (simpler pipeline)
    run_benchmark("Grayscale q=50", &gray, 50);
    
    // Then RGB
    run_benchmark("RGB q=50", &rgb, 50);
    
    // Print profiling data
    print_timing_data();
}
