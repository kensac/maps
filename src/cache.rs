use serde::{Deserialize, Serialize};
use std::{ffi::OsStr, fs};

pub trait SerializableData: Serialize + for<'de> Deserialize<'de> {}

impl<T> SerializableData for T where T: Serialize + for<'de> Deserialize<'de> {}

pub fn load_cache<T: SerializableData>(filename: &str) -> Result<T, Box<dyn std::error::Error>> {
    let data = fs::read(filename)?;
    let cached_data: T = bincode::deserialize(&data)?;
    Ok(cached_data)
}

pub fn save_cache<T: SerializableData>(
    filename: &OsStr,
    data: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let serialized_data = bincode::serialize(data)?;
    fs::write(filename, serialized_data)?;
    Ok(())
}
