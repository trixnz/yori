//! Workspace tab identity and lifetime, independent of rendering.

use std::{path::Path, sync::Arc};

use crate::{comparison::Comparison, review::ReviewSourceIdentity};

#[derive(Clone, Debug)]
pub(super) enum TabIdentity {
    Comparison(Comparison),
    Review {
        identity: ReviewSourceIdentity,
        label: Arc<str>,
    },
}

impl PartialEq for TabIdentity {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Comparison(left), Self::Comparison(right)) => left == right,
            (
                Self::Review { identity: left, .. },
                Self::Review {
                    identity: right, ..
                },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for TabIdentity {}

impl TabIdentity {
    pub(super) fn comparison(&self) -> Option<&Comparison> {
        match self {
            Self::Comparison(comparison) => Some(comparison),
            Self::Review { .. } => None,
        }
    }

    pub(super) fn description(&self) -> String {
        match self {
            Self::Comparison(comparison) => comparison.description(),
            Self::Review { identity, .. } => identity.description(),
        }
    }
}

pub(super) struct Tab<T> {
    pub id: usize,
    pub identity: TabIdentity,
    pub content: T,
}

pub(super) struct Tabs<T> {
    pub entries: Vec<Tab<T>>,
    pub active: Option<usize>,
    next_id: usize,
}

impl<T> Default for Tabs<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            active: None,
            next_id: 0,
        }
    }
}

impl<T> Tabs<T> {
    pub fn find(&self, identity: &TabIdentity) -> Option<usize> {
        self.entries
            .iter()
            .find(|tab| &tab.identity == identity)
            .map(|tab| tab.id)
    }

    pub fn get(&self, id: usize) -> Option<&Tab<T>> {
        self.entries.iter().find(|tab| tab.id == id)
    }

    pub fn activate(&mut self, id: usize) {
        if self.get(id).is_some() {
            self.active = Some(id);
        }
    }

    /// Duplicate opens preserve the existing content rather than replacing it.
    pub fn insert(&mut self, identity: TabIdentity, content: T) -> usize {
        if let Some(id) = self.find(&identity) {
            self.activate(id);
            return id;
        }

        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Tab {
            id,
            identity,
            content,
        });
        self.active = Some(id);

