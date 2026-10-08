//! Exact Git 2.43/files/SHA-1 callback grammar, without authority or I/O.
// trace:BUG-1808 | ai:codex

use std::collections::{BTreeMap, BTreeSet};

const FRAME_LIMIT: usize = 1024 * 1024;
const REF_LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ObjectId([u8; 20]);

impl ObjectId {
    pub const ZERO: Self = Self([0; 20]);

    fn parse(bytes: &[u8]) -> Result<Self, InputError> {
        if bytes.len() != 40 {
            return Err(InputError::ObjectFormat);
        }
        let mut oid = [0; 20];
        for (pair, output) in bytes.chunks_exact(2).zip(oid.iter_mut()) {
            let digit = |byte| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(InputError::ObjectFormat),
            };
            *output = (digit(pair[0])? << 4) | digit(pair[1])?;
        }
        Ok(Self(oid))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RefName(Vec<u8>);

impl RefName {
    fn parse(bytes: &[u8]) -> Result<Self, InputError> {
        // Git 2.43's real merge-backend rebase also emits direct OID updates
        // for these per-worktree pseudorefs. Each still needs an exact target
        // in the prepared step; symbolic/admin file changes remain journaled.
        if matches!(
            bytes,
            b"HEAD" | b"ORIG_HEAD" | b"REBASE_HEAD" | b"CHERRY_PICK_HEAD"
        ) {
            return Ok(Self(bytes.to_vec()));
        }
        if !bytes.starts_with(b"refs/")
            || bytes.ends_with(b".")
            || bytes.windows(2).any(|w| w == b".." || w == b"@{")
            || bytes
                .iter()
                .any(|b| *b <= b' ' || *b == 127 || b"~^:?*[\\".contains(b))
            || bytes
                .split(|b| *b == b'/')
                .any(|part| part.is_empty() || part.starts_with(b".") || part.ends_with(b".lock"))
        {
            return Err(InputError::RefName);
        }
        Ok(Self(bytes.to_vec()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefPhase {
    Prepared,
    Committed,
    Aborted,
}

impl RefPhase {
    fn parse(argument: &[u8]) -> Result<Self, InputError> {
        match argument {
            b"prepared" => Ok(Self::Prepared),
            b"committed" => Ok(Self::Committed),
            b"aborted" => Ok(Self::Aborted),
            _ => Err(InputError::Phase),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawRefUpdate {
    /// Preserve the actual Git input, including ambiguous all-zero old OIDs.
    pub raw_old: ObjectId,
    pub new: ObjectId,
    pub name: RefName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RefInput {
    pub phase: RefPhase,
    pub updates: Vec<RawRefUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum InputError {
    #[error("Git callback exceeds its finite input budget")]
    Budget,
    #[error("Git callback input is incomplete or malformed")]
    Framing,
    #[error("Git callback object does not match the SHA-1 profile")]
    ObjectFormat,
    #[error("Git callback names an unsupported reference")]
    RefName,
    #[error("Git callback phase does not match the selected profile")]
    Phase,
    #[error("Git callback repeats a reference")]
    DuplicateRef,
    #[error("Git callback does not match the complete prepared reference set")]
    TargetSet,
    #[error("Git callback does not match the exact prepared transition")]
    Transition,
    #[error("shared Git transition lacks an explicit expected-old value")]
    UnconstrainedSharedUpdate,
    #[error("Git rewrite callback kind is unsupported")]
    RewriteKind,
}

/// Typed parsing only. A result must not be stored as authenticated evidence.
pub(super) fn parse_reference(argument: &[u8], stdin: &[u8]) -> Result<RefInput, InputError> {
    let phase = RefPhase::parse(argument)?;
    let lines = complete_lines(stdin)?;
    let mut names = BTreeSet::new();
    let mut updates = Vec::new();
    for line in lines {
        if updates.len() == REF_LIMIT {
            return Err(InputError::Budget);
        }
        let (old, new, name) = three_fields(line)?;
        let name = RefName::parse(name)?;
        if !names.insert(name.clone()) {
            return Err(InputError::DuplicateRef);
        }
        updates.push(RawRefUpdate {
            raw_old: ObjectId::parse(old)?,
            new: ObjectId::parse(new)?,
            name,
        });
    }
    if updates.is_empty() {
        return Err(InputError::Framing);
    }
    Ok(RefInput { phase, updates })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefDomain {
    Shared,
    /// Only a sealed exclusive starting-ref map permits interpreting raw zero.
    PrivateExclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExpectedUpdate {
    pub old: ObjectId,
    pub new: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BoundRefUpdate {
    pub raw: RawRefUpdate,
    pub effective_old: ObjectId,
}

/// Check the entire declared tuple set, never a prefix or merely refs/aida.
/// This checks data consistency only, not peer/domain/step authorization.
pub(super) fn match_exact_transitions(
    input: &RefInput,
    expected: &BTreeMap<RefName, ExpectedUpdate>,
    domain: RefDomain,
) -> Result<Vec<BoundRefUpdate>, InputError> {
    if input.updates.len() != expected.len() {
        return Err(InputError::TargetSet);
    }
    let mut result = Vec::with_capacity(input.updates.len());
    let mut seen = BTreeSet::new();
    for update in &input.updates {
        if !seen.insert(&update.name) {
            return Err(InputError::DuplicateRef);
        }
        let target = expected.get(&update.name).ok_or(InputError::TargetSet)?;
        if update.new != target.new {
            return Err(InputError::Transition);
        }
        if update.raw_old != target.old {
            if update.raw_old != ObjectId::ZERO {
                return Err(InputError::Transition);
            }
            if domain == RefDomain::Shared {
                return Err(InputError::UnconstrainedSharedUpdate);
            }
        }
        result.push(BoundRefUpdate {
            raw: update.clone(),
            effective_old: target.old,
        });
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RewriteKind {
    Amend,
    Rebase,
}

/// Ordered pairs are retained, including many old commits mapped to one new
/// commit. This is one bounded frame; map completion/count/digest is separate.
pub(super) fn parse_rewrite(
    argument: &[u8],
    stdin: &[u8],
) -> Result<(RewriteKind, Vec<(ObjectId, ObjectId)>), InputError> {
    let kind = match argument {
        b"amend" => RewriteKind::Amend,
        b"rebase" => RewriteKind::Rebase,
        _ => return Err(InputError::RewriteKind),
    };
    let mut pairs = Vec::new();
    for line in complete_lines(stdin)? {
        if pairs.len() == REF_LIMIT {
            return Err(InputError::Budget);
        }
        let mut fields = line.split(|b| *b == b' ');
        let old = ObjectId::parse(fields.next().ok_or(InputError::Framing)?)?;
        let new = ObjectId::parse(fields.next().ok_or(InputError::Framing)?)?;
        if fields.next().is_some() || old == ObjectId::ZERO || new == ObjectId::ZERO {
            return Err(InputError::Framing);
        }
        pairs.push((old, new));
    }
    Ok((kind, pairs))
}

fn complete_lines(stdin: &[u8]) -> Result<impl Iterator<Item = &[u8]>, InputError> {
    if stdin.len() > FRAME_LIMIT {
        return Err(InputError::Budget);
    }
    if !stdin.is_empty() && !stdin.ends_with(b"\n") {
        return Err(InputError::Framing);
    }
    Ok(stdin
        .split_inclusive(|b| *b == b'\n')
        .map(|line| &line[..line.len() - 1]))
}

fn three_fields(line: &[u8]) -> Result<(&[u8], &[u8], &[u8]), InputError> {
    let mut fields = line.split(|b| *b == b' ');
    let first = fields.next().ok_or(InputError::Framing)?;
    let second = fields.next().ok_or(InputError::Framing)?;
    let third = fields.next().ok_or(InputError::Framing)?;
    if fields.next().is_some() || first.is_empty() || second.is_empty() || third.is_empty() {
        return Err(InputError::Framing);
    }
    Ok((first, second, third))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(digit: char) -> String {
        digit.to_string().repeat(40)
    }

    fn tuple(old: char, new: char, name: &str) -> String {
        format!("{} {} {name}\n", oid(old), oid(new))
    }

    fn expectations(input: &RefInput) -> BTreeMap<RefName, ExpectedUpdate> {
        input
            .updates
            .iter()
            .map(|u| {
                (
                    u.name.clone(),
                    ExpectedUpdate {
                        old: u.raw_old,
                        new: u.new,
                    },
                )
            })
            .collect()
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn exact_full_tuple_set_and_all_supported_phases() {
        let data = tuple('1', '2', "refs/heads/topic") + &tuple('0', '3', "refs/aida/retained/pin");
        for phase in [b"prepared".as_slice(), b"committed", b"aborted"] {
            let input = parse_reference(phase, data.as_bytes()).unwrap();
            let expected = expectations(&input);
            assert_eq!(
                match_exact_transitions(&input, &expected, RefDomain::Shared)
                    .unwrap()
                    .len(),
                2
            );
            let mut missing = expected.clone();
            missing.pop_last();
            assert_eq!(
                match_exact_transitions(&input, &missing, RefDomain::Shared),
                Err(InputError::TargetSet)
            );
            let mut wrong = expected.clone();
            wrong.values_mut().next().unwrap().new = ObjectId::ZERO;
            assert_eq!(
                match_exact_transitions(&input, &wrong, RefDomain::Shared),
                Err(InputError::Transition)
            );
        }
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn zero_old_is_not_shared_creation_proof() {
        let genuine =
            parse_reference(b"prepared", tuple('1', '2', "refs/heads/topic").as_bytes()).unwrap();
        let expected = expectations(&genuine);
        let unconstrained =
            parse_reference(b"prepared", tuple('0', '2', "refs/heads/topic").as_bytes()).unwrap();
        assert_eq!(
            match_exact_transitions(&unconstrained, &expected, RefDomain::Shared),
            Err(InputError::UnconstrainedSharedUpdate)
        );
        let bound = match_exact_transitions(&unconstrained, &expected, RefDomain::PrivateExclusive)
            .unwrap();
        assert_eq!(bound[0].raw.raw_old, ObjectId::ZERO);
        assert_eq!(bound[0].effective_old, genuine.updates[0].raw_old);
        assert!(match_exact_transitions(&genuine, &expected, RefDomain::Shared).is_ok());
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn malformed_partial_duplicate_and_newer_profile_inputs_refuse() {
        let valid = tuple('1', '2', "refs/heads/topic");
        assert!(parse_reference(b"prepared", valid.as_bytes()).is_ok());
        assert_eq!(
            parse_reference(b"preparing", valid.as_bytes()),
            Err(InputError::Phase)
        );
        assert_eq!(
            parse_reference(b"prepared", &valid.as_bytes()[..valid.len() - 1]),
            Err(InputError::Framing)
        );
        assert_eq!(
            parse_reference(b"prepared", (valid.clone() + &valid).as_bytes()),
            Err(InputError::DuplicateRef)
        );
        assert_eq!(parse_reference(b"prepared", b""), Err(InputError::Framing));
        assert!(parse_reference(
            b"prepared",
            format!("{} {} refs/heads/topic\n", "a".repeat(64), oid('2')).as_bytes()
        )
        .is_err());
        assert!(parse_reference(
            b"prepared",
            b"ref:refs/heads/main ref:refs/heads/topic HEAD\n"
        )
        .is_err());
        assert!(parse_reference(b"prepared", &vec![b'a'; FRAME_LIMIT + 1]).is_err());
        let many: String = (0..=REF_LIMIT)
            .map(|n| tuple('0', '1', &format!("refs/heads/t{n}")))
            .collect();
        assert_eq!(
            parse_reference(b"prepared", many.as_bytes()),
            Err(InputError::Budget)
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn literal_ref_names_and_rewrite_order_are_preserved() {
        for name in [
            "refs/heads/a..b",
            "refs/heads/.hidden",
            "refs/heads/a.lock",
            "refs/heads/a@{b",
            "refs//a",
            "refs/a/",
            "refs/a:",
            "refs/a\\b",
            "UNKNOWN_HEAD",
        ] {
            assert!(RefName::parse(name.as_bytes()).is_err(), "{name}");
        }
        for name in [
            b"HEAD".as_slice(),
            b"ORIG_HEAD",
            b"REBASE_HEAD",
            b"CHERRY_PICK_HEAD",
        ] {
            assert!(RefName::parse(name).is_ok());
            let input = parse_reference(
                b"prepared",
                tuple('0', '1', std::str::from_utf8(name).unwrap()).as_bytes(),
            )
            .unwrap();
            assert!(
                match_exact_transitions(&input, &expectations(&input), RefDomain::Shared).is_ok()
            );
        }
        assert!(RefName::parse(b"refs/heads/\xff").is_ok());
        let data = format!("{} {}\n{} {}\n", oid('1'), oid('3'), oid('2'), oid('3'));
        let (kind, pairs) = parse_rewrite(b"rebase", data.as_bytes()).unwrap();
        assert_eq!(kind, RewriteKind::Rebase);
        assert_eq!(pairs.len(), 2);
        assert_ne!(pairs[0].0, pairs[1].0);
        assert_eq!(pairs[0].1, pairs[1].1);
        assert!(
            parse_rewrite(b"rebase", format!("{} {}\n", oid('0'), oid('1')).as_bytes()).is_err()
        );
        assert!(parse_rewrite(
            b"rebase",
            format!("{} {} extra\n", oid('1'), oid('2')).as_bytes()
        )
        .is_err());
    }
}
