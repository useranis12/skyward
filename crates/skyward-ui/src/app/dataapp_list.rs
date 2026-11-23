//! The apps the owner installed, as the pages read them: the list, the name
//! each one is called, and the page that opens one.
//!
//! Ported from skyward-pudding v0.3.15.

use crate::app::App;
use crate::app::SearchScope;
use crate::app::dataapp::DataApp;

impl App {
    /// The first entry on a page that answers `query`: apps by their name or
    /// package, modules by their name or folder.
    pub fn first_match(&self, scope: SearchScope, query: &str) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        match scope {
            SearchScope::Apps => match &self.state.apps {
                Some(Ok(list)) => list
                    .iter()
                    .position(|a| a.label.to_lowercase().contains(query) || a.package.to_lowercase().contains(query)),
                _ => None,
            },
            SearchScope::Modules => match &self.state.modules {
                Some(Ok(list)) => list
                    .iter()
                    .position(|m| m.name.to_lowercase().contains(query) || m.id.to_lowercase().contains(query)),
                _ => None,
            },
            SearchScope::Features => None,
        }
    }

    /// The apps read for the page, or why they could not be.
    ///
    /// Nothing is redrawn here: the reading tells the list and then a name at
    /// a time, and the caller redraws once when the batch is in.
    pub fn set_apps(&mut self, list: Result<Vec<DataApp>, String>) {
        self.state.set_apps(list);
    }

    /// One app's name is read: put it in the list at `at`.
    ///
    /// Nothing is redrawn here. The names come in a batch, and redrawing on
    /// each one would hold the loop that takes the touches for as long as
    /// the batch lasts; the caller redraws once when the batch is in.
    pub fn data_app_enriched(&mut self, at: usize, label: String) {
        let Some(Ok(list)) = &mut self.state.apps else { return };
        let Some(app) = list.get_mut(at) else { return };
        app.label = label;
    }

    /// The apps the system's own partitions hold are read: the page that only
    /// looks at them shows this list. Nothing is redrawn here, as above.
    pub fn set_sys_apps(&mut self, list: Vec<DataApp>) {
        self.state.sys_apps = Some(list);
    }

    /// The apps are being read: the page says so until they come.
    pub fn set_apps_pending(&mut self) {
        self.state.apps = None;
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use crate::app::dataapp::DataApp;
    use crate::app::testing::*;
    use crate::app::{Action, App, Confirm, Effect, Screen, SearchScope};

    fn one_app(package: &str, label: &str, system: bool) -> DataApp {
        DataApp {
            package: package.into(),
            dir: if system { format!("/mnt/system/system/app/{package}") } else { format!("/data/app/{package}-hash") },
            label: label.into(),
            version: String::new(),
            installed: 1_700_000_000,
            updated: 1_700_000_000,
            size: 1024,
            system,
        }
    }

    fn page(list: Vec<DataApp>) -> App {
        let mut app = App::new(Screen::DataApps, W, H, PPD);
        app.state.apps = Some(Ok(list));
        app.refresh();
        app
    }

    #[test]
    fn a_search_finds_an_app_by_its_name_or_its_package() {
        let mut app_state = App::new(Screen::DataApps, W, H, PPD);
        app_state.state.apps = Some(Ok(vec![one_app("com.example.one", "One", false), one_app("com.example.two", "Two", false)]));
        assert_eq!(app_state.first_match(SearchScope::Apps, "two"), Some(1), "by name");
        assert_eq!(app_state.first_match(SearchScope::Apps, "example.two"), Some(1), "by package");
        assert_eq!(app_state.first_match(SearchScope::Apps, "one"), Some(0));
        assert_eq!(app_state.first_match(SearchScope::Apps, "nothing"), None);
    }

    #[test]
    fn a_search_with_nothing_typed_finds_nothing() {
        let mut app_state = App::new(Screen::DataApps, W, H, PPD);
        app_state.state.apps = Some(Ok(vec![one_app("com.example.one", "One", false)]));
        assert_eq!(app_state.first_match(SearchScope::Apps, ""), None);
    }

    #[test]
    fn a_name_read_for_an_app_lands_in_the_list_where_it_belongs() {
        let mut app_state = App::new(Screen::DataApps, W, H, PPD);
        app_state.state.apps = Some(Ok(vec![one_app("com.example.one", "com.example.one", false)]));
        app_state.data_app_enriched(0, "One".into());
        let Some(Ok(list)) = &app_state.state.apps else { panic!("the list is still there") };
        assert_eq!(list[0].label, "One");
    }

    #[test]
    fn a_name_for_an_app_that_is_not_there_is_put_nowhere() {
        let mut app_state = App::new(Screen::DataApps, W, H, PPD);
        app_state.state.apps = Some(Ok(vec![one_app("com.example.one", "One", false)]));
        // Out of the list, and with no list at all: neither is a panic.
        app_state.data_app_enriched(9, "Nine".into());
        app_state.state.apps = None;
        app_state.data_app_enriched(0, "One".into());
    }

    #[test]
    fn the_page_lists_what_was_read_and_opens_one() {
        let (f, i) = (fonts(), info());
        let mut app = page(vec![one_app("com.example.one", "One", false), one_app("com.example.two", "Two", false)]);
        assert!(tap(&mut app, &f, &i, Action::DataAppOpen(0)).is_empty());
        assert_eq!(app.screen(), Screen::DataAppInfo);
        assert_eq!(app.state.app_at, Some(0));
    }

    #[test]
    fn a_page_turned_goes_nowhere_below_the_first_screen() {
        let (f, i) = (fonts(), info());
        let mut app = page(vec![one_app("com.example.one", "One", false)]);
        // One app is one screen: the page it draws has no way on, and the
        // screen it stands on is the first.
        assert_eq!(app.state.app_page, 0);
        assert!(app.frame(&f, &i).hits.iter().all(|h| !matches!(h.action, Action::AppPage(_))), "nowhere to turn to");
    }

    #[test]
    fn a_page_is_told_where_to_go_rather_than_by_how_much() {
        let (f, i) = (fonts(), info());
        let mut app = page((0..40).map(|k| one_app(&format!("com.example.a{k}"), &format!("A{k}"), false)).collect());
        // A screen holds fewer than forty, so there is a page on.
        assert!(app.frame(&f, &i).hits.iter().any(|h| matches!(h.action, Action::AppPage(1))));
        tap(&mut app, &f, &i, Action::AppPage(1));
        assert_eq!(app.state.app_page, 1);
        // The list grows shorter than the page the owner stands on -- an app
        // taken off it -- and the page still draws: the page it goes back to
        // is the last one there is, not one past the end.
        app.state.apps = Some(Ok(vec![one_app("com.example.one", "One", false)]));
        app.refresh();
        assert!(app.frame(&f, &i).hits.iter().all(|h| !matches!(h.action, Action::AppPage(_))), "one screen, nowhere to turn");
    }

    #[test]
    fn taking_an_app_off_asks_for_the_swipe_and_then_removes_it() {
        let (f, i) = (fonts(), info());
        let mut app = page(vec![one_app("com.example.one", "One", false)]);
        tap(&mut app, &f, &i, Action::DataAppOpen(0));
        // The swipe is on the page: the app is not taken off until it is made.
        assert_eq!(
            swipe(&mut app, &f, &i, Confirm::DeleteApp),
            vec![Effect::DataAppRemove("com.example.one".into(), "/data/app/com.example.one-hash".into())]
        );
    }

    #[test]
    fn an_app_in_the_systems_own_partitions_is_not_taken_off_here() {
        let (f, i) = (fonts(), info());
        let mut app = page(vec![one_app("com.android.settings", "Settings", true)]);
        tap(&mut app, &f, &i, Action::DataAppOpen(0));
        assert!(app.frame(&f, &i).hits.iter().all(|h| !matches!(h.action, Action::Confirm(Confirm::DeleteApp))), "nothing to swipe");
    }

    #[test]
    fn a_page_not_read_yet_says_so_and_opens_nothing() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::DataApps, W, H, PPD);
        app.state.apps = None;
        app.refresh();
        assert!(app.frame(&f, &i).hits.iter().all(|h| !matches!(h.action, Action::DataAppOpen(_))), "nothing to open");
    }

    #[test]
    fn the_system_apps_page_lists_them_and_opens_nothing() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::SystemApps, W, H, PPD);
        app.state.sys_apps = Some(vec![one_app("com.android.settings", "Settings", true)]);
        app.refresh();
        assert!(app.frame(&f, &i).hits.iter().all(|h| !matches!(h.action, Action::DataAppOpen(_))), "a system app opens nothing");
    }

    #[test]
    fn a_search_looks_through_what_the_page_it_was_opened_from_holds() {
        let (f, i) = (fonts(), info());
        let mut app = page(vec![one_app("com.example.one", "One", false), one_app("com.example.two", "Two", false)]);
        tap(&mut app, &f, &i, Action::SearchApps);
        assert_eq!(app.screen(), Screen::Search);
        assert_eq!(app.state.search_scope, SearchScope::Apps);
        assert_eq!(app.first_match(SearchScope::Apps, "two"), Some(1));
    }
}
