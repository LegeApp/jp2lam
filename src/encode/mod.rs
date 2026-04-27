pub(crate) mod backend;
pub(crate) mod context;

#[cfg(feature = "counters")]
pub mod counters;

use crate::error::{Jp2LamError, Result};
use crate::j2k::CodestreamParts;
use crate::jp2;
use crate::model::{EncodeOptions, Image, OutputFormat};
use backend::{CodestreamBackend, NativeBackend};
use context::EncodeContext;
use std::io::Write;
use std::time::Instant;

#[cfg(feature = "profile")]
static TIMING_DATA: std::sync::Mutex<Vec<(String, std::time::Duration)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(feature = "profile")]
pub fn profile_enter(name: &'static str) -> ProfileScope {
    ProfileScope(name, Instant::now())
}

#[cfg(not(feature = "profile"))]
pub fn profile_enter(_name: &'static str) -> ProfileScope {
    ProfileScope("", Instant::now())
}

#[cfg(feature = "profile")]
pub fn print_timing_data() {
    if let Ok(times) = TIMING_DATA.lock() {
        if times.is_empty() {
            return;
        }
        let mut sorted: Vec<_> = times.clone();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let total: std::time::Duration = sorted.iter().map(|t| t.1).sum();
        println!("\n=== Profile Timing ({} entries) ===", sorted.len());
        for (name, dur) in sorted.iter().take(20) {
            let pct = 100.0 * dur.as_secs_f64() / total.as_secs_f64();
            println!("  {:>6.2}% {:12.3}ms  {}", pct, dur.as_secs_f64() * 1000.0, name);
        }
        println!("  Total: {:.3}ms", total.as_secs_f64() * 1000.0);
    }
}

#[cfg(not(feature = "profile"))]
pub fn print_timing_data() {}

#[cfg(feature = "profile")]
pub fn clear_timing_data() {
    if let Ok(mut times) = TIMING_DATA.lock() {
        times.clear();
    }
}

#[cfg(not(feature = "profile"))]
pub fn clear_timing_data() {}

#[cfg(not(feature = "counters"))]
pub mod counters {
    pub fn reset() {}
    pub fn print() {}
}

pub struct ProfileScope(&'static str, Instant);

impl Drop for ProfileScope {
    fn drop(&mut self) {
        #[cfg(feature = "profile")]
        {
            if !self.0.is_empty() {
                let elapsed = self.1.elapsed();
                if let Ok(mut times) = TIMING_DATA.lock() {
                    times.push((self.0.to_string(), elapsed));
                }
            }
        }
    }
}

pub fn encode(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let _p = profile_enter("encode::total");
    let context = EncodeContext::new(image, options)?;
    let native = NativeBackend;
    if !native.supports(&context) {
        return Err(Jp2LamError::EncodeFailed(
            "native backend does not support this lane".to_string(),
        ));
    }
    let backend_codestream = native.encode_codestream(&context)?;
    CodestreamParts::parse_single_tile(&backend_codestream)?;
    let codestream = backend_codestream;

    match context.plan.output_format {
        OutputFormat::J2k => Ok(codestream),
        OutputFormat::Jp2 => jp2::wrap_codestream(context.image, &codestream),
    }
}

pub fn encode_to_writer<W: Write>(
    image: &Image,
    options: &EncodeOptions,
    writer: &mut W,
) -> Result<()> {
    let bytes = encode(image, options)?;
    writer
        .write_all(&bytes)
        .map_err(|err| Jp2LamError::EncodeFailed(err.to_string()))
}

/// Encode and compute an internal PSNR estimate in one call.
///
/// The PSNR is computed by simulating the decoder: re-running the encoder
/// pipeline to get PCRD-truncated coefficients, applying inverse quantization
/// and inverse DWT, and comparing against the original pixels.
///
/// Returns `(encoded_bytes, psnr_db)`. For lossless encodes (quality == 100)
/// `psnr_db` is `f64::INFINITY`.
pub fn encode_with_psnr(image: &Image, options: &EncodeOptions) -> Result<(Vec<u8>, f64)> {
    let bytes = encode(image, options)?;
    let context = EncodeContext::new(image, options)?;
    let native = NativeBackend;
    let psnr = native.compute_psnr(&context)?;
    Ok((bytes, psnr))
}
