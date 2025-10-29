// SPDX-License-Identifier: Apache-2.0 OR MIT

use serde::Deserialize;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContextEntry {
    // TODO: Restrict profile's name.
    pub profile: String,
    pub when_beneath: Option<PathBuf>,
}

impl ContextEntry {
    /// Compares two context entries with the same profile to determine their precedence.
    ///
    /// This is useful to detect and avoid useless contexts in the configuration.
    ///
    /// Returns which context should be kept when both contexts have the same profile.
    /// More specific `when_beneath` paths take precedence over less specific ones.
    fn compare_precedence(&self, other: &Self) -> Precedence {
        if self.profile != other.profile {
            return Precedence::KeepBoth;
        }

        // We should not check the actual LandlockConfig object because two
        // different profiles could legitimately have the exact same resolved
        // configuration (e.g. because managed by different owners).
        //
        // We should not check the canonicalized profile path because it could lead
        // to impure errors (i.e. not directly related to the content of main.toml).
        match (&self.when_beneath, &other.when_beneath) {
            (None, None) => Precedence::RemoveOne,
            (Some(_), None) => Precedence::RemoveOther,
            (None, Some(_)) => Precedence::RemoveSelf,
            (Some(s), Some(o)) => match (s.starts_with(o), o.starts_with(s)) {
                (false, false) => Precedence::KeepBoth,
                (false, true) => Precedence::RemoveOther,
                (true, false) => Precedence::RemoveSelf,
                (true, true) => Precedence::RemoveOne,
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Precedence {
    KeepBoth,
    RemoveOne,
    RemoveSelf,
    RemoveOther,
}

#[derive(Debug)]
pub enum InsertResult {
    /// A more specific context entry was inserted, removing a less specific one.
    RemovedSubset(ContextEntry),
    /// Context entry was ignored because an identical one already exists.
    IgnoredEqual(ContextEntry),
    /// Context entry was ignored because a more specific one already exists.
    IgnoredSubset(ContextEntry),
    /// Context entry was successfully inserted.
    Inserted,
}

impl InsertResult {
    pub fn warning(&self) -> Option<String> {
        match self {
            InsertResult::RemovedSubset(context) => Some(format!(
                "profile \"{}\" has overlapping contexts: kept more specific one",
                context.profile
            )),
            InsertResult::IgnoredEqual(context) => Some(format!(
                "profile \"{}\" has duplicate contexts: ignoring one",
                context.profile
            )),
            InsertResult::IgnoredSubset(context) => Some(format!(
                "profile \"{}\" has overlapping contexts: ignoring less specific one",
                context.profile
            )),
            InsertResult::Inserted => None,
        }
    }
}

/// A custom set that automatically filters out superseded contexts when inserting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ContextSet {
    inner: BTreeSet<ContextEntry>,
}

impl ContextSet {
    /// Insert a context, automatically removing any superseded contexts.
    /// Returns detailed information about the insertion result.
    pub fn insert(&mut self, context: ContextEntry) -> InsertResult {
        let mut contexts_to_remove = Vec::new();

        for existing in &self.inner {
            match context.compare_precedence(existing) {
                Precedence::KeepBoth => continue,
                Precedence::RemoveOne => {
                    return InsertResult::IgnoredEqual(context);
                }
                Precedence::RemoveSelf => {
                    return InsertResult::IgnoredSubset(context);
                }
                Precedence::RemoveOther => {
                    contexts_to_remove.push(existing.clone());
                }
            }
        }

        if !contexts_to_remove.is_empty() {
            let removed = contexts_to_remove[0].clone();
            for to_remove in contexts_to_remove {
                self.inner.remove(&to_remove);
            }
            self.inner.insert(context);
            InsertResult::RemovedSubset(removed)
        } else {
            self.inner.insert(context);
            InsertResult::Inserted
        }
    }

    /// Returns an iterator over the context entries in the set.
    pub fn iter(&self) -> impl Iterator<Item = &ContextEntry> {
        self.inner.iter()
    }
}

impl<'a> IntoIterator for &'a ContextSet {
    type Item = &'a ContextEntry;
    type IntoIter = std::collections::btree_set::Iter<'a, ContextEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_precedence() {
        let ctx1 = ContextEntry {
            profile: "a".to_string(),
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            profile: "b".to_string(),
            when_beneath: Some("/foo".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::KeepBoth);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::KeepBoth);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: None,
        };
        let ctx2 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: None,
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: None,
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::RemoveOther);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::RemoveSelf);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/bar".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::KeepBoth);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::KeepBoth);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/foo/bar".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::RemoveOther);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::RemoveSelf);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            profile: "test".to_string(),
            when_beneath: Some("/foo".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);
    }
}
