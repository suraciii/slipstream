use super::*;

impl Library {
    /// Reads one persisted XMP snapshot by its export identity.
    pub async fn xmp_export(
        &self,
        export_id: &str,
    ) -> Result<Option<crate::XmpExportRecord>, LibraryError> {
        self.admitted(|persistence| persistence.read_xmp_receiver(export_id))
            .await
    }

    /// Lists one Photo's persisted XMP snapshots, newest first. `None`
    /// means the Photo is not part of the published Library.
    pub async fn photo_xmp_exports(
        &self,
        photo_id: &str,
    ) -> Result<Option<Vec<crate::XmpExportRecord>>, LibraryError> {
        self.admitted(|persistence| persistence.list_xmp_receiver(photo_id))
            .await
    }
}
