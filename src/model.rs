#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Gray,
    #[doc(hidden)]
    Rgb,
    Srgb,
    #[doc(hidden)]
    Yuv,
    #[doc(hidden)]
    YCbCr,
}

impl ColorSpace {
    pub fn encoding_domain(self) -> Self {
        match self {
            Self::Gray => Self::Gray,
            Self::Rgb | Self::Srgb | Self::Yuv | Self::YCbCr => Self::Srgb,
        }
    }

    pub fn component_count(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb | Self::Srgb | Self::Yuv | Self::YCbCr => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Jp2,
    J2k,
}

/// Encoding preset selecting both content type and quality tier.
///
/// Pick the preset that matches your source material and the PDF use case.
/// Quality settings within each preset are tuned for that scenario; you do not
/// need to set a quality value separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Scanned book pages destined for a PDF — compressed but fully readable.
    DocumentLow,
    /// Scanned book pages destined for a PDF — high fidelity, near-archival.
    DocumentHigh,
    /// Web-derived images (screenshots, web-rips) destined for a PDF — compact.
    WebLow,
    /// Web-derived images (screenshots, web-rips) destined for a PDF — crisp.
    WebHigh,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub data: Vec<i32>,
    pub width: u32,
    pub height: u32,
    pub precision: u32,
    pub signed: bool,
    pub dx: u32,
    pub dy: u32,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub components: Vec<Component>,
    pub colorspace: ColorSpace,
}

impl Image {
    /// Construct from interleaved sRGB bytes (3 bytes per pixel, R-G-B order).
    ///
    /// `data.len()` must equal `width * height * 3`.
    pub fn from_rgb_bytes(width: u32, height: u32, data: &[u8]) -> crate::error::Result<Self> {
        let expected = (width as usize) * (height as usize) * 3;
        if data.len() != expected {
            return Err(crate::error::Jp2LamError::InvalidInput(format!(
                "RGB buffer length {} does not match {}×{}×3={}",
                data.len(),
                width,
                height,
                expected
            )));
        }
        let pixel_count = (width * height) as usize;
        let mut r = Vec::with_capacity(pixel_count);
        let mut g = Vec::with_capacity(pixel_count);
        let mut b = Vec::with_capacity(pixel_count);
        for px in data.chunks_exact(3) {
            r.push(i32::from(px[0]));
            g.push(i32::from(px[1]));
            b.push(i32::from(px[2]));
        }
        Ok(Self {
            width,
            height,
            components: vec![
                make_component(r, width, height),
                make_component(g, width, height),
                make_component(b, width, height),
            ],
            colorspace: ColorSpace::Srgb,
        })
    }

    /// Construct from grayscale bytes (1 byte per pixel).
    ///
    /// `data.len()` must equal `width * height`.
    pub fn from_gray_bytes(width: u32, height: u32, data: &[u8]) -> crate::error::Result<Self> {
        let expected = (width as usize) * (height as usize);
        if data.len() != expected {
            return Err(crate::error::Jp2LamError::InvalidInput(format!(
                "Gray buffer length {} does not match {}×{}={}",
                data.len(),
                width,
                height,
                expected
            )));
        }
        let samples: Vec<i32> = data.iter().map(|&v| i32::from(v)).collect();
        Ok(Self {
            width,
            height,
            components: vec![make_component(samples, width, height)],
            colorspace: ColorSpace::Gray,
        })
    }
}

fn make_component(data: Vec<i32>, width: u32, height: u32) -> Component {
    Component {
        data,
        width,
        height,
        precision: 8,
        signed: false,
        dx: 1,
        dy: 1,
    }
}

#[derive(Debug, Clone)]
pub struct EncodeOptions {
    pub preset: Preset,
    pub format: OutputFormat,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            preset: Preset::DocumentHigh,
            format: OutputFormat::Jp2,
        }
    }
}
