use super::*;

pub(crate) async fn execute(
    client: &ServiceClient,
    admission: &AdmissionState,
    args: &PhotoRestoreArgs,
    prepared: Option<&PreparedRestore>,
    exclusions: &[String],
) -> Result<Value, CommandFailure> {
    let photo_ids = prepared
        .map(|prepared| {
            prepared
                .markers
                .iter()
                .map(|marker| marker.photo_id.clone())
                .collect()
        })
        .unwrap_or_default();
    let identity = MutationIdentity {
        operation: Operation::PhotosRestore,
        photo_ids,
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    if args.all {
        let result: PhotoRestoreWire = client
            .mutation(
                &identity,
                admission,
                client.endpoint(&["api", "trash", "restore"]),
                json!({
                    "operationId": args.operation_id,
                    "all": true,
                    "photoIds": [],
                    "excludePhotoIds": exclusions,
                }),
            )
            .await?;
        confirmed_restore_result(&identity, &args.operation_id, None, exclusions, result)
    } else if let Some(prepared) = prepared {
        let result: PhotoRestoreWire = client
            .mutation(
                &identity,
                admission,
                client.endpoint(&["api", "photos", "restore-explicit"]),
                prepared.body(&args.operation_id),
            )
            .await?;
        confirmed_restore_result(&identity, &args.operation_id, Some(prepared), &[], result)
    } else {
        let data: Value = client
            .mutation(
                &identity,
                admission,
                client.endpoint(&["api", "photos", "restore"]),
                json!({ "operation": args.operation_id }),
            )
            .await?;
        if !data.is_object() {
            return Err(CommandFailure::unknown(&identity));
        }
        Ok(data)
    }
}
