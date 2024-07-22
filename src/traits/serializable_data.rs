use serde::{Deserialize, Serialize};

pub trait SerializableData: Serialize + for<'de> Deserialize<'de> {}

impl<T> SerializableData for T where T: Serialize + for<'de> Deserialize<'de> {}
