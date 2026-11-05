//! Strict S4-32b artifact profile, reusing the S4-32a machine construction.

use crate::stage4_flat_probe::{
    LoadedStage4FlatProbe, Stage4FlatProbeLoadError, load_stage4_image,
};

const CODE: [u8; 8] = [0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03, 0xee, 0xf4];
const INITIAL_OFFSET: usize = 0x8000;
const STORE_VALUE_OFFSET: usize = 0x8008;

/// Load a fixed-layout Stage4StoreLoadProbeV1 artifact and hand off its
/// store-value qword in RCX. Both data qwords remain artifact-owned bytes.
pub fn load_stage4_store_load_probe(
    image: &[u8],
) -> Result<LoadedStage4FlatProbe, Stage4FlatProbeLoadError> {
    if image.len() != crate::STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return load_stage4_image(image, &CODE, 0);
    }
    let initial = read_qword(image, INITIAL_OFFSET);
    let store_value = read_qword(image, STORE_VALUE_OFFSET);
    if initial == store_value {
        return Err(Stage4FlatProbeLoadError::Layout(
            crate::Stage4FlatProbeLayoutError::DataQwordsEqual,
        ));
    }
    load_stage4_image(image, &CODE, store_value)
}

fn read_qword(image: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        image[offset..offset + 8]
            .try_into()
            .expect("fixed image field"),
    )
}
