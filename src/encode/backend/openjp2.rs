use super::CodestreamBackend;
use crate::encode::context::EncodeContext;
use crate::error::{Jp2LamError, Result};
use crate::model::{ColorSpace, Image};
use crate::plan::WaveletTransform;
use openjp2::{
    default_encoder_parameters, opj_image, opj_image_comptparm, Codec, Stream, CODEC_FORMAT,
    OPJ_CLRSPC_GRAY, OPJ_CLRSPC_SRGB,
};

pub(crate) struct OpenJp2Backend;

impl CodestreamBackend for OpenJp2Backend {
    fn supports(&self, _context: &EncodeContext<'_>) -> bool {
        true
    }

    fn encode_codestream(&self, context: &EncodeContext<'_>) -> Result<Vec<u8>> {
        let image = context.image;
        let plan = &context.plan;
        let components = component_params(image);
        let mut opj_image = opj_image::create(&components, colorspace(image.colorspace));
        opj_image.x0 = 0;
        opj_image.y0 = 0;
        opj_image.x1 = image.width;
        opj_image.y1 = image.height;

        {
            let Some(mut comp_iter) = opj_image.comps_data_mut_iter() else {
                return Err(Jp2LamError::EncodeFailed(
                    "failed to access OpenJPEG image components".to_string(),
                ));
            };

            for component_index in 0..image.components.len() {
                let Some(dst) = comp_iter.next() else {
                    return Err(Jp2LamError::EncodeFailed(
                        "OpenJPEG component allocation mismatch".to_string(),
                    ));
                };
                let Some(src) = context.component_data(component_index) else {
                    return Err(Jp2LamError::EncodeFailed(
                        "missing component samples in encode context".to_string(),
                    ));
                };
                for (out, &sample) in dst.iter_mut().zip(src.iter()) {
                    *out = sample;
                }
            }
        }

        let mut params = default_encoder_parameters();
        params.tcp_numlayers = plan.layers.len() as i32;
        // OpenJPEG's encoder expects DISTO allocation mode even for lossless
        // when tcp_rates[0] is 0 (reversible path). Leaving this disabled can
        // produce non-exact "lossless" output on some fixtures.
        params.cp_disto_alloc = 1;
        params.cp_fixed_quality = 0;
        params.numresolution = i32::from(plan.num_resolutions.max(1));
        params.cblockw_init = plan.code_block_size.width as i32;
        params.cblockh_init = plan.code_block_size.height as i32;
        params.irreversible = match plan.transform {
            WaveletTransform::Reversible53 => 0,
            WaveletTransform::Irreversible97 => 1,
        };
        params.tcp_rates[0] = plan.layers[0].target_rate.unwrap_or(0.0);
        params.tcp_mct = if plan.use_mct { 1 } else { 0 };

        let mut codec = Codec::new_encoder(CODEC_FORMAT::OPJ_CODEC_J2K)
            .ok_or_else(|| Jp2LamError::EncodeFailed("failed to create encoder".to_string()))?;
        let mut stream = Stream::new_memory_writer(1 << 20);

        if codec.setup_encoder(&mut params, &mut opj_image) == 0 {
            return Err(Jp2LamError::EncodeFailed(
                "setup_encoder failed".to_string(),
            ));
        }
        if codec.start_compress(&mut opj_image, &mut stream) == 0 {
            return Err(Jp2LamError::EncodeFailed(
                "start_compress failed".to_string(),
            ));
        }
        if codec.encode(&mut stream) == 0 {
            return Err(Jp2LamError::EncodeFailed("encode failed".to_string()));
        }
        if codec.end_compress(&mut stream) == 0 {
            return Err(Jp2LamError::EncodeFailed("end_compress failed".to_string()));
        }
        stream
            .flush()
            .map_err(|err| Jp2LamError::EncodeFailed(err.to_string()))?;
        stream
            .into_bytes()
            .map_err(|err| Jp2LamError::EncodeFailed(err.to_string()))
    }
}

fn component_params(image: &Image) -> Vec<opj_image_comptparm> {
    image
        .components
        .iter()
        .map(|component| opj_image_comptparm {
            dx: component.dx,
            dy: component.dy,
            w: component.width,
            h: component.height,
            x0: 0,
            y0: 0,
            prec: component.precision,
            bpp: component.precision,
            sgnd: component.signed as u32,
        })
        .collect()
}

fn colorspace(color_space: ColorSpace) -> openjp2::OPJ_COLOR_SPACE {
    match color_space.encoding_domain() {
        ColorSpace::Gray => OPJ_CLRSPC_GRAY,
        ColorSpace::Srgb => OPJ_CLRSPC_SRGB,
        _ => OPJ_CLRSPC_SRGB,
    }
}

#[cfg(test)]
mod tests {
    use super::component_params;
    use crate::model::{ColorSpace, Component, Image};

    #[test]
    fn component_params_preserve_component_metadata() {
        let image = Image {
            width: 3,
            height: 2,
            components: vec![Component {
                data: vec![0; 6],
                width: 3,
                height: 2,
                precision: 12,
                signed: true,
                dx: 2,
                dy: 3,
            }],
            colorspace: ColorSpace::Gray,
        };

        let params = component_params(&image);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].w, 3);
        assert_eq!(params[0].h, 2);
        assert_eq!(params[0].prec, 12);
        assert_eq!(params[0].bpp, 12);
        assert_eq!(params[0].sgnd, 1);
        assert_eq!(params[0].dx, 2);
        assert_eq!(params[0].dy, 3);
    }
}
