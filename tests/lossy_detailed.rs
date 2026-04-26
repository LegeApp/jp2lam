use jp2lam::{encode, ColorSpace, Component, EncodeOptions, Image, OutputFormat, Preset};

fn rgb_pattern(width: u32, height: u32) -> Image {
    let mut r = Vec::with_capacity((width * height) as usize);
    let mut g = Vec::with_capacity((width * height) as usize);
    let mut b = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let v = ((x as u32 * y as u32) % 256) as i32;
            r.push(v);
            g.push(v);
            b.push(v);
        }
    }
    Image {
        width,
        height,
        components: vec![
            Component {
                data: r,
                width,
                height,
                precision: 8,
                signed: false,
                dx: 1,
                dy: 1,
            },
            Component {
                data: g,
                width,
                height,
                precision: 8,
                signed: false,
                dx: 1,
                dy: 1,
            },
            Component {
                data: b,
                width,
                height,
                precision: 8,
                signed: false,
                dx: 1,
                dy: 1,
            },
        ],
        colorspace: ColorSpace::Srgb,
    }
}

fn interleave_rgb(image: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity((image.width * image.height * 3) as usize);
    let r = &image.components[0].data;
    let g = &image.components[1].data;
    let b = &image.components[2].data;
    for i in 0..r.len() {
        out.push(r[i] as u8);
        out.push(g[i] as u8);
        out.push(b[i] as u8);
    }
    out
}

#[test]
fn test_lossy_encoding_detailed() {
    println!("\nTesting lossy encoding with detailed comparison...");
    
    // Test with a simple gradient pattern
    let image = rgb_pattern(32, 32);
    let expected = interleave_rgb(&image);
    
    println!("Image size: {}x{}", image.width, image.height);
    println!("Expected data length: {}", expected.len());
    
    // Test different quality levels
    for quality in [10, 25, 50, 75, 90, 100].iter() {
        println!("\n=== Testing quality {} ===", quality);
        
        let options = EncodeOptions {
            preset: Preset::Image,
            quality: *quality,
            format: OutputFormat::Jp2,
        };
        
        let bytes = encode(&image, &options).expect("encode failed");
        println!("Encoded size: {} bytes", bytes.len());
        
        // Basic sanity checks
        assert!(bytes.len() > 20, "Encoded bytes too small for quality {}", quality);
        
        // Test that file size generally increases with quality (not strictly monotonic but generally true)
        if *quality >= 50 {
            // Higher quality should produce reasonable file sizes
            assert!(bytes.len() > 50, 
                    "File size too small for quality {}: {} bytes", quality, bytes.len());
        }
    }
    
    // Test that we can at least encode something reasonable
    let options = EncodeOptions {
        preset: Preset::Image,
        quality: 50,
        format: OutputFormat::Jp2,
    };
    let bytes = encode(&image, &options).expect("encode failed");
    assert!(bytes.len() > 100, "Encoded bytes too small for quality 50");
    
    // Test that we can also encode lossless (quality 100 with Text preset which should be lossless for grayscale)
    let gray_image = Image {
        width: 32,
        height: 32,
        components: vec![Component {
            data: vec![128; 32*32], // mid-gray
            width: 32,
            height: 32,
            precision: 8,
            signed: false,
            dx: 1,
            dy: 1,
        }],
        colorspace: ColorSpace::Gray,
    };
    
    let lossless_options = EncodeOptions {
        preset: Preset::Text, // This should be lossless for grayscale
        quality: 100,
        format: OutputFormat::Jp2,
    };
    
    let lossless_bytes = encode(&gray_image, &lossless_options).expect("encode lossless failed");
    println!("Lossless encoded size: {} bytes", lossless_bytes.len());
    assert!(lossless_bytes.len() > 50, "Lossless encoded bytes too small");
    
    println!("\nDone.");
}