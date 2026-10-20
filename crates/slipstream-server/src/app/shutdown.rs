use super::*;

impl Application {
    fn shutdown_blocking(&self) -> Result<(), ServerError> {
        let mut closed = self.shutdown.lock().expect("application shutdown poisoned");
        if *closed {
            return Ok(());
        }
        *closed = true;
        drop(closed);
        let preview_result = self
            .preview
            .shutdown()
            .map_err(|error| ServerError::Preview(error.to_string()));
        let library_result = self.library.shutdown().map_err(ServerError::Library);
        preview_result.and(library_result)
    }

    pub async fn shutdown(self: &Arc<Self>) -> Result<(), ServerError> {
        // Close recovery admission first; accepted operations may still need
        // the Library for the final commit and publication.
        self.close_recovery().await;
        // Stop new admissions and drain the application-owned leader before
        // closing the Library, so publication and status accounting complete.
        self.scan_cycle.close();
        self.tools.begin_shutdown();
        if let Some(exports) = &self.exports {
            exports.begin_shutdown();
        }
        if let Some(proxies) = &self.proxies {
            proxies.begin_shutdown();
        }
        self.scan_cycle.wait_for_idle().await;
        let tool_result = self.tools.shutdown().await.map_err(ServerError::Export);
        ReviewWarmup::close(self).await;
        if let Some(proxies) = &self.proxies {
            proxies.shutdown().await;
        }
        if let Some(exports) = &self.exports {
            exports.shutdown_processing().await;
        }
        let application = Arc::clone(self);
        let result = tokio::task::spawn_blocking(move || application.shutdown_blocking())
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?;
        tool_result.and(result)
    }
}
