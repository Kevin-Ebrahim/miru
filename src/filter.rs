use std::{cell::RefCell, sync::Arc};

#[derive(Clone)]
struct Selection {
    query: String,
    exact: bool,
    accepted_query: Option<String>,
    indices: Arc<[usize]>,
}

/// Owns title normalization and invalidates cached indices whenever data changes.
#[derive(Clone)]
pub struct TitleList<T> {
    items: Vec<T>,
    titles: Vec<String>,
    normalize: fn(&str) -> String,
    selection: RefCell<Option<Selection>>,
}

impl<T> Default for TitleList<T> {
    fn default() -> Self {
        Self::new(crate::model::normalized)
    }
}

impl<T> TitleList<T> {
    pub fn new(normalize: fn(&str) -> String) -> Self {
        Self {
            items: vec![],
            titles: vec![],
            normalize,
            selection: RefCell::new(None),
        }
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn replace(&mut self, items: Vec<T>, title: impl Fn(&T) -> &str) {
        self.titles = items
            .iter()
            .map(|item| (self.normalize)(title(item)))
            .collect();

        self.items = items;
        *self.selection.get_mut() = None;
    }

    pub fn visible(&self, query: &str, exact: bool, accepted_query: Option<&str>) -> Arc<[usize]> {
        let mut selection = self.selection.borrow_mut();

        if let Some(cached) = selection.as_ref()
            && cached.query == query
            && cached.exact == exact
            && cached.accepted_query.as_deref() == accepted_query
        {
            return cached.indices.clone();
        }

        let normalized = (self.normalize)(query);
        let include_all_titles =
            !exact && accepted_query.is_some_and(|source| (self.normalize)(source) == normalized);
        let indices: Arc<[usize]> = self
            .titles
            .iter()
            .enumerate()
            .filter_map(|(index, title)| {
                let matches_query = include_all_titles
                    || if exact {
                        *title == normalized
                    } else {
                        title.contains(&normalized)
                    };

                matches_query.then_some(index)
            })
            .collect();

        *selection = Some(Selection {
            query: query.into(),
            exact,
            accepted_query: accepted_query.map(str::to_owned),
            indices: indices.clone(),
        });

        indices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_refresh_when_data_query_or_match_mode_changes() {
        let mut list = TitleList::default();
        list.replace(vec!["Series", "Other"], |title| title);
        assert_eq!(&*list.visible(" series ", true, None), &[0]);
        assert_eq!(&*list.visible("series", false, Some("SERIES")), &[0, 1]);
        assert_eq!(&*list.visible("other", false, None), &[1]);
        list.replace(vec!["Other", "Series"], |title| title);
        assert_eq!(&*list.visible("other", false, None), &[0]);
        list.replace(vec![], |title| title);
        assert!(list.visible("other", false, None).is_empty());
    }
}
