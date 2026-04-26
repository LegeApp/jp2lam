use crate::error::Result;
use crate::model::{ColorSpace, EncodeOptions, Image};
use crate::plan::EncodingPlan;

pub(crate) struct EncodeContext<'a> {
    pub image: &'a Image,
    pub plan: EncodingPlan,
    prepared_components: Vec<Vec<i32>>,
}

impl<'a> EncodeContext<'a> {
    pub(crate) fn new(image: &'a Image, options: &EncodeOptions) -> Result<Self> {
        let plan = EncodingPlan::build(image, options)?;
        let prepared_components = prepare_components(image);
        Ok(Self {
            image,
            plan,
            prepared_components,
        })
    }

    pub(crate) fn component_data(&self, index: usize) -> Option<&[i32]> {
        self.prepared_components.get(index).map(Vec::as_slice)
    }
}

fn prepare_components(image: &Image) -> Vec<Vec<i32>> {
    match image.colorspace {
        ColorSpace::Gray | ColorSpace::Rgb | ColorSpace::Srgb => image
            .components
            .iter()
            .map(|component| component.data.clone())
            .collect(),
        ColorSpace::Yuv | ColorSpace::YCbCr => yuv_family_to_rgb(image),
    }
}

fn yuv_family_to_rgb(image: &Image) -> Vec<Vec<i32>> {
    let (y, u, v) = (
        &image.components[0].data,
        &image.components[1].data,
        &image.components[2].data,
    );
    let mut r = Vec::with_capacity(y.len());
    let mut g = Vec::with_capacity(y.len());
    let mut b = Vec::with_capacity(y.len());

    for ((&yy, &uu), &vv) in y.iter().zip(u.iter()).zip(v.iter()) {
        let d = uu - 128;
        let e = vv - 128;
        // ITU-R BT.601 full-range integer approximation.
        let rr = yy + ((91881 * e) >> 16);
        let gg = yy - ((22554 * d + 46802 * e) >> 16);
        let bb = yy + ((116130 * d) >> 16);
        r.push(clamp_u8_range(rr));
        g.push(clamp_u8_range(gg));
        b.push(clamp_u8_range(bb));
    }

    vec![r, g, b]
}

#[inline]
fn clamp_u8_range(value: i32) -> i32 {
    value.clamp(0, 255)
}

#[cfg(test)]
mod tests {
    use super::EncodeContext;
    use crate::model::{ColorSpace, Component, EncodeOptions, Image, OutputFormat, Preset};
    use crate::plan::{EncodeLane, QuantizationStyle};

    #[test]
    fn context_exposes_plan_and_samples() {
        let image = Image {
            width: 2,
            height: 2,
            components: vec![Component {
                data: vec![1, 2, 3, 4],
                width: 2,
                height: 2,
                precision: 8,
                signed: false,
                dx: 1,
                dy: 1,
            }],
            colorspace: ColorSpace::Gray,
        };
        let context = EncodeContext::new(
            &image,
            &EncodeOptions {
                preset: Preset::DocumentHigh,
                format: OutputFormat::J2k,
            },
        )
        .expect("build context");
        assert_eq!(context.plan.lane, EncodeLane::GrayLossy);
        assert_eq!(
            context.plan.quantization_style,
            QuantizationStyle::ScalarExpounded
        );
        assert_eq!(context.component_data(0), Some([1, 2, 3, 4].as_slice()));
        assert_eq!(context.component_data(1), None);
    }

    #[test]
    fn context_converts_ycbcr_to_rgb_component_data() {
        let image = Image {
            width: 2,
            height: 1,
            components: vec![
                Component {
                    data: vec![64, 200],
                    width: 2,
                    height: 1,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                },
                Component {
                    data: vec![128, 90],
                    width: 2,
                    height: 1,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                },
                Component {
                    data: vec![128, 170],
                    width: 2,
                    height: 1,
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                },
            ],
            colorspace: ColorSpace::YCbCr,
        };

        let context = EncodeContext::new(
            &image,
            &EncodeOptions {
                preset: Preset::DocumentHigh,
                format: OutputFormat::J2k,
            },
        )
        .expect("build context");

        assert_eq!(context.plan.colorspace, ColorSpace::Srgb);
        assert_eq!(context.component_data(0), Some([64, 255].as_slice()));
        assert_eq!(context.component_data(1), Some([64, 184].as_slice()));
        assert_eq!(context.component_data(2), Some([64, 132].as_slice()));
    }
}
