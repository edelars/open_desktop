//! Strict S4-33a guest virtual alias probe profile.
use crate::stage4_flat_probe::{
    LoadedStage4FlatProbe, Stage4FlatProbeLoadError, load_stage4_alias_image,
};

const CODE: [u8; 6] = [0x48, 0x89, 0x0b, 0x58, 0xee, 0xf4];
const STORE_OFFSET: usize = 0x8008;

pub fn load_stage4_alias_probe(
    image: &[u8],
) -> Result<LoadedStage4FlatProbe, Stage4FlatProbeLoadError> {
    if image.len() != crate::STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return load_stage4_alias_image(image, &CODE, 0);
    }
    let store_value = u64::from_le_bytes(
        image[STORE_OFFSET..STORE_OFFSET + 8]
            .try_into()
            .expect("fixed alias store field"),
    );
    load_stage4_alias_image(image, &CODE, store_value)
}
