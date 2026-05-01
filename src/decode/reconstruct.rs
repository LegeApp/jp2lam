//! Coefficient reconstruction, inverse DWT, and color transform for the decoder.

use crate::dwt::{inverse_53_2d_in_place, inverse_97_2d_in_place};
use crate::dwt::norms::band_gain;
use crate::error::{Jp2LamError, Result};
use crate::j2k::decode_markers::{
    CodestreamHeader, QuantizationStep, QuantizationStyle, WaveletTransform,
};
use crate::model::{ColorSpace, Component, Image};
use crate::plan::BandOrientation;

use super::t1::DecodedTileCoefficients;

#[derive(Debug, Clone, Copy)]
struct SubbandRect {
    index: usize,
    band: BandOrientation,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

pub(crate) fn reconstruct_image(
    header: &CodestreamHeader,
    colorspace: ColorSpace,
    tiles: Vec<DecodedTileCoefficients>,
) -> Result<Image> {
    match colorspace {
        ColorSpace::Gray => {
            if tiles.len() != 1 {
                return Err(invalid("grayscale JP2 must decode exactly one component"));
            }
            reconstruct_grayscale_image(header, tiles.into_iter().next().unwrap())
        }
        ColorSpace::Srgb => reconstruct_srgb_image(header, tiles),
        _ => Err(invalid(format!(
            "unsupported JP2 colorspace for reconstruction: {colorspace:?}"
        ))),
    }
}

pub(crate) fn reconstruct_grayscale_image(
    header: &CodestreamHeader,
    tile: DecodedTileCoefficients,
) -> Result<Image> {
    let component = header
        .siz
        .components
        .first()
        .ok_or_else(|| invalid("missing decoded component header"))?;
    let width = usize::try_from(header.siz.width)
        .map_err(|_| invalid("decoded width exceeds usize"))?;
    let height = usize::try_from(header.siz.height)
        .map_err(|_| invalid("decoded height exceeds usize"))?;
    if tile.component != 0 || tile.width != width || tile.height != height {
        return Err(invalid("decoded coefficient tile dimensions do not match SIZ"));
    }

    let samples = match header.cod.transform {
        WaveletTransform::Reversible53 => finalize_i32_samples(reconstruct_reversible_53_centered(
            header,
            tile.coefficients,
        )?),
        WaveletTransform::Irreversible97 => finalize_f32_samples(
            reconstruct_irreversible_97_centered(header, tile.coefficients, component.precision)?,
        ),
    };

    Ok(Image {
        width: header.siz.width,
        height: header.siz.height,
        colorspace: ColorSpace::Gray,
        components: vec![Component {
            data: samples,
            width: header.siz.width,
            height: header.siz.height,
            precision: u32::from(component.precision),
            signed: component.signed,
            dx: u32::from(component.dx),
            dy: u32::from(component.dy),
        }],
    })
}

fn reconstruct_srgb_image(
    header: &CodestreamHeader,
    mut tiles: Vec<DecodedTileCoefficients>,
) -> Result<Image> {
    if tiles.len() != 3 || header.siz.components.len() != 3 {
        return Err(invalid("sRGB JP2 must decode exactly three components"));
    }
    tiles.sort_by_key(|tile| tile.component);
    for (idx, tile) in tiles.iter().enumerate() {
        if tile.component != idx {
            return Err(invalid("decoded sRGB components are not contiguous"));
        }
    }

    let planes: Vec<Vec<i32>> = match header.cod.transform {
        WaveletTransform::Reversible53 => {
            let mut planes = Vec::with_capacity(3);
            for tile in tiles {
                planes.push(reconstruct_reversible_53_centered(header, tile.coefficients)?);
            }
            if header.cod.use_mct {
                inverse_rct_centered(&mut planes)?;
            }
            planes.into_iter().map(finalize_i32_samples).collect()
        }
        WaveletTransform::Irreversible97 => {
            let mut planes = Vec::with_capacity(3);
            for tile in tiles {
                let component = header
                    .siz
                    .components
                    .get(tile.component)
                    .ok_or_else(|| invalid("missing decoded component header"))?;
                planes.push(reconstruct_irreversible_97_centered(
                    header,
                    tile.coefficients,
                    component.precision,
                )?);
            }
            if header.cod.use_mct {
                inverse_ict_centered(&mut planes)?;
            }
            planes.into_iter().map(finalize_f32_samples).collect()
        }
    };

    let width = header.siz.width;
    let height = header.siz.height;
    Ok(Image {
        width,
        height,
        colorspace: ColorSpace::Srgb,
        components: planes
            .into_iter()
            .enumerate()
            .map(|(idx, data)| {
                let component = header.siz.components[idx];
                Component {
                    data,
                    width,
                    height,
                    precision: u32::from(component.precision),
                    signed: component.signed,
                    dx: u32::from(component.dx),
                    dy: u32::from(component.dy),
                }
            })
            .collect(),
    })
}

fn reconstruct_reversible_53_centered(
    header: &CodestreamHeader,
    mut coefficients: Vec<i32>,
) -> Result<Vec<i32>> {
    if header.qcd.style != QuantizationStyle::NoQuantization {
        return Err(invalid("reversible 5/3 reconstruction expects no quantization"));
    }
    inverse_53_2d_in_place(
        &mut coefficients,
        header.siz.width as usize,
        header.siz.height as usize,
        header.cod.decomposition_levels,
    )?;
    Ok(coefficients)
}

fn inverse_ict_centered(planes: &mut [Vec<f32>]) -> Result<()> {
    let [y, cb, cr] = planes else {
        return Err(invalid("ICT requires exactly three components"));
    };
    if y.len() != cb.len() || y.len() != cr.len() {
        return Err(invalid("ICT component lengths differ"));
    }
    for i in 0..y.len() {
        let yy = y[i];
        let cbb = cb[i];
        let crr = cr[i];
        let r = yy + 1.402f32 * crr;
        let g = yy - 0.344_13f32 * cbb - 0.714_14f32 * crr;
        let b = yy + 1.772f32 * cbb;
        y[i] = r;
        cb[i] = g;
        cr[i] = b;
    }
    Ok(())
}

fn inverse_rct_centered(planes: &mut [Vec<i32>]) -> Result<()> {
    let [y, db, dr] = planes else {
        return Err(invalid("RCT requires exactly three components"));
    };
    if y.len() != db.len() || y.len() != dr.len() {
        return Err(invalid("RCT component lengths differ"));
    }
    for i in 0..y.len() {
        let yy = y[i];
        let dbv = db[i];
        let drv = dr[i];
        let g = yy - ((dbv + drv) >> 2);
        let r = drv + g;
        let b = dbv + g;
        y[i] = r;
        db[i] = g;
        dr[i] = b;
    }
    Ok(())
}

fn reconstruct_irreversible_97_centered(
    header: &CodestreamHeader,
    coefficients: Vec<i32>,
    precision: u8,
) -> Result<Vec<f32>> {
    if header.qcd.style != QuantizationStyle::ScalarExpounded {
        return Err(invalid(
            "irreversible 9/7 reconstruction currently expects scalar-expounded QCD",
        ));
    }

    let width = header.siz.width as usize;
    let height = header.siz.height as usize;
    let rects = subband_rects(width, height, header.cod.decomposition_levels);
    let mut data = vec![0.0f32; coefficients.len()];
    for rect in rects {
        let quant = header
            .qcd
            .steps
            .get(rect.index)
            .copied()
            .ok_or_else(|| invalid("missing QCD step for subband reconstruction"))?;
        let step = quant_step(u32::from(precision), rect.band, quant);
        dequantize_rect(&coefficients, &mut data, width, rect, step);
    }

    inverse_97_2d_in_place(
        &mut data,
        width,
        height,
        header.cod.decomposition_levels,
    );

    Ok(data)
}

fn finalize_i32_samples(samples: Vec<i32>) -> Vec<i32> {
    samples
        .into_iter()
        .map(|sample| (sample + 128).clamp(0, 255))
        .collect()
}

fn finalize_f32_samples(samples: Vec<f32>) -> Vec<i32> {
    samples
        .into_iter()
        .map(|sample| (sample + 128.0).round().clamp(0.0, 255.0) as i32)
        .collect()
}

fn dequantize_rect(input: &[i32], output: &mut [f32], stride: usize, rect: SubbandRect, step: f32) {
    for y in rect.y0..rect.y1 {
        let row = y * stride;
        for x in rect.x0..rect.x1 {
            let q = input[row + x];
            output[row + x] = if q == 0 {
                0.0
            } else {
                let sign = if q >= 0 { 1.0 } else { -1.0 };
                sign * (q.unsigned_abs() as f32 + 0.5) * step
            };
        }
    }
}

fn quant_step(precision: u32, band: BandOrientation, quant: QuantizationStep) -> f32 {
    let numbps = (precision + u32::from(band_gain(band))) as i32;
    let exponent = i32::from(quant.exponent);
    let base = 1.0 + f32::from(quant.mantissa) / 2048.0;
    (base * 2f32.powi(numbps - exponent)).max(1e-6)
}

fn subband_rects(width: usize, height: usize, levels: u8) -> Vec<SubbandRect> {
    let resolutions = resolution_ladder(width, height, levels);
    let mut rects = Vec::with_capacity(1 + usize::from(levels) * 3);

    let ll = resolutions[0];
    rects.push(SubbandRect {
        index: 0,
        band: BandOrientation::Ll,
        x0: 0,
        y0: 0,
        x1: ll.0,
        y1: ll.1,
    });

    for index in 0..usize::from(levels) {
        let low = resolutions[index];
        let full = resolutions[index + 1];
        rects.push(SubbandRect {
            index: rects.len(),
            band: BandOrientation::Hl,
            x0: low.0,
            y0: 0,
            x1: full.0,
            y1: low.1,
        });
        rects.push(SubbandRect {
            index: rects.len(),
            band: BandOrientation::Lh,
            x0: 0,
            y0: low.1,
            x1: low.0,
            y1: full.1,
        });
        rects.push(SubbandRect {
            index: rects.len(),
            band: BandOrientation::Hh,
            x0: low.0,
            y0: low.1,
            x1: full.0,
            y1: full.1,
        });
    }

    rects
}

fn resolution_ladder(width: usize, height: usize, levels: u8) -> Vec<(usize, usize)> {
    let mut resolutions = Vec::with_capacity(usize::from(levels) + 1);
    let mut w = width;
    let mut h = height;
    resolutions.push((w, h));
    for _ in 0..levels {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        resolutions.push((w, h));
    }
    resolutions.reverse();
    resolutions
}

fn invalid(message: impl Into<String>) -> Jp2LamError {
    Jp2LamError::DecodeFailed(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_ict_uses_centered_unclipped_chroma() {
        let mut planes = vec![vec![0.0], vec![80.25], vec![-30.5]];

        inverse_ict_centered(&mut planes).expect("inverse ict");

        assert_eq!(finalize_f32_samples(planes.remove(0)), vec![85]);
        assert_eq!(finalize_f32_samples(planes.remove(0)), vec![122]);
        assert_eq!(finalize_f32_samples(planes.remove(0)), vec![270_i32.clamp(0, 255)]);
    }

    #[test]
    fn inverse_rct_uses_centered_unclipped_differences() {
        let mut planes = vec![vec![10], vec![80], vec![-30]];

        inverse_rct_centered(&mut planes).expect("inverse rct");

        assert_eq!(finalize_i32_samples(planes.remove(0)), vec![96]);
        assert_eq!(finalize_i32_samples(planes.remove(0)), vec![126]);
        assert_eq!(finalize_i32_samples(planes.remove(0)), vec![206]);
    }
}
