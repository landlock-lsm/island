// SPDX-License-Identifier: Apache-2.0 OR MIT

use serde::Deserialize;
use std::{collections::BTreeSet, path::PathBuf};

// Additional context properties will require TomlContextEntry to be replaced with ContextEntry.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContextEntry {
    pub when_beneath: Option<PathBuf>,
}

impl ContextEntry {
    /// Compares two context entries to determine their precedence.
    ///
    /// This is useful to detect and avoid useless contexts in the configuration.
    ///
    /// Returns which context should be kept.
    /// More specific `when_beneath` paths take precedence over less specific ones.
    fn compare_precedence(&self, other: &Self) -> Precedence {
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

#[derive(Debug, PartialEq, Eq)]
pub enum InsertResult {
    /// Context entry was successfully inserted and more specific context entries
    /// may have been removed.
    Swapped(BTreeSet<ContextEntry>),
    /// Context entry was ignored because an identical one already exists.
    IgnoredEqual(ContextEntry),
    /// Context entry was ignored because a less specific one already exists.
    IgnoredSubset(ContextEntry),
}

impl InsertResult {
    pub fn warning(&self, profile_name: &str) -> Option<String> {
        match self {
            InsertResult::Swapped(contexts) => {
                if contexts.is_empty() {
                    None
                } else {
                    Some(format!(
                        "profile \"{}\" has overlapping contexts: \
                        kept the more specific one and removed {:?}",
                        profile_name, contexts
                    ))
                }
            }
            InsertResult::IgnoredEqual(context) => Some(format!(
                "profile \"{}\" has duplicate contexts: ignoring {:?}",
                profile_name, context
            )),
            InsertResult::IgnoredSubset(context) => Some(format!(
                "profile \"{}\" has overlapping contexts: ignoring {:?}",
                profile_name, context
            )),
        }
    }
}

/// A custom set that automatically filters out superseded contexts when inserting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ContextSet {
    inner: BTreeSet<ContextEntry>,
}

impl ContextSet {
    /// Insert a context, automatically removing or ignoring any superseded contexts.
    ///
    /// Returns detailed information about the insertion result.
    pub fn insert(&mut self, other: ContextEntry) -> InsertResult {
        use std::ops::ControlFlow;
        enum BreakStatus {
            IgnoreEqual,
            IgnoreSubset,
        }

        // We could use BTreeSet::extract_if() instead (when stable enough).
        let status = self
            .inner
            .iter()
            // Only walk one time and stop ASAP.
            .try_fold(Vec::new(), |mut to_remove, existing| {
                match existing.compare_precedence(&other) {
                    Precedence::RemoveOne => ControlFlow::Break(BreakStatus::IgnoreEqual),
                    Precedence::RemoveOther => ControlFlow::Break(BreakStatus::IgnoreSubset),
                    Precedence::KeepBoth => ControlFlow::Continue(to_remove),
                    Precedence::RemoveSelf => {
                        to_remove.push(existing.clone());
                        ControlFlow::Continue(to_remove)
                    }
                }
            });

        match status {
            ControlFlow::Break(BreakStatus::IgnoreEqual) => InsertResult::IgnoredEqual(other),
            ControlFlow::Break(BreakStatus::IgnoreSubset) => InsertResult::IgnoredSubset(other),
            ControlFlow::Continue(to_remove) => {
                let mut removed_set = BTreeSet::new();
                for item in to_remove {
                    if self.inner.remove(&item) {
                        removed_set.insert(item);
                    } else {
                        // This branch is unreachable because item always comes from self.inner .
                        #[cfg(test)]
                        unreachable!();
                    }
                }
                self.inner.insert(other);
                InsertResult::Swapped(removed_set)
            }
        }
    }

