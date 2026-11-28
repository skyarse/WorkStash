use crate::{kind_name, library::Entry};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Scope {
    #[default]
    All,
    Favorites,
    Section(i64),
}

impl Scope {
    pub fn section(self) -> Option<i64> {
        match self {
            Self::Section(id) => Some(id),
            _ => None,
        }
    }

    fn includes(self, entry: &Entry) -> bool {
        match self {
            Self::All => true,
            Self::Favorites => entry.favorite,
            Self::Section(id) => entry.section == id,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Title,
}

impl Sort {
    pub const OPTIONS: [Self; 3] = [Self::Newest, Self::Oldest, Self::Title];

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Сначала новые",
            Self::Oldest => "Сначала старые",
            Self::Title => "По названию",
        }
    }
}

#[derive(Default)]
pub struct Catalog {
    scope: Scope,
    sort: Sort,
    kind: Option<String>,
    query: String,
    valid: bool,
    pub kinds: Vec<String>,
    pub rows: Vec<usize>,
}

impl Catalog {
    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    pub fn refresh(
        &mut self,
        entries: &[Entry],
        scope: Scope,
        sort: Sort,
        kind: &mut Option<String>,
        query: &str,
    ) -> bool {
        let query = query.trim();
        if self.valid
            && self.scope == scope
            && self.sort == sort
            && &self.kind == kind
            && self.query == query
        {
            return false;
        }

        if !self.valid || self.scope != scope {
            let kinds: BTreeSet<_> = entries
                .iter()
                .filter(|entry| scope.includes(entry))
                .map(|entry| entry.kind.as_str())
                .collect();
            self.kinds = kinds.into_iter().map(str::to_owned).collect();
        }
        if kind.as_ref().is_some_and(|kind| !self.kinds.contains(kind)) {
            *kind = None;
        }

        let needle = query.to_lowercase();
        self.rows.clear();
        self.rows.extend(
            entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    scope.includes(entry)
                        && kind.as_ref().is_none_or(|kind| kind == &entry.kind)
                        && (needle.is_empty()
                            || [
                                entry.title.as_str(),
                                entry.kind.as_str(),
                                kind_name(&entry.kind),
                                entry.body.as_str(),
                            ]
                            .iter()
                            .any(|text| text.to_lowercase().contains(&needle)))
                })
                .map(|(index, _)| index),
        );
        match sort {
            Sort::Newest => {}
            Sort::Oldest => self.rows.reverse(),
            Sort::Title => self
                .rows
                .sort_by_cached_key(|&index| entries[index].title.to_lowercase()),
        }
        self.scope = scope;
        self.sort = sort;
        self.kind.clone_from(kind);
        self.query.clear();
        self.query.push_str(query);
        self.valid = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use anyhow::Result;

    #[test]
    fn search_sort_and_favorites_keep_material_order_until_inputs_change() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let library = Library::open(folder.path())?;
        let section = library.add_section("Материалы")?;
        let one = library.save_text(None, section, "Заметка", "Абрикос", "Пример")?;
        let two = library.save_text(None, 1, "Промпт", "Яблоко", "пример")?;
        library.save_text(None, 1, "Заметка", "Ананас", "Другое")?;
        library.set_favorite(one, true)?;
        library.set_favorite(two, true)?;
        let entries = library.entries()?;
        let mut catalog = Catalog::default();
        let mut kind = None;
        assert!(catalog.refresh(&entries, Scope::Favorites, Sort::Title, &mut kind, "ПРИМЕР"));
        assert_eq!(
            catalog
                .rows
                .iter()
                .map(|&i| entries[i].id)
                .collect::<Vec<_>>(),
            [one, two]
        );
        assert!(!catalog.refresh(
            &entries,
            Scope::Favorites,
            Sort::Title,
            &mut kind,
            " ПРИМЕР "
        ));
        kind = Some("Промпт".into());
        assert!(catalog.refresh(&entries, Scope::Favorites, Sort::Title, &mut kind, "ПРИМЕР"));
        assert_eq!(catalog.rows.len(), 1);
        assert_eq!(entries[catalog.rows[0]].id, two);
        assert!(catalog.refresh(
            &entries,
            Scope::Section(section),
            Sort::Title,
            &mut kind,
            "ПРИМЕР"
        ));
        assert_eq!(kind, None);
        assert_eq!(entries[catalog.rows[0]].id, one);
        Ok(())
    }
}
