//! Film attempt execution on the executor journal: the staging socket
//! handoff, the frozen engine permit checks, and the bounded waits that
//! resolve a Film or qualified fixture attempt to its terminal outcome.

use super::*;

impl Executor {
    fn film_permit(
        &self,
        record: &Record,
        parent: Option<&ParentIdentity>,
        pid: u32,
    ) -> Result<(), ErrorCode> {
        self.backend.verify_film_bootstrap(record, pid)?;
        let Some((config, documents, _)) = &self.qualified else {
            return Ok(());
        };
        self.backend.verify_parent(parent)?;
        self.backend.admission_ready(parent)?;
        self.qualified_ready()?;
        let captured = record.film.as_ref().ok_or(ErrorCode::Uncertain)?;
        captured.grant.validate()?;
        let (_, expected) = documents.plan(
            &captured.grant.fixture.id,
            &config.envelope_sha256,
            record.receipt.limits.memory_bytes,
        )?;
        if captured.catalogue != config.catalogue_sha256
            || captured.resource_model != config.envelope_sha256
            || captured.grant.plan.qualified() != Some(&expected)
            || record.image_id != self.image_id
            || record.receipt.policy != self.policy
            || record.receipt.bundle != self.bundle
            || record.receipt.limits != self.config.limits()
        {
            return Err(ErrorCode::Unavailable);
        }
        self.backend.verify_film_limits(record)
    }

