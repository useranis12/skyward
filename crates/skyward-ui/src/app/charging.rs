//! What the charger is doing: the power it delivers, the protocol it
//! negotiated, and the battery it fills.
//!
//! This is a reading, not a setting. The recovery cannot change how fast the
//! phone charges. On this phone's charger interface the nodes that would hold
//! a limit are policy votes rather than figures — `iin_limit` reads
//! `buck eff_client:input_suspend 0`, `input_suspend` reads
//! `buck eff_client:wire_qc 1` — and a number written to either is refused
//! with `EINVAL`. The power a fast charge reaches is negotiated between the
//! kernel and the charger before the recovery is ever drawn, and the recovery
//! takes what it is given. What it can do is say what that is.

use super::*;

/// What the runtime last read from the phone's charger.
///
/// Every field is something the kernel tells: a phone that does not carry the
/// charger nodes leaves them None, and the page leaves those rows out rather
/// than showing a figure it does not have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChargeNow {
    /// The power going into the battery now, in tenths of a watt.
    pub now: Option<u32>,
    /// The most the charger has given since it was plugged in, in tenths of
    /// a watt: the number that says how fast this charge has run.
    pub most: Option<u32>,
    /// The protocol the charger negotiated, as the kernel names it: `PD_PPS`
    /// and the like. None where the phone does not say, or where nothing is
    /// plugged in.
    pub protocol: Option<String>,
    /// The most power the charger says it can give, in watts.
    pub rated: Option<u32>,
    /// The voltage the charger delivers, in microvolts.
    pub volts: Option<u64>,
    /// The current the charger delivers, in microamps.
    pub amps: Option<u64>,
}

impl App {
    /// Take in what the runtime read from the charger, or None where nothing
    /// is plugged in.
    ///
    /// The most the charger has given is kept here rather than by the
    /// runtime, since it is a fact about this charge and not about the phone:
    /// it rises as the charge does, and is forgotten when the charger goes.
    /// A charger that is plugged in but says nothing of its power leaves the
    /// most where it stands, since that is a gap in the reading rather than
    /// the end of the charge.
    ///
    /// A change is drawn as a whole page rather than as rows: the charger
    /// cards are as tall as what they have to say, and a card gains or loses
    /// a row as the charger comes up or goes — so a page drawn again in part
    /// came back a different height, which the drawing refuses. The refresh
    /// is asked for only where the figures a page shows have changed, which is
    /// what the comparison above settles.
    pub fn set_charge_state(&mut self, state: Option<ChargeNow>) {
        let Some(mut state) = state else {
            if self.state.charge != ChargeNow::default() {
                self.state.charge = ChargeNow::default();
                self.refresh();
            }
            return;
        };
        state.most = match (self.state.charge.most, state.now) {
            (Some(most), Some(now)) => Some(most.max(now)),
            (most, now) => most.or(now),
        };
        if self.state.charge != state {
            self.state.charge = state;
            self.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::*;

    #[test]
    fn the_most_the_charger_gave_is_kept_as_the_charge_rises_and_falls() {
        let mut app = App::new(Screen::Home, W, H, PPD);
        app.set_charge_state(Some(ChargeNow { now: Some(200), ..ChargeNow::default() }));
        assert_eq!(app.state.charge.most, Some(200), "the first reading is the most so far");
        app.set_charge_state(Some(ChargeNow { now: Some(500), ..ChargeNow::default() }));
        assert_eq!(app.state.charge.most, Some(500), "a higher reading raises it");
        app.set_charge_state(Some(ChargeNow { now: Some(300), ..ChargeNow::default() }));
        assert_eq!(app.state.charge.most, Some(500), "and a lower one leaves it standing");
    }

    #[test]
    fn a_charger_that_says_nothing_leaves_the_most_where_it_was() {
        let mut app = App::new(Screen::Home, W, H, PPD);
        app.set_charge_state(Some(ChargeNow { now: Some(400), ..ChargeNow::default() }));
        app.set_charge_state(Some(ChargeNow { now: None, ..ChargeNow::default() }));
        assert_eq!(app.state.charge.most, Some(400), "no reading is not a reading of nothing");
    }

    #[test]
    fn the_charger_going_forgets_what_it_gave() {
        let mut app = App::new(Screen::Home, W, H, PPD);
        app.set_charge_state(Some(ChargeNow { now: Some(400), ..ChargeNow::default() }));
        app.set_charge_state(None);
        assert_eq!(app.state.charge, ChargeNow::default(), "nothing plugged in: nothing to say");
    }
}
