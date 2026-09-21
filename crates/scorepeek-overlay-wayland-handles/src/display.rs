use crate::SurfaceHandle;

impl SurfaceHandle {
    /// Completes one synchronous connection round trip.
    ///
    /// # Errors
    /// Returns the Wayland connection error when the round trip fails.
    pub fn roundtrip(&self) -> Result<(), String> {
        self.connection
            .roundtrip()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
