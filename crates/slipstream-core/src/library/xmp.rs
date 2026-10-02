use super::*;

impl Library {
    /// Captures the selected darktable step's semantic exposure and white-balance
    /// intent without opening the Original or running processing. Arbitrary
    /// controls are not portable; existing snapshots always replay unchanged.
    pub async fn create_xmp_export(
        &self,
        photo_id: &str,
        request_id: &str,
        expected_recipe: &str,
        expected_source: &str,
        now: i64,
    ) -> Result<crate::XmpCreateOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.create_xmp_receiver(
                photo_id,
                request_id,
                expected_recipe,
                expected_source,
                now,
            )
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads one persisted XMP snapshot by its export identity.
    pub async fn xmp_export(
        &self,
        export_id: &str,
    ) -> Result<Option<crate::XmpExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.read_xmp_receiver(export_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Lists one Photo's persisted XMP snapshots, newest first. `None`
    /// means the Photo is not part of the published Library.
    pub async fn photo_xmp_exports(
        &self,
        photo_id: &str,
    ) -> Result<Option<Vec<crate::XmpExportRecord>>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.list_xmp_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }
}