    /// Returns an iterator over the context entries in the set.
    pub fn iter(&self) -> impl Iterator<Item = &ContextEntry> {
        self.inner.iter()
    }
}

impl IntoIterator for ContextSet {
    type Item = ContextEntry;
    type IntoIter = std::collections::btree_set::IntoIter<ContextEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
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
    fn test_insert_when_beneath() {
        let mut ctx_set = ContextSet::default();

        let ctx_empty = ContextEntry { when_beneath: None };
        assert_eq!(
            ctx_set.insert(ctx_empty.clone()),
            InsertResult::Swapped([].into())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_empty));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert the same value.
        assert_eq!(
            ctx_set.insert(ctx_empty.clone()),
            InsertResult::IgnoredEqual(ctx_empty.clone())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_empty));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert a value that supersedes the previous one.
        let ctx_foo_b = ContextEntry {
            when_beneath: Some("/foo/b".into()),
        };
        assert_eq!(
            ctx_set.insert(ctx_foo_b.clone()),
            InsertResult::Swapped([ctx_empty.clone()].into())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_foo_b));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert a sibling value, that should be ordered before the previous one.
        let ctx_foo_a = ContextEntry {
            when_beneath: Some("/foo/a".into()),
        };
        assert_eq!(
            ctx_set.insert(ctx_foo_a.clone()),
            InsertResult::Swapped([].into())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_foo_a));
        assert_eq!(ctx_iter.next(), Some(&ctx_foo_b));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert again an empty context.
        assert_eq!(
            ctx_set.insert(ctx_empty.clone()),
            InsertResult::IgnoredSubset(ctx_empty.clone())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_foo_a));
        assert_eq!(ctx_iter.next(), Some(&ctx_foo_b));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert a value that supersedes the previous one.
        let ctx_foo = ContextEntry {
            when_beneath: Some("/foo".into()),
        };
        assert_eq!(
            ctx_set.insert(ctx_foo.clone()),
            InsertResult::Swapped([ctx_foo_a.clone(), ctx_foo_b.clone()].into())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_foo));
        // Ignored ctx_foo
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert again a superseded when_beneath.
        assert_eq!(
            ctx_set.insert(ctx_foo_a.clone()),
            InsertResult::IgnoredSubset(ctx_foo_a.clone())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_foo));
        // Ignored ctx_foo_a
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert a duplicated value with when_beneath.
        assert_eq!(
            ctx_set.insert(ctx_foo.clone()),
            InsertResult::IgnoredEqual(ctx_foo.clone())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_foo));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert context for /bar .
        let ctx_bar = ContextEntry {
            when_beneath: Some("/bar".into()),
        };
        assert_eq!(
            ctx_set.insert(ctx_bar.clone()),
            InsertResult::Swapped([].into())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_bar));
        assert_eq!(ctx_iter.next(), Some(&ctx_foo));
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);

        // Insert a when_beneath superseded value.
        assert_eq!(
            ctx_set.insert(ctx_foo_a.clone()),
            InsertResult::IgnoredSubset(ctx_foo_a.clone())
        );
        let mut ctx_iter = ctx_set.iter();
        assert_eq!(ctx_iter.next(), Some(&ctx_bar));
        assert_eq!(ctx_iter.next(), Some(&ctx_foo));
        // Ignored ctx_foo_a .
        assert_eq!(ctx_iter.next(), None);
        drop(ctx_iter);
    }

    #[test]
    fn test_compare_precedence() {
        let ctx1 = ContextEntry { when_beneath: None };
        let ctx2 = ContextEntry { when_beneath: None };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry { when_beneath: None };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::RemoveOther);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::RemoveSelf);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            when_beneath: Some("/bar".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::KeepBoth);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::KeepBoth);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            when_beneath: Some("/foo/bar".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx2), Precedence::RemoveOther);
        assert_eq!(ctx2.compare_precedence(&ctx1), Precedence::RemoveSelf);

        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);

        let ctx1 = ContextEntry {
            when_beneath: Some("/foo".into()),
        };
        let ctx2 = ContextEntry {
            when_beneath: Some("/foo".into()),
        };
        assert_eq!(ctx1.compare_precedence(&ctx1), Precedence::RemoveOne);
        assert_eq!(ctx2.compare_precedence(&ctx2), Precedence::RemoveOne);
    }
}
