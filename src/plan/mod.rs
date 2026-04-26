mod derive;
mod validate;

use crate::error::Result;
use crate::model::{ColorSpace, EncodeOptions, Image, OutputFormat, Preset};
use derive::{
    apply_quality_step_scaling, derive_component_plans, derive_lane, derive_subband_quants,
    max_decompositions, max_target_decompositions, tcp_rate_from_quality, transform_for, use_mct,
    PresetParams,
};
use validate::validate_image;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressionOrder {
    Lrcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaveletTransform {
    Reversible53,
    Irreversible97,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuantizationStyle {
    NoQuantization,
    ScalarExpounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BandOrientation {
    Ll,
    Hl,
    Lh,
    Hh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EncodeLane {
    GrayLossless,
    GrayLossy,
    RgbLossless,
    RgbLossy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodeBlockSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct QualityLayer {
    pub target_rate: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TilePlan {
    pub index: u16,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ComponentPlan {
    pub precision: u32,
    pub signed: bool,
    pub dx: u32,
    pub dy: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubbandQuant {
    pub resolution: u8,
    pub band: BandOrientation,
    pub exponent: u8,
    pub mantissa: u16,
}

impl From<&crate::model::Component> for ComponentPlan {
    fn from(component: &crate::model::Component) -> Self {
        Self {
            precision: component.precision,
            signed: component.signed,
            dx: component.dx,
            dy: component.dy,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct EncodingPlan {
    pub width: u32,
    pub height: u32,
    pub component_count: u16,
    pub colorspace: ColorSpace,
    pub output_format: OutputFormat,
    pub preset: Preset,
    pub quality: u8,
    pub lane: EncodeLane,
    pub progression_order: ProgressionOrder,
    pub transform: WaveletTransform,
    pub quantization_style: QuantizationStyle,
    pub use_mct: bool,
    pub decomposition_levels: u8,
    pub num_resolutions: u8,
    pub code_block_size: CodeBlockSize,
    pub guard_bits: u8,
    pub layers: [QualityLayer; 1],
    pub tile: TilePlan,
    pub components: Vec<ComponentPlan>,
    pub subband_quants: Vec<SubbandQuant>,
}

impl EncodingPlan {
    pub(crate) fn build(image: &Image, options: &EncodeOptions) -> Result<Self> {
        validate_image(image)?;

        let encoding_colorspace = image.colorspace.encoding_domain();
        let preset_params = options.preset.params();
        let quality = preset_params.quality;
        let decomposition_cap = max_target_decompositions(encoding_colorspace, options.preset);
        let decomposition_levels =
            max_decompositions(image.width, image.height).min(decomposition_cap) as u8;
        let use_mct = use_mct(encoding_colorspace, options.preset);
        let transform = transform_for(options.preset);
        let target_rate = if quality >= 100 {
            None
        } else {
            Some(tcp_rate_from_quality(quality, options.preset))
        };
        let lane = derive_lane(
            encoding_colorspace,
            target_rate.is_none() && matches!(transform, WaveletTransform::Reversible53),
        );
        let components = derive_component_plans(image);
        let (quantization_style, mut subband_quants) = derive_subband_quants(
            image.components[0].precision,
            decomposition_levels,
            transform,
        );
        // Scale step sizes at the plan level so quantizer, tier-1 bitplane
        // analysis, PCRD distortion estimates, and the QCD header all agree.
        if matches!(transform, WaveletTransform::Irreversible97) {
            apply_quality_step_scaling(&mut subband_quants, quality);
        }

        Ok(Self {
            width: image.width,
            height: image.height,
            component_count: image.components.len() as u16,
            colorspace: encoding_colorspace,
            output_format: options.format,
            preset: options.preset,
            quality,
            lane,
            progression_order: ProgressionOrder::Lrcp,
            transform,
            quantization_style,
            use_mct,
            decomposition_levels,
            num_resolutions: decomposition_levels.saturating_add(1),
            code_block_size: CodeBlockSize {
                width: 64,
                height: 64,
            },
            guard_bits: 2,
            layers: [QualityLayer { target_rate }],
            tile: TilePlan {
                index: 0,
                width: image.width,
                height: image.height,
            },
            components,
            subband_quants,
        })
    }

    pub(crate) fn is_lossless(&self) -> bool {
        matches!(self.transform, WaveletTransform::Reversible53)
            && self.layers[0].target_rate.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Component, EncodeOptions, Image, OutputFormat, Preset};

    #[test]
    fn rate_is_monotonic_for_web_low_preset() {
        let mut prev = f32::INFINITY;
        for q in (0u8..100).step_by(5) {
            let rate = tcp_rate_from_quality(q, Preset::WebLow);
            assert!(rate <= prev);
            prev = rate;
        }
    }

    #[test]
    fn validate_rejects_wrong_component_count() {
        let image = Image {
            width: 1,
            height: 1,
            components: vec![],
            colorspace: ColorSpace::Gray,
        };
        assert!(EncodingPlan::build(&image, &EncodeOptions::default()).is_err());
    }

    #[test]
    fn plan_caps_resolution_count() {
        let image = Image {
            width: 8,
            height: 8,
            components: vec![Component {
                data: vec![0; 64],
                width: 8,
                height: 8,
                precision: 8,
                signed: false,
                dx: 1,
                dy: 1,
            }],
            colorspace: ColorSpace::Gray,
        };
        let plan = EncodingPlan::build(
            &image,
            &EncodeOptions {
                preset: Preset::DocumentHigh,
                format: OutputFormat::Jp2,
            },
        )
        .expect("build plan");
        assert!(plan.num_resolutions <= 4);
        assert_eq!(plan.progression_order, ProgressionOrder::Lrcp);
        assert_eq!(plan.code_block_size.width, 64);
        assert_eq!(plan.lane, EncodeLane::GrayLossy);
        assert_eq!(plan.quantization_style, QuantizationStyle::ScalarExpounded);
        assert_eq!(plan.tile.width, 8);
        assert_eq!(plan.components.len(), 1);
    }

    #[test]
    fn tiny_gray_plan_is_bounded() {
        for (width, height, expected_resolutions) in
            [(2, 2, 2), (3, 2, 2), (3, 3, 2), (5, 3, 2), (17, 19, 5)]
        {
            let image = Image {
                width,
                height,
                components: vec![Component {
                    data: vec![0; (width * height) as usize],
                    width,
                    height,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                }],
                colorspace: ColorSpace::Gray,
            };
            let plan = EncodingPlan::build(
                &image,
                &EncodeOptions {
                    preset: Preset::DocumentHigh,
                    format: OutputFormat::J2k,
                },
            )
            .expect("build plan");

            assert_eq!(plan.lane, EncodeLane::GrayLossy, "{width}x{height}");
            assert_eq!(
                plan.transform,
                WaveletTransform::Irreversible97,
                "{width}x{height}"
            );
            assert_eq!(
                plan.quantization_style,
                QuantizationStyle::ScalarExpounded,
                "{width}x{height}"
            );
            assert!(!plan.use_mct, "{width}x{height}");
            assert_eq!(
                plan.num_resolutions, expected_resolutions,
                "{width}x{height}"
            );
            assert_eq!(
                plan.decomposition_levels,
                expected_resolutions - 1,
                "{width}x{height}"
            );
            assert!(plan.layers[0].target_rate.is_some(), "{width}x{height}");
        }
    }

    #[test]
    fn lossy_rgb_plan_enables_scalar_expounded_quantization() {
        let image = Image {
            width: 32,
            height: 32,
            components: vec![
                Component {
                    data: vec![0; 1024],
                    width: 32,
                    height: 32,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                },
                Component {
                    data: vec![0; 1024],
                    width: 32,
                    height: 32,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                },
                Component {
                    data: vec![0; 1024],
                    width: 32,
                    height: 32,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                },
            ],
            colorspace: ColorSpace::Srgb,
        };
        let plan = EncodingPlan::build(
            &image,
            &EncodeOptions {
                preset: Preset::WebLow,
                format: OutputFormat::J2k,
            },
        )
        .expect("build plan");
        assert_eq!(plan.lane, EncodeLane::RgbLossy);
        assert_eq!(plan.quantization_style, QuantizationStyle::ScalarExpounded);
        assert!(plan.use_mct);
        assert_eq!(plan.components.len(), 3);
        assert_eq!(plan.tile.height, 32);
        assert!(plan.subband_quants[0].mantissa > 0);
    }
}