        id
    }

    /// Closing the active tab chooses its right neighbor, then its left neighbor.
    pub fn remove(&mut self, id: usize) {
        let Some(index) = self.entries.iter().position(|tab| tab.id == id) else {
            return;
        };

        self.entries.remove(index);
        if self.active == Some(id) {
            self.active = self
                .entries
                .get(index)
                .or_else(|| self.entries.last())
                .map(|tab| tab.id);
        }
    }

    pub fn requires_discard_confirmation(
        &self,
        target: Option<usize>,
        modified: impl Fn(&T) -> bool,
    ) -> bool {
        match target {
            Some(id) => self.get(id).is_some_and(|tab| modified(&tab.content)),
            None => self.entries.iter().any(|tab| modified(&tab.content)),
        }
    }

    pub fn label(&self, id: usize) -> String {
        let tab = self.get(id).expect("label requested for an existing tab");
        let TabIdentity::Comparison(comparison) = &tab.identity else {
            let TabIdentity::Review { label, .. } = &tab.identity else {
                unreachable!();
            };
            return label.to_string();
        };
        let path = comparison.target();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let matching_names = self
            .entries
            .iter()
            .filter_map(|other| other.identity.comparison())
            .filter(|other| other.target().file_name() == path.file_name())
            .count();
        if matching_names == 1 {
            return name.into_owned();
        }

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let label = format!("{name} — {}", parent.display());
        let matching_locals = self
            .entries
            .iter()
            .filter_map(|other| other.identity.comparison())
            .filter(|other| other.target() == path)
            .count();
        if matching_locals > 1 {
            format!("{label} ← {}", comparison.qualifier())
        } else {
            label
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(left: &str, right: &str) -> TabIdentity {
        TabIdentity::Comparison(Comparison::diff(left.into(), right.into()))
    }

    #[test]
    fn closing_tabs_keeps_stable_identity_and_selects_a_neighbor() {
        let mut tabs = Tabs::default();
        let a = tabs.insert(pair("a", "a"), ());
        let b = tabs.insert(pair("b", "b"), ());
        let c = tabs.insert(pair("c", "c"), ());
        tabs.activate(b);

        tabs.remove(a);
        assert_eq!(tabs.active, Some(b));

        tabs.remove(b);
        assert_eq!(tabs.active, Some(c));

        tabs.remove(c);
        assert_eq!(tabs.active, None);
        assert!(tabs.entries.is_empty());

        let next = tabs.insert(pair("d", "d"), ());
        assert!(next > c);
        tabs.remove(b);
        assert_eq!(tabs.active, Some(next));
    }

    #[test]
    fn closing_the_window_checks_inactive_tabs_and_cancellation_keeps_them() {
        let mut tabs = Tabs::default();
        let modified = tabs.insert(pair("a", "a"), true);
        let clean = tabs.insert(pair("b", "b"), false);

        assert_eq!(tabs.active, Some(clean));
        assert!(!tabs.requires_discard_confirmation(Some(clean), |dirty| *dirty));
        assert!(tabs.requires_discard_confirmation(Some(modified), |dirty| *dirty));
        assert!(tabs.requires_discard_confirmation(None, |dirty| *dirty));
        assert_eq!(tabs.entries.len(), 2);
        assert!(tabs.get(modified).unwrap().content);

        tabs.remove(modified);
        assert_eq!(tabs.active, Some(clean));
        assert!(!tabs.requires_discard_confirmation(None, |dirty| *dirty));
    }

    #[test]
    fn labels_disambiguate_filenames_and_multiple_baselines() {
        let mut tabs = Tabs::default();
        let first = tabs.insert(pair("/base/one", "/one/parser.rs"), ());
        assert_eq!(tabs.label(first), "parser.rs");

        let second = tabs.insert(pair("/base/two", "/two/parser.rs"), ());
        assert_eq!(tabs.label(first), "parser.rs — /one");
        assert_eq!(tabs.label(second), "parser.rs — /two");

        tabs.insert(pair("/base/three", "/one/parser.rs"), ());
        assert_eq!(tabs.label(first), "parser.rs — /one ← /base/one");
        assert_ne!(
            tabs.find(&pair("/base/one", "/one/parser.rs")),
            tabs.find(&pair("/one/parser.rs", "/base/one"))
        );
    }

    #[test]
    fn review_identity_deduplicates_independently_of_its_display_label() {
        let mut tabs = Tabs::default();
        let identity = ReviewSourceIdentity::new("test", "working");
        let first = tabs.insert(
            TabIdentity::Review {
                identity: identity.clone(),
                label: "Working changes".into(),
            },
            "first",
        );
        let duplicate = tabs.insert(
            TabIdentity::Review {
                identity,
                label: "Changed label".into(),
            },
            "replacement",
        );

        assert_eq!(duplicate, first);
        assert_eq!(tabs.entries.len(), 1);
        assert_eq!(tabs.get(first).unwrap().content, "first");
        assert_eq!(tabs.label(first), "Working changes");
    }

    #[test]
    fn merge_identity_includes_all_roles_and_preserves_independent_tabs() {
        let paths = crate::comparison::MergePaths {
            base: "/base.rs".into(),
            local: "/local.rs".into(),
            incoming: "/incoming.rs".into(),
            result: "/result.rs".into(),
        };
        let original = TabIdentity::Comparison(Comparison::Merge(paths.clone()));
        let mut tabs = Tabs::default();
        let first = tabs.insert(original.clone(), "edited first");
        let diff = tabs.insert(pair("/base.rs", "/result.rs"), "edited diff");

        for variant in [
            crate::comparison::MergePaths {
                base: "/other-base.rs".into(),
                ..paths.clone()
            },
            crate::comparison::MergePaths {
                local: paths.incoming.clone(),
                incoming: paths.local.clone(),
                ..paths.clone()
            },
            crate::comparison::MergePaths {
                incoming: "/other-incoming.rs".into(),
                ..paths.clone()
            },
            crate::comparison::MergePaths {
                result: "/other-result.rs".into(),
                ..paths.clone()
            },
        ] {
            let id = tabs.insert(
                TabIdentity::Comparison(Comparison::Merge(variant)),
                "another merge",
            );
            assert_ne!(id, first);
            assert_ne!(id, diff);
        }

        assert_eq!(tabs.insert(original, "replacement must be ignored"), first);
        assert_eq!(tabs.entries.len(), 6);
        assert_eq!(tabs.get(first).unwrap().content, "edited first");
        assert_eq!(tabs.get(diff).unwrap().content, "edited diff");
        assert_ne!(tabs.label(first), tabs.label(diff));
    }
}
