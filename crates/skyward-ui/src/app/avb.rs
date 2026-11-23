//! The AVB page's state and the answers the runtime gives it.

use super::*;

/// Which AVB check the page asks to switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvbCheck {
    Verity,
    Verification,
}

/// What the runtime read from the top-level vbmeta image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvbInfo {
    pub verity: bool,
    pub verification: bool,
    pub rollback_index: u64,
    pub unlocked: Option<bool>,
    pub backup: bool,
}

impl App {
    pub fn set_avb(&mut self, answer: Result<AvbInfo, String>) {
        self.state.avb = Some(answer);
        if self.screen() == Screen::Avb {
            self.refresh();
        }
    }

    pub fn avb_ended(&mut self, notice: String, now: u64) {
        self.state.avb_notice = Some(notice);
        self.wake(now);
        self.refresh();
    }

    /// Start changing an AVB check, so the page cannot ask again while it
    /// is being written.
    pub fn avb_started(&mut self, check: AvbCheck) {
        self.state.avb_notice = Some(
            self.state
                .lang
                .pick(match check {
                    AvbCheck::Verity => [
                        "正在修改 dm-verity…",
                        "正在修改 dm-verity…",
                        "Changing dm-verity…",
                    ],
                    AvbCheck::Verification => [
                        "正在修改启动验证…",
                        "正在修改啟動驗證…",
                        "Changing boot verification…",
                    ],
                })
                .into(),
        );
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::*;

    /// The page with `state` read, unlocked so the switches show.
    fn page(state: AvbInfo) -> (App, Fonts, DeviceInfo) {
        let (f, i) = (fonts(), info());
        let mut app = opened(Screen::Avb, &f, &i);
        app.set_avb(Ok(state));
        (app, f, i)
    }

    fn unlocked(verity: bool, verification: bool) -> AvbInfo {
        AvbInfo { verity, verification, rollback_index: 3, unlocked: Some(true), backup: true }
    }

    #[test]
    fn the_page_says_what_turning_a_check_off_costs_and_how_to_come_back() {
        let (mut app, f, i) = page(unlocked(true, true));
        let (h, last) = {
            let frame = app.frame(&f, &i);
            let last = frame.hits.iter().filter(|h| matches!(h.action, Action::ToggleAvb(_) | Action::BackupVbmeta | Action::RestoreVbmeta)).map(|h| h.rect.y + h.rect.h).fold(0.0f32, f32::max);
            (frame.canvas.h, last)
        };
        // The warning and the way back stand below the switches, so the page
        // runs on past the last control it offers.
        assert!(h as f32 > last + 400.0, "the page carries the warning and the way back under the controls");
        assert_eq!(app.screen(), Screen::Avb);
    }

    #[test]
    fn the_switches_stay_above_the_warning_and_are_still_reachable() {
        let (mut app, f, i) = page(unlocked(true, true));
        let (toggles, backup) = {
            let frame = app.frame(&f, &i);
            let toggles: Vec<Rect> = frame.hits.iter().filter(|h| matches!(h.action, Action::ToggleAvb(_))).map(|h| h.rect).collect();
            let backup = frame.hits.iter().find(|h| h.action == Action::BackupVbmeta).map(|h| h.rect);
            (toggles, backup)
        };
        assert_eq!(toggles.len(), 2, "both checks are offered");
        let backup = backup.expect("a backup is offered");
        assert!(toggles.iter().all(|t| t.y < backup.y), "the checks come before the copy of what they change");
    }

    #[test]
    fn a_locked_bootloader_still_shows_what_turning_a_check_off_would_cost() {
        let (mut app, f, i) = page(AvbInfo { unlocked: Some(false), ..unlocked(true, true) });
        let (h, toggles) = {
            let frame = app.frame(&f, &i);
            (frame.canvas.h, frame.hits.iter().filter(|h| matches!(h.action, Action::ToggleAvb(_))).count())
        };
        // A locked bootloader offers nothing to press, but the warning and
        // the way back are still there to read.
        assert_eq!(toggles, 0, "nothing can be changed");
        assert!(h as f32 > H as f32, "the warning is shown whether or not the switches work");
    }
}

