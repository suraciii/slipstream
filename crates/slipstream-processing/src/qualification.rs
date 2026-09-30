use super::{Record, Registry};
use crate::{film, protocol::*, qualified};

pub(super) fn availability(
    ready: bool,
    cases: &[qualified::Case],
    envelope: &str,
    invalidations: &[qualified::Invalidation],
) -> qualified::Availability {
    use qualified::{Availability, Case};
    if !ready {
        Availability::Blocked
    } else if cases.iter().any(|case| {
        matches!(case, Case::Qualified { .. })
            && !invalidations
                .iter()
                .any(|e| e.envelope == envelope && e.fixture_id == case.fixture_id())
    }) {
        Availability::Available
    } else {
        Availability::Unqualified
    }
}

pub(super) fn plan(
    documents: &qualified::Documents,
    invalidations: &[qualified::Invalidation],
    fixture: &str,
    envelope: &str,
    memory: u64,
) -> Result<(film::Fixture, qualified::Plan), ErrorCode> {
    if invalidations.len() >= qualified::INVALIDATIONS {
        return Err(ErrorCode::Capacity);
    }
    if invalidations
        .iter()
        .any(|e| e.envelope == envelope && e.fixture_id == fixture)
    {
        return Err(ErrorCode::UnqualifiedEnvelope);
    }
    documents.plan(fixture, envelope, memory)
}

pub(super) fn attempt_oom_killed(evidence: &Evidence) -> bool {
    evidence
        .attempt_before
        .as_ref()
        .zip(evidence.attempt_after.as_ref())
        .is_some_and(|(before, after)| {
            after
                .oom_kill
                .checked_sub(before.oom_kill)
                .is_some_and(|delta| delta > 0)
        })
}

pub(super) fn owned_limit_pressure(evidence: &Evidence) -> bool {
    evidence
        .attempt_before
        .as_ref()
        .zip(evidence.attempt_after.as_ref())
        .is_some_and(|(before, after)| after.oom.checked_sub(before.oom).is_some_and(|n| n > 0))
        || evidence
            .parent_before
            .as_ref()
            .zip(evidence.parent_after.as_ref())
            .is_some_and(|(before, after)| {
                after
                    .local_oom
                    .checked_sub(before.local_oom)
                    .is_some_and(|n| n > 0)
            })
}

pub(super) fn failure(record: &Record) -> Option<qualified::QualificationFailure> {
    use qualified::QualificationFailure as F;
    let captured = record.film.as_ref()?;
    let plan = captured.grant.plan.qualified()?;
    if captured.qualification_observation_valid != Some(true) {
        return None;
    }
    let outcome = record.receipt.outcome?;
    let evidence = record.receipt.evidence.as_ref()?;
    if evidence.populated != Some(false) {
        return None;
    }
    if record.cgroup_inode.is_some()
        && record.unit_invocation.is_some()
        && evidence.peak_bytes > plan.empirical_ceiling_bytes
    {
        return Some(F::PeakExceeded);
    }
    if record.cgroup_inode.is_some()
        && record.unit_invocation.is_some()
        && attempt_oom_killed(evidence)
        && owned_limit_pressure(evidence)
    {
        return Some(F::ProcessingOom);
    }
    (outcome == Outcome::AllocationFailed).then_some(F::AllocationFailed)
}

pub(super) fn assess(registry: &mut Registry, record: &mut Record) -> Result<(), ErrorCode> {
    let reason = failure(record);
    let Some(captured) = record.film.as_mut() else {
        return Ok(());
    };
    if captured.grant.plan.qualified().is_none() {
        return Ok(());
    }
    if captured.qualification_failure.is_some() && captured.qualification_failure != reason {
        return Err(ErrorCode::Uncertain);
    }
    captured.qualification_failure = reason;
    let Some(reason) = reason else {
        return Ok(());
    };
    let invalidations = registry
        .invalidations
        .as_mut()
        .ok_or(ErrorCode::Uncertain)?;
    if let Some(existing) = invalidations.iter().find(|e| {
        e.envelope == captured.resource_model && e.fixture_id == captured.grant.fixture.id
    }) {
        if existing.reason != reason
            || existing.incarnation != record.receipt.incarnation
            || existing.sequence != record.receipt.sequence
        {
            return Err(ErrorCode::Uncertain);
        }
    } else {
        if invalidations.len() >= qualified::INVALIDATIONS {
            return Err(ErrorCode::Capacity);
        }
        invalidations.push(qualified::Invalidation {
            envelope: captured.resource_model.clone(),
            fixture_id: captured.grant.fixture.id.clone(),
            reason,
            incarnation: record.receipt.incarnation.clone(),
            sequence: record.receipt.sequence,
        });
    }
    Ok(())
}

pub(super) fn restore(registry: &mut Registry) -> Result<(), ErrorCode> {
    let mut next = registry.clone();
    for mut record in registry.records.values().cloned() {
        assess(&mut next, &mut record)?;
        next.records.insert(record.receipt.sequence, record);
    }
    *registry = next;
    Ok(())
}