    fn film_wait<T>(
        &self,
        record: &Record,
        mut poll: impl FnMut() -> Result<Option<T>, ErrorCode>,
    ) -> Result<(Option<T>, Option<Outcome>), ErrorCode> {
        loop {
            if let Some(reason) = self.interrupted(record)? {
                return Ok((None, Some(reason)));
            }
            if !self.backend.live(record)?.running {
                return Ok((None, None));
            }
            if let Some(value) = poll()? {
                return Ok((Some(value), None));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Under the frozen journal view: a cancelled or expired attempt
    /// settles instead of releasing the engine, and a failed permit check
    /// invalidates the qualified observation before its error propagates.
    fn engine_gate(&self, record: &mut Record, pid: u32) -> Result<Option<Outcome>, ErrorCode> {
        let permit = {
            let data = self.lock()?;
            if data.registry.records[&record.receipt.sequence]
                .receipt
                .cancellation_requested
            {
                return Ok(Some(Outcome::Cancelled));
            }
            if now()? >= record.receipt.deadline_unix_ms {
                return Ok(Some(Outcome::Deadline));
            }
            self.film_permit(record, data.registry.parent_identity.as_ref(), pid)
        };
        if let Err(error) = permit {
            self.invalidate_observation(record)?;
            return Err(error);
        }
        Ok(None)
    }

    /// A failed engine permit check invalidates the qualified observation.
    fn invalidate_observation(&self, record: &mut Record) -> Result<(), ErrorCode> {
        if self.qualified.is_some() {
            record
                .film
                .as_mut()
                .ok_or(ErrorCode::Uncertain)?
                .qualification_observation_valid = Some(false);
            self.update(record)?;
        }
        Ok(())
    }

    pub(super) fn execute_film(
        &self,
        mut record: Record,
        mut session: Box<crate::staging::Session>,
    ) -> Result<(), ErrorCode> {
        use crate::{film, staging};
        use std::os::fd::AsRawFd;
        let execution = (|| -> Result<Option<Outcome>, ErrorCode> {
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            record
                .film
                .as_mut()
                .ok_or(ErrorCode::Uncertain)?
                .stage_release_intent = true;
            record.released = true;
            self.update(&record)?;
            crate::faults::at(
                &self.config,
                &record,
                crate::faults::Phase::StageReleaseIntent,
            )?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            self.backend.release(&mut record, |r| self.update(r))?;
            let (connected, reason) = self.film_wait(&record, || {
                if session.started.elapsed() > Duration::from_secs(10) {
                    return Err(ErrorCode::Uncertain);
                }
                let live = self.backend.live(&record)?;
                Ok(session.accept(live.pid)?.then_some(()))
            })?;
            if connected.is_none() {
                return Ok(reason);
            }
            if let Some(reason) = self.engine_gate(&mut record, session.pid)? {
                return Ok(Some(reason));
            }
            if now()? >= record.receipt.deadline_unix_ms {
                return Ok(Some(Outcome::Deadline));
            }
            if crate::faults::retain_snapshot_writer(&self.config, &record)? {
                session.retain_snapshot_writer()?;
            }
            session.offer()?;
            record.film.as_mut().ok_or(ErrorCode::Uncertain)?.phase = film::Phase::Staging;
            record.receipt.state = State::Running;
            self.update(&record)?;
            let fd = session
                .connection
                .as_ref()
                .ok_or(ErrorCode::Uncertain)?
                .as_raw_fd();
            let (ack, reason) = self.film_wait(&record, || {
                staging::receive::<film::StageAck>(fd).map_err(|_| ErrorCode::Uncertain)
            })?;
            let Some((ack, rights)) = ack else {
                return Ok(reason);
            };
            if !rights.is_empty() || ack != session.offer.ack() {
                return Err(ErrorCode::Uncertain);
            }
            crate::faults::at(&self.config, &record, crate::faults::Phase::StageAck)?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            let leaf = self.backend.pause_film(&mut record, |r| self.update(r))?;
            if self.backend.live(&record)?.pid != session.pid {
                return Err(ErrorCode::Uncertain);
            }
            match session.audit(&record, &leaf) {
                Ok(()) => {}
                Err(ErrorCode::InvalidRequest) => {
                    record.film.as_mut().ok_or(ErrorCode::Uncertain)?.detail =
                        Some(film::Detail::SourceMismatch);
                    self.update(&record)?;
                    return Ok(Some(Outcome::Interrupted));
                }
                Err(error) => return Err(error),
            }
            record.film.as_mut().ok_or(ErrorCode::Uncertain)?.phase = film::Phase::Sealed;
            self.update(&record)?;
            crate::faults::at(&self.config, &record, crate::faults::Phase::SnapshotSealed)?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            record
                .film
                .as_mut()
                .ok_or(ErrorCode::Uncertain)?
                .engine_release_intent = true;
            self.update(&record)?;
            crate::faults::at(
                &self.config,
                &record,
                crate::faults::Phase::EngineReleaseIntent,
            )?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            session.unlink_endpoint()?;
            self.backend.release(&mut record, |r| self.update(r))?;
            let permit = film::Permit {
                version: 2,
                kind: "engine-permit".into(),
                launch_id: record.launch_id.clone(),
                grant_sha256: session.offer.grant_sha256.clone(),
            };
            if let Some(reason) = self.engine_gate(&mut record, session.pid)? {
                return Ok(Some(reason));
            }
            if now()? >= record.receipt.deadline_unix_ms {
                return Ok(Some(Outcome::Deadline));
            }
            staging::send(fd, &permit, &[]).map_err(|_| ErrorCode::Uncertain)?;
            let (started, reason) = self.film_wait(&record, || {
                staging::receive::<film::Permit>(fd).map_err(|_| ErrorCode::Uncertain)
            })?;
            let Some((started, rights)) = started else {
                return Ok(reason);
            };
            if !rights.is_empty()
                || started
                    != (film::Permit {
                        kind: "engine-started".into(),
                        ..permit
                    })
            {
                return Err(ErrorCode::Uncertain);
            }
            record.film.as_mut().ok_or(ErrorCode::Uncertain)?.phase = film::Phase::Engine;
            self.update(&record)?;
            drop(session.connection.take());
            loop {
                if !self.backend.live(&record)?.running {
                    return Ok(None);
                }
                if let Some(reason) = self.interrupted(&record)? {
                    return Ok(Some(reason));
                }
                thread::sleep(Duration::from_millis(50));
            }
        })();
        match execution {
            Ok(reason) => self.settle_inner(record, reason, Some(session)),
            Err(error) if record.manager_pending.is_some() => Err(error),
            Err(_) => self.settle_inner(record, Some(Outcome::Interrupted), Some(session)),
        }
    }
}