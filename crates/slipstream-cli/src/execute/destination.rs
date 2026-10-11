use super::*;
pub(super) fn prepare(
    command: &Command,
) -> Result<Option<preview_download::Destination>, CommandFailure> {
    let destination = match command {
        Command::Photos {
            command: PhotoCommand::Preview { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::Preview,
            file,
        )?),
        Command::Photos {
            command: PhotoCommand::ProcessingPreview { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::ProcessingPreview,
            file,
        )?),
        Command::Processing {
            command: ProcessingCommand::ArtifactDownload { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::ProcessingArtifact,
            file,
        )?),
        Command::Photos {
            command: PhotoCommand::HistoricalExportDownload { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::Artifact,
            file,
        )?),
        _ => None,
    };
    Ok(destination)
}
