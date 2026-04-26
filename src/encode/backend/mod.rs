mod native;
#[cfg(feature = "openjp2-oracle")]
mod openjp2;

use super::context::EncodeContext;
use crate::error::Result;

pub(crate) trait CodestreamBackend {
    fn supports(&self, context: &EncodeContext<'_>) -> bool;
    fn encode_codestream(&self, context: &EncodeContext<'_>) -> Result<Vec<u8>>;
}

pub(crate) use native::NativeBackend;
#[cfg(feature = "openjp2-oracle")]
pub(crate) use openjp2::OpenJp2Backend;
