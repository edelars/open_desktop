//! Strict S4-32c artifact profile for the bounded guest stack probe.

use crate::stage4_flat_probe::{
    LoadedStage4FlatProbe, Stage4FlatProbeLoadError, load_stage4_image,
};

const CODE: [u8; 4] = [0x51, 0x58, 0xee, 0xf4];
const PAYLOAD_OFFSET: usize = 0x8000;

/// Load a fixed-layout Stage4StackProbeV1 image and hand its payload to RCX.
pub fn load_stage4_stack_probe(
    image: &[u8],
) -> Result<LoadedStage4FlatProbe, Stage4FlatProbeLoadError> {
    if image.len() != crate::STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return load_stage4_image(image, &CODE, 0);
    }
    let payload = u64::from_le_bytes(
        image[PAYLOAD_OFFSET..PAYLOAD_OFFSET + 8]
            .try_into()
            .expect("fixed stack probe payload field"),
    );
    load_stage4_image(image, &CODE, payload)
}
